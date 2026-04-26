//! HTTP layer.
//!
//! Three routes:
//! - `GET /metrics` — Prometheus exposition; optional Bearer auth.
//! - `GET /health` — always 200 `ok` (orchestrator probe).
//! - `GET /ready` — 200 `ready` when the breaker is closed/half-open,
//!   503 when open.
//!
//! `/health` and `/ready` are intentionally unauthenticated even when
//! `METRICS_TOKEN` is set: load balancers and orchestrators must be able to
//! probe liveness and readiness without credentials.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use subtle::ConstantTimeEq;

use crate::collector::Collector;
use crate::metrics::Metrics;

/// State shared across HTTP handlers.
pub struct AppState {
    pub metrics: Arc<Metrics>,
    pub collector: Arc<Collector>,
    pub metrics_token: Option<String>,
}

/// Build the public router with the three exporter routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .with_state(state)
}

/// Plain-text 401 with the RFC 7235 challenge header. The body intentionally
/// stays terse — it never reveals whether the failure was "no token", "wrong
/// length", or "wrong bytes".
const UNAUTHORIZED_BODY: &str = "missing or invalid Bearer token";

fn unauthorized() -> (StatusCode, [(&'static str, &'static str); 2], String) {
    (
        StatusCode::UNAUTHORIZED,
        [
            ("content-type", "text/plain; charset=utf-8"),
            ("www-authenticate", r#"Bearer realm="jellyfin-exporter""#),
        ],
        UNAUTHORIZED_BODY.to_owned(),
    )
}

/// Authenticate against an optional configured Bearer token.
///
/// Returns `true` if the request is authorized — either no token is
/// configured (auth disabled) or the request carries `Authorization: Bearer
/// <token>` whose value matches the configured token in constant time.
///
/// The byte-comparison goes through `subtle::ConstantTimeEq` and the boolean
/// combination uses bitwise `&` (not `&&`) so the auth decision itself does
/// not short-circuit on length match. There is a small residual timing
/// difference between equal-length and differing-length tokens (subtle's
/// `ct_eq` fast-paths differing-length to `Choice(0)`), which leaks only the
/// length match — token length is not considered secret in a Bearer-token
/// deployment.
fn is_authorized(headers: &HeaderMap, configured_token: Option<&str>) -> bool {
    let Some(expected) = configured_token else {
        // No token configured -> all requests authorized.
        return true;
    };

    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    let len_eq = provided.len() == expected.len();
    let bytes_eq: bool = provided.as_bytes().ct_eq(expected.as_bytes()).into();
    len_eq & bytes_eq
}

async fn metrics_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !is_authorized(&headers, state.metrics_token.as_deref()) {
        return unauthorized().into_response();
    }

    let body = state.metrics.encode();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn ready_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if state.collector.is_ready() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::*;
    use crate::collector::CollectorConfig;
    use axum_test::TestServer;
    use std::time::Duration;

    struct MockApi;

    #[async_trait::async_trait]
    impl JellyfinApi for MockApi {
        async fn get_sessions(&self) -> Result<Vec<Session>, ClientError> {
            Ok(vec![])
        }

        async fn get_libraries(&self) -> Result<Vec<Library>, ClientError> {
            Ok(vec![])
        }

        async fn get_item_counts(&self) -> Result<ItemCounts, ClientError> {
            Ok(ItemCounts {
                movie_count: 10,
                series_count: 5,
                episode_count: 50,
                book_count: 0,
                song_count: 0,
                album_count: 0,
                artist_count: 0,
                trailer_count: 0,
                music_video_count: 0,
                box_set_count: 0,
                item_count: 65,
            })
        }
        async fn get_system_info(&self) -> Result<SystemInfo, ClientError> {
            Ok(SystemInfo {
                server_name: "test".into(),
                version: "10.9.0".into(),
                operating_system: "Linux".into(),
            })
        }
        async fn get_library_item_count(&self, _parent_id: &str) -> Result<u64, ClientError> {
            Ok(0)
        }
        async fn is_publicly_reachable(&self) -> bool {
            true
        }
    }

    fn test_state() -> Arc<AppState> {
        let metrics = Arc::new(Metrics::new());
        let collector = Arc::new(Collector::new(
            Arc::new(MockApi),
            Arc::clone(&metrics),
            &CollectorConfig {
                scrape_interval: Duration::from_secs(60),
                failure_threshold: 5,
                reset_timeout: Duration::from_secs(60),
                retry_max_attempts: 0,
                retry_base_delay: Duration::from_millis(10),
                retry_max_delay: Duration::from_millis(100),
                expose_remote_address: false,
            },
        ));
        Arc::new(AppState {
            metrics,
            collector,
            metrics_token: None,
        })
    }

    fn test_state_with_auth(token: &str) -> Arc<AppState> {
        let metrics = Arc::new(Metrics::new());
        let collector = Arc::new(Collector::new(
            Arc::new(MockApi),
            Arc::clone(&metrics),
            &CollectorConfig {
                scrape_interval: Duration::from_secs(60),
                failure_threshold: 5,
                reset_timeout: Duration::from_secs(60),
                retry_max_attempts: 0,
                retry_base_delay: Duration::from_millis(10),
                retry_max_delay: Duration::from_millis(100),
                expose_remote_address: false,
            },
        ));
        Arc::new(AppState {
            metrics,
            collector,
            metrics_token: Some(token.to_owned()),
        })
    }

    #[tokio::test]
    async fn health_returns_200() {
        let server = TestServer::new(build_router(test_state())).unwrap();
        let response = server.get("/health").await;
        response.assert_status_ok();
        response.assert_text("ok");
    }

    #[tokio::test]
    async fn metrics_returns_prometheus_text() {
        let server = TestServer::new(build_router(test_state())).unwrap();
        let response = server.get("/metrics").await;
        response.assert_status_ok();

        let body = response.text();
        assert!(body.contains("jellyfin_up"));
    }

    #[tokio::test]
    async fn ready_returns_200_when_circuit_closed() {
        let server = TestServer::new(build_router(test_state())).unwrap();
        let response = server.get("/ready").await;
        response.assert_status_ok();
        response.assert_text("ready");
    }

    #[tokio::test]
    async fn metrics_auth_rejects_missing_token() {
        let server = TestServer::new(build_router(test_state_with_auth("secret-token"))).unwrap();
        let response = server.get("/metrics").await;
        response.assert_status(StatusCode::UNAUTHORIZED);
        assert!(response.text().contains("missing or invalid Bearer token"));
    }

    #[tokio::test]
    async fn metrics_auth_rejects_wrong_token() {
        let server = TestServer::new(build_router(test_state_with_auth("secret-token"))).unwrap();
        let response = server
            .get("/metrics")
            .add_header(
                "Authorization".parse::<axum::http::HeaderName>().unwrap(),
                "Bearer wrong-token"
                    .parse::<axum::http::HeaderValue>()
                    .unwrap(),
            )
            .await;
        response.assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn metrics_auth_rejects_short_token() {
        // Length-mismatched token must also be rejected — and the auth path
        // must not branch on length match alone.
        let server = TestServer::new(build_router(test_state_with_auth("secret-token"))).unwrap();
        let response = server
            .get("/metrics")
            .add_header(
                "Authorization".parse::<axum::http::HeaderName>().unwrap(),
                "Bearer x".parse::<axum::http::HeaderValue>().unwrap(),
            )
            .await;
        response.assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn metrics_auth_accepts_correct_token() {
        let server = TestServer::new(build_router(test_state_with_auth("secret-token"))).unwrap();
        let response = server
            .get("/metrics")
            .add_header(
                "Authorization".parse::<axum::http::HeaderName>().unwrap(),
                "Bearer secret-token"
                    .parse::<axum::http::HeaderValue>()
                    .unwrap(),
            )
            .await;
        response.assert_status_ok();
        assert!(response.text().contains("jellyfin_up"));
    }

    #[tokio::test]
    async fn unauthorized_response_carries_www_authenticate() {
        // RFC 7235: 401 responses must include a WWW-Authenticate challenge.
        let server = TestServer::new(build_router(test_state_with_auth("secret"))).unwrap();
        let response = server.get("/metrics").await;
        response.assert_status(StatusCode::UNAUTHORIZED);
        let challenge = response.header("www-authenticate");
        assert_eq!(
            challenge.to_str().unwrap(),
            r#"Bearer realm="jellyfin-exporter""#
        );
    }

    #[tokio::test]
    async fn health_unaffected_by_metrics_auth() {
        let server = TestServer::new(build_router(test_state_with_auth("secret-token"))).unwrap();
        let response = server.get("/health").await;
        response.assert_status_ok();
        response.assert_text("ok");
    }

    #[tokio::test]
    async fn ready_unaffected_by_metrics_auth() {
        let server = TestServer::new(build_router(test_state_with_auth("secret-token"))).unwrap();
        let response = server.get("/ready").await;
        response.assert_status_ok();
    }
}
