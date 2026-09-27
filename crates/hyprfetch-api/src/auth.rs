//! Bearer-token auth for non-loopback binds.
//!
//! Policy (matches `docs/api.md`):
//! - Loopback binds (`127.0.0.1` / `::1`): no auth — the daemon is a local
//!   desktop service.
//! - Non-loopback binds: every `/api/*` and `/ws` request must present the
//!   token, either as `Authorization: Bearer <token>` or (for browser
//!   WebSocket clients, which cannot set custom headers) as
//!   `?access_token=<token>`.
//! - `/healthz` and the embedded SPA stay open so liveness probes and the
//!   login page work without the token.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Apply the auth check to `req` and pass it to `next` when authorized.
pub async fn require_bearer(req: Request, next: Next, token: Arc<String>) -> Response {
    let authorized = authorized(&req, &token);
    if authorized {
        next.run(req).await
    } else {
        unauthorized()
    }
}

fn authorized(req: &Request, token: &str) -> bool {
    // 1. Authorization: Bearer <token>
    if let Some(v) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(presented) = v.strip_prefix("Bearer ") {
            return constant_time_eq(presented.trim(), token);
        }
    }
    // 2. ?access_token=<token> — browser WebSocket clients cannot set
    //    custom headers on the WS handshake.
    if let Some(q) = req.uri().query() {
        for pair in q.split('&') {
            if let Some(value) = pair.strip_prefix("access_token=") {
                return constant_time_eq(value, token);
            }
        }
    }
    false
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for (i, byte) in b.iter().enumerate() {
        diff |= a.get(i).copied().unwrap_or(0) ^ byte;
    }
    diff == 0
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::WWW_AUTHENTICATE, "Bearer".to_string()),
            (
                header::CONTENT_TYPE,
                "application/json".to_string(),
            ),
        ],
        r#"{"error":{"code":"unauthorized","message":"missing or invalid API token (Authorization: Bearer <token>, or ?access_token= for WebSocket clients)"}}"#,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{router, AppState};
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let db = hyprfetch_db::open_in_memory().expect("open_in_memory should succeed");
        crate::make_state(db)
    }

    fn authed_router() -> axum::Router {
        crate::router_with_token(test_state(), "secret-token-123".to_string())
    }

    async fn body_str(body: Body) -> String {
        String::from_utf8(to_bytes(body, 64 * 1024).await.unwrap().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn api_without_token_is_401() {
        let res = authed_router()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("unauthorized"), "got: {body}");
    }

    #[tokio::test]
    async fn api_with_wrong_token_is_401() {
        let res = authed_router()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .header("authorization", "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_with_bearer_token_passes() {
        let res = authed_router()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .header("authorization", "Bearer secret-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_with_access_token_query_passes() {
        let res = authed_router()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks?access_token=secret-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn healthz_and_ui_stay_open_with_auth() {
        let app = authed_router();

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "healthz is a liveness probe");

        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "SPA index stays reachable");
    }

    #[tokio::test]
    async fn router_without_token_does_not_auth() {
        let res = router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("abcd", "abc"));
        assert!(constant_time_eq("", ""));
    }
}
