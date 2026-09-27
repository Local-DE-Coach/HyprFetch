//! HTTP + WebSocket server (axum). Serves the embedded SPA and the REST/WS API.
//!
//! Stub. Real router lands in `feature/http-api`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Placeholder for the future router builder. Currently returns a static
/// `{"status":"ok"}` JSON response.
pub async fn healthz() -> &'static str {
    "{\"status\":\"ok\"}"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn healthz_returns_ok() {
        let body = healthz().await;
        assert!(body.contains("\"ok\""));
    }
}
