//! The minimal HTTP seam used by indexer adapters.
//!
//! Adapters fetch documents through a [`Fetcher`] rather than calling `reqwest`
//! directly. That keeps the protocol logic pure and lets the record/replay tests
//! feed recorded `t=caps`/search responses without any network — live indexers
//! are never a test dependency (`docs/06-integrations.md`).

use async_trait::async_trait;
use std::sync::Arc;

use std::future::Future;
use std::pin::Pin;

use tokio::sync::OnceCell;

use crate::error::{IndexerError, Result};

tokio::task_local! {
    /// Set for the duration of a [`Fetcher::in_session`] body, marking that this
    /// task already holds the FlareSolverr gate.
    ///
    /// The gate admits one request at a time, so a sequence holding it would
    /// deadlock against its own inner requests without this. A task-local is the
    /// right scope: it travels with the sequence and is invisible to every other
    /// task, which must still queue normally.
    static GATE_HELD: ();
}

/// A fetched page plus the context a follow-up request needs.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// The response body.
    pub body: String,
    /// The URL the request actually ended up at, which is not always the one
    /// asked for: a site may redirect to a canonical host, and a follow-up
    /// request bound to the page's session has to be addressed to *that* host or
    /// it never reaches the session at all.
    pub final_url: String,
}

/// Something that can fetch a URL and return its body as text.
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// GET `url` and return the response body as a string.
    async fn get(&self, url: &str) -> Result<String>;

    /// POST `body` (with `content_type`) to `url` and return the response body.
    ///
    /// Defaults to unsupported: only a fetcher that needs form-POST search (some
    /// Cardigann trackers) implements it. Record/replay test fetchers override it
    /// when a test exercises a POST-method definition.
    async fn post(&self, url: &str, _body: &str, _content_type: &str) -> Result<String> {
        Err(IndexerError::Unsupported(format!(
            "POST to {url} (this fetcher is GET-only)"
        )))
    }

    /// GET `url` and return the **raw, undecoded** response bytes.
    ///
    /// Needed by the Cardigann engine to honor a definition's declared `encoding`
    /// (e.g. `windows-1251`) when the server sends no/incorrect charset header. The
    /// default decodes nothing — it returns [`Fetcher::get`]'s UTF-8 bytes, which is
    /// correct for the UTF-8 case and for the string-replay test fetchers.
    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.get(url).await.map(String::into_bytes)
    }

    /// POST and return the **raw, undecoded** response bytes (see [`get_bytes`]).
    ///
    /// [`get_bytes`]: Fetcher::get_bytes
    async fn post_bytes(&self, url: &str, body: &str, content_type: &str) -> Result<Vec<u8>> {
        self.post(url, body, content_type)
            .await
            .map(String::into_bytes)
    }

    /// GET `url`, reporting where the request ended up as well as what it
    /// returned.
    ///
    /// The default reports the URL that was asked for, which is right for any
    /// fetcher that does not follow redirects on the caller's behalf.
    async fn get_page(&self, url: &str) -> Result<FetchedPage> {
        Ok(FetchedPage {
            body: self.get(url).await?,
            final_url: url.to_string(),
        })
    }

    /// POST the way a page's own script would: as a request carrying the session
    /// the page was fetched under.
    ///
    /// Distinct from [`Fetcher::post`], which for a browser-driving fetcher is a
    /// top-level *navigation*. A navigation is subject to the cookie's SameSite
    /// policy, and a session cookie is routinely withheld from a cross-site POST
    /// navigation — so the endpoint sees no session at all and rejects a
    /// perfectly well-formed request. A page's own XHR has no such problem, and
    /// this is the seam for reproducing it.
    ///
    /// The default is an ordinary POST, which is already right for a fetcher
    /// whose requests are plain HTTP rather than browser navigations.
    async fn post_in_page_session(&self, url: &str, body: &str) -> Result<String> {
        self.post(url, body, "application/x-www-form-urlencoded")
            .await
    }

    /// Run `op` with this fetcher's underlying session held for its whole
    /// duration, so a sequence of requests that must share one session is not
    /// interleaved with anyone else's.
    ///
    /// Needed when a later request is only valid against the session that served
    /// an earlier one — a signature bound to tokens from a details page, say.
    /// Serializing each request individually is not enough for that: it stops two
    /// requests overlapping but still lets an unrelated request land *between*
    /// them and rotate the session out from under the pair.
    ///
    /// The default runs `op` unchanged, which is correct for every fetcher whose
    /// requests carry no session identity.
    async fn in_session<'a>(
        &'a self,
        op: Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>,
    ) -> Result<String> {
        op.await
    }
}

/// A `reqwest`-backed fetcher for production use.
pub struct ReqwestFetcher {
    client: reqwest::Client,
}

impl ReqwestFetcher {
    /// Build a fetcher from an existing client (so callers control timeouts,
    /// proxies, and connection pooling centrally).
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for ReqwestFetcher {
    fn default() -> Self {
        Self::new(reqwest::Client::new())
    }
}

impl ReqwestFetcher {
    /// Read a response body, turning a non-success status into [`IndexerError::Status`]
    /// (a 403/429 here is the canonical "banned / rate-limited" signal).
    async fn read_body(resp: reqwest::Response) -> Result<String> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: status.as_u16(),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        Ok(resp.text().await?)
    }

    /// Read a response as raw bytes, with the same non-success → [`IndexerError::Status`]
    /// handling as [`read_body`](Self::read_body).
    async fn read_raw(resp: reqwest::Response) -> Result<Vec<u8>> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: status.as_u16(),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

/// One solved request: what came back, and where it came back from.
struct Solved {
    body: String,
    final_url: String,
}

/// The session context captured from a solve: what a follow-up request must
/// present to be recognized as coming from the same browser.
#[derive(Debug, Clone)]
struct PageSession {
    cookies: Vec<(String, String)>,
    user_agent: String,
}

/// A [`Fetcher`] that proxies through a **FlareSolverr** instance.
///
/// Some trackers sit behind an interstitial bot check that a plain HTTP client
/// cannot clear — the request never reaches the site, so no amount of definition
/// tuning helps. FlareSolverr drives a real browser, solves the challenge, and
/// returns the resulting document; keeping a named session across calls preserves
/// the clearance cookies so only the first request pays the solve cost.
///
/// This is **opt-in per indexer**. It introduces an external service, which the
/// single-binary/zero-required-services default must not depend on, so it is only
/// constructed when an operator configures an endpoint for a specific indexer.
pub struct FlareSolverrFetcher {
    client: reqwest::Client,
    /// Base URL of the FlareSolverr instance (its `/v1` endpoint is appended).
    endpoint: String,
    /// Session name, so clearance cookies persist across requests.
    session: String,
    /// How long FlareSolverr may spend solving, in milliseconds.
    max_timeout_ms: u64,
    /// Ensures the session is created once, on first use.
    session_ready: OnceCell<()>,
    /// Cookies and User-Agent from the most recent solve, so a follow-up request
    /// can be issued directly under the same session instead of as a navigation.
    /// Written on every solve; the sequence lock is what makes the pairing sound.
    page_session: std::sync::Mutex<Option<PageSession>>,
    /// One in-flight request at a time.
    ///
    /// A session is one browser. Driving several requests into it concurrently
    /// makes its tabs fight over the session and, under load, crash it outright —
    /// after which every later request on that session fails instantly. Serializing
    /// here is not a throughput loss: FlareSolverr processes a session's requests
    /// one at a time regardless.
    gate: tokio::sync::Semaphore,
}

impl FlareSolverrFetcher {
    /// Default solve budget. A challenge typically clears in 15-20s; below that a
    /// first request against a protected host fails spuriously.
    const DEFAULT_MAX_TIMEOUT_MS: u64 = 90_000;

    /// Build a fetcher pointed at a FlareSolverr instance (e.g.
    /// `http://flaresolverr:8191/`), tagging its session with `session`.
    #[must_use]
    pub fn new(
        client: reqwest::Client,
        endpoint: impl Into<String>,
        session: impl Into<String>,
    ) -> Self {
        Self {
            client,
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            session: session.into(),
            max_timeout_ms: Self::DEFAULT_MAX_TIMEOUT_MS,
            session_ready: OnceCell::new(),
            page_session: std::sync::Mutex::new(None),
            gate: tokio::sync::Semaphore::new(1),
        }
    }

    /// Build against `endpoint` with a client of this crate's choosing, so a caller
    /// that only has configuration (an endpoint string) needs no `reqwest` of its own.
    ///
    /// The client's timeout must exceed the solve budget — a challenge routinely
    /// takes 15-20s and the browser holds the request open for the whole of it, so a
    /// default-timeout client would abort every protected fetch.
    #[must_use]
    pub fn with_endpoint(endpoint: impl Into<String>, session: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(
                Self::DEFAULT_MAX_TIMEOUT_MS + 30_000,
            ))
            .build()
            .unwrap_or_default();
        Self::new(client, endpoint, session)
    }

    /// Override the solve budget in milliseconds.
    #[must_use]
    pub fn with_max_timeout_ms(mut self, max_timeout_ms: u64) -> Self {
        self.max_timeout_ms = max_timeout_ms;
        self
    }

    /// POST one command to FlareSolverr and return the decoded envelope.
    async fn command(&self, payload: serde_json::Value) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(format!("{}/v1", self.endpoint))
            .json(&payload)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: status.as_u16(),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        serde_json::from_str(&body)
            .map_err(|e| IndexerError::Parse(format!("flaresolverr envelope: {e}")))
    }

    /// Adopt the named session, creating it only if it isn't already there.
    ///
    /// Creating a session that exists resets its browser, throwing away the
    /// clearance cookies that make the *second* request cheap — and callers rebuild
    /// this fetcher often (indexer adapters are constructed per search), so a
    /// create-every-time would re-solve a challenge on every request and the solves
    /// would pile up until they time out. Listing first makes adoption idempotent
    /// across instances and across process restarts.
    async fn ensure_session(&self) {
        self.session_ready
            .get_or_init(|| async {
                let existing = self
                    .command(serde_json::json!({ "cmd": "sessions.list" }))
                    .await
                    .ok()
                    .and_then(|envelope| {
                        Some(
                            envelope
                                .get("sessions")?
                                .as_array()?
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .any(|s| s == self.session),
                        )
                    })
                    .unwrap_or(false);
                if !existing {
                    let _ = self
                        .command(serde_json::json!({
                            "cmd": "sessions.create",
                            "session": self.session,
                        }))
                        .await;
                }
            })
            .await;
    }

    /// Tear the session down and build a fresh one.
    ///
    /// The browser behind a session can die (`tab crashed`), and every later
    /// request on it then fails instantly — adopting a session forever would pin
    /// the indexer to a corpse. Destroying first is what makes the create
    /// meaningful, since creating over a live session is what we avoid elsewhere.
    async fn reset_session(&self) {
        let _ = self
            .command(serde_json::json!({
                "cmd": "sessions.destroy",
                "session": self.session,
            }))
            .await;
        let _ = self
            .command(serde_json::json!({
                "cmd": "sessions.create",
                "session": self.session,
            }))
            .await;
    }

    /// Whether a failure means the session's browser is unusable rather than the
    /// site being slow, so it is worth rebuilding the session and trying once more.
    fn is_session_fault(error: &IndexerError) -> bool {
        let text = error.to_string();
        [
            "tab crashed",
            "invalid session",
            "session deleted",
            "no such session",
            "chrome not reachable",
        ]
        .iter()
        .any(|marker| text.to_ascii_lowercase().contains(marker))
    }

    /// Run a `request.get`/`request.post` and return the page body, serialized
    /// against this session and retried once if the session itself has died.
    async fn request(&self, payload: serde_json::Value) -> Result<Solved> {
        // Skip the gate when this task already holds it for a whole sequence:
        // acquiring the single permit again would deadlock against ourselves.
        let _permit = if GATE_HELD.try_with(|()| ()).is_ok() {
            None
        } else {
            Some(
                self.gate
                    .acquire()
                    .await
                    .map_err(|_| IndexerError::Parse("flaresolverr gate closed".to_string()))?,
            )
        };
        self.ensure_session().await;
        match self.request_once(payload.clone()).await {
            Err(err) if Self::is_session_fault(&err) => {
                tracing::warn!(
                    session = %self.session,
                    error = %err,
                    "flaresolverr session died; rebuilding it and retrying once"
                );
                self.reset_session().await;
                self.request_once(payload).await
            }
            other => other,
        }
    }

    /// One attempt, with no session handling.
    async fn request_once(&self, payload: serde_json::Value) -> Result<Solved> {
        let envelope = self.command(payload).await?;

        if envelope.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
            let message = envelope
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no message");
            return Err(IndexerError::Parse(format!("flaresolverr: {message}")));
        }
        // The upstream status lives on the solution; FlareSolverr itself answers 200
        // even when the site returned an error page, so a non-success here must be
        // surfaced as the tracker being unavailable rather than as empty results.
        let solution = envelope
            .get("solution")
            .ok_or_else(|| IndexerError::Parse("flaresolverr reply had no solution".to_string()))?;
        let upstream = solution
            .get("status")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let body = solution
            .get("response")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !(200..300).contains(&upstream) {
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: u16::try_from(upstream).unwrap_or(0),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        // Keep the session this solve ran under, so a follow-up request can be
        // issued directly as the page rather than as a navigation.
        let cookies: Vec<(String, String)> = solution
            .get("cookies")
            .and_then(serde_json::Value::as_array)
            .map(|cs| {
                cs.iter()
                    .filter_map(|c| {
                        Some((
                            c.get("name")?.as_str()?.to_string(),
                            c.get("value")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let user_agent = solution
            .get("userAgent")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !cookies.is_empty() {
            if let Ok(mut slot) = self.page_session.lock() {
                *slot = Some(PageSession {
                    cookies,
                    user_agent,
                });
            }
        }
        // Where the request ended up, which a redirect to a canonical host makes
        // different from where it was aimed.
        let final_url = solution
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(Solved {
            body: unwrap_pre(&body),
            final_url,
        })
    }
}

/// Long-lived [`FlareSolverrFetcher`] instances, keyed by endpoint and session.
///
/// Indexer adapters are rebuilt for every search, but a FlareSolverr session is
/// expensive to establish and must not be driven concurrently. Holding the
/// fetchers here — one shared pool passed in alongside the rate limiter — means
/// every search for an indexer reuses that indexer's session and its
/// one-at-a-time gate, instead of standing up a rival browser each time.
#[derive(Default)]
pub struct FetcherPool {
    flaresolverr: std::sync::Mutex<std::collections::HashMap<String, Arc<FlareSolverrFetcher>>>,
}

impl FetcherPool {
    /// An empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The shared fetcher for `endpoint`/`session`, creating it on first use.
    #[must_use]
    pub fn flaresolverr(&self, endpoint: &str, session: &str) -> Arc<dyn Fetcher> {
        let key = format!("{endpoint}\u{0}{session}");
        let mut map = self
            .flaresolverr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            map.entry(key)
                .or_insert_with(|| Arc::new(FlareSolverrFetcher::with_endpoint(endpoint, session))),
        ) as Arc<dyn Fetcher>
    }
}

/// Unwrap a non-HTML body from the `<pre>` block the headless browser renders it in.
///
/// A JSON endpoint fetched through FlareSolverr comes back as a rendered document,
/// not raw bytes, so callers expecting JSON would otherwise get markup. A body with
/// no single `<pre>` is returned unchanged.
fn unwrap_pre(body: &str) -> String {
    let Some(start) = body.find("<pre") else {
        return body.to_string();
    };
    let Some(open_end) = body[start..].find('>').map(|i| start + i + 1) else {
        return body.to_string();
    };
    let Some(close) = body[open_end..].find("</pre>").map(|i| open_end + i) else {
        return body.to_string();
    };
    let inner = &body[open_end..close];
    inner
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[async_trait]
impl Fetcher for FlareSolverrFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        self.get_page(url).await.map(|p| p.body)
    }

    async fn get_page(&self, url: &str) -> Result<FetchedPage> {
        let solved = self
            .request(serde_json::json!({
                "cmd": "request.get",
                "url": url,
                "session": self.session,
                "maxTimeout": self.max_timeout_ms,
            }))
            .await?;
        let final_url = if solved.final_url.is_empty() {
            url.to_string()
        } else {
            solved.final_url
        };
        Ok(FetchedPage {
            body: solved.body,
            final_url,
        })
    }

    /// Issue the POST directly, presenting the cookies the solve established,
    /// instead of driving the browser to navigate to it.
    ///
    /// A navigation is subject to SameSite, which withholds the session cookie
    /// from a cross-site POST — the endpoint then sees no session and rejects a
    /// request that is otherwise exactly right. Sending it ourselves reproduces
    /// what the page's own script does. The User-Agent is carried over with the
    /// cookies because a clearance cookie is issued against one.
    async fn post_in_page_session(&self, url: &str, body: &str) -> Result<String> {
        let Some(session) = self.page_session.lock().ok().and_then(|s| s.clone()) else {
            // Nothing solved yet, so there is no session to present; a navigation
            // is no worse than nothing here.
            return self
                .post(url, body, "application/x-www-form-urlencoded")
                .await;
        };
        let cookies = session
            .cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let resp = self
            .client
            .post(url)
            .header(reqwest::header::COOKIE, cookies)
            .header(reqwest::header::USER_AGENT, session.user_agent)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body.to_string())
            .send()
            .await?;
        ReqwestFetcher::read_body(resp).await
    }

    async fn post(&self, url: &str, body: &str, _content_type: &str) -> Result<String> {
        // FlareSolverr only submits form-encoded bodies; it derives the content type
        // itself, so the caller's is not forwarded.
        self.request(serde_json::json!({
            "cmd": "request.post",
            "url": url,
            "postData": body,
            "session": self.session,
            "maxTimeout": self.max_timeout_ms,
        }))
        .await
        .map(|s| s.body)
    }

    async fn in_session<'a>(
        &'a self,
        op: Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>,
    ) -> Result<String> {
        // Take the gate once for the whole sequence rather than once per request.
        // Holding it end to end is the point: it is what stops another search
        // landing between two requests that have to share a session.
        let _permit = self
            .gate
            .acquire()
            .await
            .map_err(|_| IndexerError::Parse("flaresolverr gate closed".to_string()))?;
        self.ensure_session().await;
        GATE_HELD.scope((), op).await
    }
}

#[async_trait]
impl Fetcher for ReqwestFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        let resp = self.client.get(url).send().await?;
        Self::read_body(resp).await
    }

    async fn post(&self, url: &str, body: &str, content_type: &str) -> Result<String> {
        let resp = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body.to_string())
            .send()
            .await?;
        Self::read_body(resp).await
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.client.get(url).send().await?;
        Self::read_raw(resp).await
    }

    async fn post_bytes(&self, url: &str, body: &str, content_type: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body.to_string())
            .send()
            .await?;
        Self::read_raw(resp).await
    }
}
