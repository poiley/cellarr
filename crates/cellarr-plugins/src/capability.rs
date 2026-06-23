//! The capability layer — the *only* authority a plugin can have.
//!
//! The WIT world imports exactly one host capability (`http.fetch`). This module
//! defines what that capability is and how the host decides whether a given
//! plugin gets it. The default is **deny-all**: a freshly built [`HostConfig`]
//! grants nothing, so a plugin can compute but cannot touch the network. The
//! host opts a plugin in by handing it an [`HttpCapability`] (typically an
//! allow-list-scoped fetcher), never raw sockets.
//!
//! This indirection is what lets the production daemon plug a real
//! `reqwest`-backed fetcher behind the same trait the unit tests exercise with
//! an in-memory stub — without the guest ever knowing the difference.

use crate::error::{PluginError, Result};

/// A response the host returns to a guest's HTTP fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body bytes.
    pub body: Vec<u8>,
}

/// The single network capability a plugin may be granted.
///
/// Implementations enforce *all* policy: which URLs are reachable, timeouts, and
/// response-size caps. The guest cannot widen this — it only sees a `fetch(url)`
/// that may succeed or fail. The daemon's production impl wraps `reqwest` with
/// an allow-list; tests use deterministic in-memory impls.
pub trait HttpCapability: Send + Sync {
    /// Fetch `url`. Returns [`PluginError::CapabilityDenied`] if policy forbids
    /// the request, so a guest probing for ambient authority gets a clean,
    /// typed refusal rather than silent success.
    fn fetch(&self, url: &str) -> Result<HttpResponse>;
}

/// The default capability: deny every request.
///
/// A plugin wired to this can do nothing on the network. This is what a
/// plugin gets unless the host explicitly grants something narrower.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAllHttp;

impl HttpCapability for DenyAllHttp {
    fn fetch(&self, url: &str) -> Result<HttpResponse> {
        Err(PluginError::CapabilityDenied(format!(
            "http fetch is not granted to this plugin (attempted: {url})"
        )))
    }
}

/// An allow-list-scoped HTTP capability.
///
/// Only requests whose URL begins with one of the configured prefixes are
/// allowed; everything else is denied. The actual transport is injected as a
/// closure so the daemon can supply a real client and tests can supply a
/// fixture — neither widens what the guest can reach, because the allow-list is
/// checked here, before the transport ever runs.
pub struct AllowListHttp<F> {
    allowed_prefixes: Vec<String>,
    transport: F,
}

impl<F> AllowListHttp<F>
where
    F: Fn(&str) -> Result<HttpResponse> + Send + Sync,
{
    /// Build an allow-list capability over `transport`, permitting only URLs
    /// that start with one of `allowed_prefixes`.
    pub fn new(allowed_prefixes: Vec<String>, transport: F) -> Self {
        Self {
            allowed_prefixes,
            transport,
        }
    }

    fn is_allowed(&self, url: &str) -> bool {
        self.allowed_prefixes.iter().any(|p| url.starts_with(p))
    }
}

impl<F> HttpCapability for AllowListHttp<F>
where
    F: Fn(&str) -> Result<HttpResponse> + Send + Sync,
{
    fn fetch(&self, url: &str) -> Result<HttpResponse> {
        if !self.is_allowed(url) {
            return Err(PluginError::CapabilityDenied(format!(
                "url not on allow-list: {url}"
            )));
        }
        (self.transport)(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_all_refuses_everything() {
        let cap = DenyAllHttp;
        let err = cap.fetch("https://example.invalid/caps").unwrap_err();
        assert!(matches!(err, PluginError::CapabilityDenied(_)));
    }

    #[test]
    fn allow_list_permits_only_matching_prefix() {
        let cap = AllowListHttp::new(vec!["https://api.allowed.test/".to_string()], |url| {
            Ok(HttpResponse {
                status: 200,
                body: url.as_bytes().to_vec(),
            })
        });

        let ok = cap.fetch("https://api.allowed.test/search").unwrap();
        assert_eq!(ok.status, 200);

        let denied = cap.fetch("https://evil.test/exfiltrate").unwrap_err();
        assert!(
            matches!(denied, PluginError::CapabilityDenied(_)),
            "off-list URL must be denied, got {denied:?}"
        );
    }
}
