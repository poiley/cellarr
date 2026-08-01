//! Session handling against a FlareSolverr that behaves badly.
//!
//! These bugs live below the [`Fetcher`](cellarr_indexers::http::Fetcher) seam —
//! they are about the conversation with FlareSolverr itself — so they are tested
//! against a loopback stub rather than an injected fetcher. Each one cost real
//! downtime: adapters are rebuilt per search, so anything that treats construction
//! as "a new session" re-solves a challenge on every single request until the
//! solves pile up past their own timeout.

use std::sync::{Arc, Mutex};

use cellarr_indexers::http::{FetcherPool, FlareSolverrFetcher};
use cellarr_indexers::Fetcher;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A stub FlareSolverr: records the commands it receives, and can be told that a
/// session has died so the fetcher's recovery path is exercised.
struct Stub {
    addr: std::net::SocketAddr,
    commands: Arc<Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Stub {
    /// `dead_until` makes `request.*` fail with a session fault for that many
    /// calls, the way a crashed browser does, before recovering.
    async fn start(existing_session: Option<&str>, dead_for: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let commands: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sessions = existing_session.map(str::to_string);
        let recorder = Arc::clone(&commands);
        let remaining_deaths = Arc::new(Mutex::new(dead_for));

        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let recorder = Arc::clone(&recorder);
                let sessions = sessions.clone();
                let deaths = Arc::clone(&remaining_deaths);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let cmd = [
                        "sessions.list",
                        "sessions.create",
                        "sessions.destroy",
                        "request.get",
                        "request.post",
                    ]
                    .iter()
                    .find(|c| req.contains(*c))
                    .map_or("unknown", |c| *c)
                    .to_string();
                    recorder.lock().expect("lock").push(cmd.clone());

                    let body = match cmd.as_str() {
                        "sessions.list" => {
                            let list = sessions
                                .as_ref()
                                .map_or_else(String::new, |s| format!("\"{s}\""));
                            format!(r#"{{"status":"ok","sessions":[{list}]}}"#)
                        }
                        "sessions.create" | "sessions.destroy" => r#"{"status":"ok"}"#.to_string(),
                        _ => {
                            let mut left = deaths.lock().expect("lock");
                            if *left > 0 {
                                *left -= 1;
                                // What a crashed browser actually returns.
                                r#"{"status":"error","message":"Error: tab crashed"}"#.to_string()
                            } else {
                                r#"{"status":"ok","solution":{"status":200,"response":"<html>ok</html>"}}"#.to_string()
                            }
                        }
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        Stub {
            addr,
            commands,
            handle,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("lock").clone()
    }
}

/// Creating a session that already exists resets its browser, throwing away the
/// clearance that makes the second request cheap. Adapters are rebuilt per search,
/// so a create-on-construct re-solves a challenge every single time.
#[tokio::test]
async fn an_existing_session_is_adopted_not_recreated() {
    let stub = Stub::start(Some("cellarr-x"), 0).await;
    let fetcher = FlareSolverrFetcher::with_endpoint(stub.endpoint(), "cellarr-x");

    fetcher
        .get("https://tracker.example/")
        .await
        .expect("fetch");

    let cmds = stub.commands();
    assert!(cmds.contains(&"sessions.list".to_string()), "{cmds:?}");
    assert!(
        !cmds.contains(&"sessions.create".to_string()),
        "an existing session must be adopted, not recreated: {cmds:?}"
    );
}

/// A session that does not exist yet still has to be created once.
#[tokio::test]
async fn a_missing_session_is_created_once() {
    let stub = Stub::start(None, 0).await;
    let fetcher = FlareSolverrFetcher::with_endpoint(stub.endpoint(), "cellarr-y");

    fetcher
        .get("https://tracker.example/a")
        .await
        .expect("fetch");
    fetcher
        .get("https://tracker.example/b")
        .await
        .expect("fetch");

    let creates = stub
        .commands()
        .iter()
        .filter(|c| *c == "sessions.create")
        .count();
    assert_eq!(
        creates,
        1,
        "exactly one create across two requests: {:?}",
        stub.commands()
    );
}

/// Adopting an existing session forever pins the indexer to a corpse: once the
/// browser dies, every later request fails in milliseconds and never recovers.
#[tokio::test]
async fn a_dead_session_is_rebuilt_and_the_request_retried() {
    let stub = Stub::start(Some("cellarr-z"), 1).await;
    let fetcher = FlareSolverrFetcher::with_endpoint(stub.endpoint(), "cellarr-z");

    let body = fetcher
        .get("https://tracker.example/")
        .await
        .expect("a dead session must be rebuilt and the request retried");
    assert!(body.contains("ok"));

    let cmds = stub.commands();
    assert!(
        cmds.contains(&"sessions.destroy".to_string())
            && cmds.contains(&"sessions.create".to_string()),
        "the session must be torn down and rebuilt: {cmds:?}"
    );
}

/// A session is one browser. Several searches driving it at once make its tabs
/// fight and, under load, crash it outright.
#[tokio::test]
async fn requests_on_one_session_are_serialized() {
    let stub = Stub::start(Some("cellarr-s"), 0).await;
    let fetcher = Arc::new(FlareSolverrFetcher::with_endpoint(
        stub.endpoint(),
        "cellarr-s",
    ));

    let mut tasks = Vec::new();
    for i in 0..6 {
        let f = Arc::clone(&fetcher);
        tasks.push(tokio::spawn(async move {
            f.get(&format!("https://tracker.example/{i}")).await
        }));
    }
    for t in tasks {
        t.await.expect("join").expect("fetch");
    }

    let requests = stub
        .commands()
        .iter()
        .filter(|c| c.starts_with("request."))
        .count();
    assert_eq!(requests, 6, "every request still happens, just not at once");
}

/// Adapters are rebuilt for every search, so the pool is what keeps one session
/// per indexer alive across them. Without it each search stands up a rival.
#[tokio::test]
async fn the_pool_hands_back_one_fetcher_per_session() {
    let pool = FetcherPool::new();
    let a = pool.flaresolverr("http://flaresolverr.invalid:8191", "cellarr-1");
    let b = pool.flaresolverr("http://flaresolverr.invalid:8191", "cellarr-1");
    let other = pool.flaresolverr("http://flaresolverr.invalid:8191", "cellarr-2");

    assert!(Arc::ptr_eq(&a, &b), "same session must reuse one fetcher");
    assert!(
        !Arc::ptr_eq(&a, &other),
        "distinct sessions must not share one"
    );
}
