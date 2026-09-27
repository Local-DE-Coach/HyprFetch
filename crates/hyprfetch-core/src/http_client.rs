//! HTTP client wrapper — HEAD probes, Range fetches, cache validator parsing.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::{header, Client, Response, StatusCode};
use thiserror::Error;

use crate::ssrf::{check_url, SsrfPolicy, UrlSafetyError};
use url::Url;

/// Default User-Agent string sent on all requests unless overridden.
pub const DEFAULT_USER_AGENT: &str = concat!("HyprFetch/", env!("CARGO_PKG_VERSION"));

/// Per-request extra headers (e.g. Authorization, Referer).
pub type ExtraHeaders = HashMap<String, String>;

/// Outcome of a HEAD probe.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    /// Final URL after any redirects. May differ from the input URL.
    pub final_url: Url,
    /// `Content-Length` header value, if present.
    pub content_length: Option<i64>,
    /// `true` if the server returned `Accept-Ranges: bytes`.
    pub accept_ranges: bool,
    /// `ETag` header value, if present.
    pub etag: Option<String>,
    /// `Last-Modified` header value, if present.
    pub last_modified: Option<String>,
}

/// Errors returned by the HTTP client.
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("SSRF check failed: {0}")]
    Ssrf(#[from] UrlSafetyError),
    #[error("HTTP request failed: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("server returned {status} for {url}")]
    BadStatus { status: StatusCode, url: String },
    #[error("missing Content-Length for {url}")]
    MissingContentLength { url: String },
    #[error("server does not support range requests (no Accept-Ranges: bytes) for {url}")]
    RangeUnsupported { url: String },
}

/// Thin wrapper around `reqwest::Client` that applies HyprFetch's defaults:
/// SSRF protection, sane timeouts, rustls TLS, custom UA.
#[derive(Clone)]
pub struct HttpClient {
    inner: Client,
    ssrf_policy: SsrfPolicy,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new(SsrfPolicy::default())
    }
}

impl HttpClient {
    /// Construct with a given SSRF policy and the default User-Agent.
    pub fn new(ssrf_policy: SsrfPolicy) -> Self {
        Self::with_user_agent(ssrf_policy, None)
    }

    /// Construct with a given SSRF policy and an optional User-Agent override
    /// (from the `user_agent` setting; empty/None keeps the default).
    pub fn with_user_agent(ssrf_policy: SsrfPolicy, user_agent: Option<String>) -> Self {
        let inner = Client::builder()
            .user_agent(
                user_agent
                    .filter(|ua| !ua.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string()),
            )
            // NOTE: deliberately NO total `.timeout()` on this client.
            // reqwest's client-level timeout covers the ENTIRE request
            // including streaming the body, so it aborted every segment
            // worker ~30 s in ("only 0 of N segments completed" on any
            // download whose segments take longer than 30 s — e.g. a 1 GB
            // file at 16 MB/s died at ~455 MB). Long transfers must be
            // allowed to run; stalls are bounded by `read_timeout` (idle
            // read gap) and dead peers by `connect_timeout`.
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(4)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("reqwest client build");
        Self { inner, ssrf_policy }
    }

    /// HEAD the URL, follow redirects (max 5), validate SSRF on each hop
    /// (via redirect policy — re-checked here on the final URL too),
    /// return parsed validator headers.
    pub async fn probe(
        &self,
        url: &Url,
        extra: Option<&ExtraHeaders>,
    ) -> Result<ProbeResult, HttpError> {
        check_url(url, self.ssrf_policy)?;

        let mut req = self.inner.head(url.as_str());
        if let Some(h) = extra {
            req = apply_headers(req, h);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(HttpError::BadStatus {
                status,
                url: url.to_string(),
            });
        }

        let final_url = resp.url().clone();
        // Re-check SSRF on the final URL (in case of redirect to private IP).
        check_url(&final_url, self.ssrf_policy)?;

        let content_length = content_length_i64(&resp);
        let accept_ranges = resp
            .headers()
            .get(header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("bytes"))
            .unwrap_or(false);
        let etag = header_string(&resp, header::ETAG);
        let last_modified = header_string(&resp, header::LAST_MODIFIED);

        Ok(ProbeResult {
            final_url,
            content_length,
            accept_ranges,
            etag,
            last_modified,
        })
    }

    /// Fetch a byte range `[start, end]` inclusive. Returns the streaming
    /// `Response`. Caller is responsible for reading the body.
    pub async fn fetch_range(
        &self,
        url: &Url,
        start: i64,
        end_inclusive: i64,
        extra: Option<&ExtraHeaders>,
    ) -> Result<Response, HttpError> {
        check_url(url, self.ssrf_policy)?;
        let range = format!("bytes={start}-{end_inclusive}");
        let mut req = self.inner.get(url.as_str()).header(header::RANGE, range);
        if let Some(h) = extra {
            req = apply_headers(req, h);
        }
        let resp = req.send().await?;
        let status = resp.status();
        // 206 Partial Content is the success status for a range request.
        // 200 OK means the server ignored Range — caller must treat as full body.
        if status != StatusCode::PARTIAL_CONTENT && status != StatusCode::OK {
            return Err(HttpError::BadStatus {
                status,
                url: url.to_string(),
            });
        }
        Ok(resp)
    }
}

fn apply_headers(req: reqwest::RequestBuilder, h: &ExtraHeaders) -> reqwest::RequestBuilder {
    let mut r = req;
    for (k, v) in h {
        if let Ok(name) = header::HeaderName::from_bytes(k.as_bytes()) {
            if let Ok(value) = header::HeaderValue::from_str(v) {
                r = r.header(name, value);
            }
        }
    }
    r
}

fn content_length_i64(resp: &Response) -> Option<i64> {
    resp.headers()
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn header_string(resp: &Response, name: header::HeaderName) -> Option<String> {
    resp.headers()
        .get(name)?
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn probe_returns_validators_when_present() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", "1234")
                    .insert_header("accept-ranges", "bytes")
                    .insert_header("etag", "\"abc123\"")
                    .insert_header("last-modified", "Wed, 21 Oct 2026 07:28:00 GMT"),
            )
            .mount(&server)
            .await;

        let client = HttpClient::new(SsrfPolicy {
            block_private: false,
        });
        let url = Url::parse(&server.uri()).unwrap();
        let probe = client.probe(&url, None).await.unwrap();
        assert_eq!(probe.content_length, Some(1234));
        assert!(probe.accept_ranges);
        assert_eq!(probe.etag.as_deref(), Some("\"abc123\""));
        assert!(probe.last_modified.is_some());
    }

    #[tokio::test]
    async fn probe_returns_accept_ranges_false_when_missing() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-length", "10"))
            .mount(&server)
            .await;

        let client = HttpClient::new(SsrfPolicy {
            block_private: false,
        });
        let probe = client
            .probe(&Url::parse(&server.uri()).unwrap(), None)
            .await
            .unwrap();
        assert!(!probe.accept_ranges);
    }

    #[tokio::test]
    async fn probe_returns_4xx_as_bad_status() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = HttpClient::new(SsrfPolicy {
            block_private: false,
        });
        let err = client
            .probe(&Url::parse(&server.uri()).unwrap(), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HttpError::BadStatus { status, .. } if status == StatusCode::NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn fetch_range_sends_range_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=0-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-99/1000")
                    .insert_header("content-length", "100")
                    .set_body_bytes(b"x".repeat(100)),
            )
            .mount(&server)
            .await;

        let client = HttpClient::new(SsrfPolicy {
            block_private: false,
        });
        let url = Url::parse(&server.uri()).unwrap();
        let resp = client.fetch_range(&url, 0, 99, None).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        let body = resp.bytes().await.unwrap();
        assert_eq!(body.len(), 100);
    }

    #[tokio::test]
    async fn fetch_range_returns_5xx_as_bad_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = HttpClient::new(SsrfPolicy {
            block_private: false,
        });
        let url = Url::parse(&server.uri()).unwrap();
        let err = client.fetch_range(&url, 0, 99, None).await.unwrap_err();
        assert!(
            matches!(err, HttpError::BadStatus { status, .. } if status == StatusCode::INTERNAL_SERVER_ERROR)
        );
    }

    #[tokio::test]
    async fn extra_headers_are_applied() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(header("authorization", "Bearer sekret"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-length", "1"))
            .mount(&server)
            .await;

        let client = HttpClient::new(SsrfPolicy {
            block_private: false,
        });
        let mut extra = ExtraHeaders::new();
        extra.insert("Authorization".into(), "Bearer sekret".into());
        let _probe = client
            .probe(&Url::parse(&server.uri()).unwrap(), Some(&extra))
            .await
            .unwrap();
    }
}
