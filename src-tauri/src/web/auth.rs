use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::{
    extract::{ConnectInfo, Request},
    http::{header::RETRY_AFTER, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use super::auth_limiter::AuthFailureLimiter;

pub const WS_EVENT_PROTOCOL: &str = "codeg-events";
const WS_TOKEN_PROTOCOL_PREFIX: &str = "codeg-token.";

fn token_from_ws_protocols(value: &str) -> Option<String> {
    value
        .split(',')
        .map(str::trim)
        .find_map(|protocol| protocol.strip_prefix(WS_TOKEN_PROTOCOL_PREFIX))
        .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

pub async fn require_token(request: Request, next: Next, token: String) -> Response {
    // Keep the existing standalone middleware API safe as well. Production
    // routers pass their own shared limiter to require_token_with_limiter.
    static LIMITER: OnceLock<Arc<AuthFailureLimiter>> = OnceLock::new();
    let limiter = LIMITER.get_or_init(|| Arc::new(AuthFailureLimiter::default()));
    require_token_with_limiter(request, next, token, limiter.clone()).await
}

pub(crate) async fn require_token_with_limiter(
    request: Request,
    next: Next,
    token: String,
    limiter: Arc<AuthFailureLimiter>,
) -> Response {
    // Fail closed on a misconfigured empty token: otherwise `Bearer ` (an empty
    // bearer value) would match it and silently disable authentication.
    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "Server token is not configured").into_response();
    }

    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    // A ban takes precedence over valid credentials and is not extended by
    // retries. Forwarded headers are untrusted and deliberately ignored.
    if let Some(retry_after) = limiter.retry_after(peer) {
        return too_many_failures(retry_after);
    }

    if let Some(auth_header) = request.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if auth_str
                .strip_prefix("Bearer ")
                .is_some_and(|t| tokens_match(t, &token))
            {
                return next.run(request).await;
            }
        }
    }

    if let Some(protocol_header) = request.headers().get("sec-websocket-protocol") {
        if let Ok(protocols) = protocol_header.to_str() {
            if token_from_ws_protocols(protocols).is_some_and(|t| tokens_match(&t, &token)) {
                return next.run(request).await;
            }
        }
    }

    match limiter.record_failure(peer) {
        Some(retry_after) => too_many_failures(retry_after),
        None => (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response(),
    }
}

fn tokens_match(candidate: &str, expected: &str) -> bool {
    !expected.is_empty()
        && constant_time_eq::constant_time_eq(candidate.as_bytes(), expected.as_bytes())
}

fn too_many_failures(retry_after_secs: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(RETRY_AFTER, retry_after_secs.to_string())],
        Json(serde_json::json!({
            "code": "too_many_auth_failures",
            "retry_after_secs": retry_after_secs,
        })),
    )
        .into_response()
}

/// Mark an in-process relay request with the peer authenticated by the tunnel.
/// Call on EVERY request before sending it to a clone of the shared router.
/// Only use trusted transport metadata, never client-supplied HTTP headers.
pub fn with_peer<B>(request: &mut axum::http::Request<B>, peer: SocketAddr) {
    request.extensions_mut().insert(ConnectInfo(peer));
}

/// Configure every accepted TCP stream before HTTP/WebSocket processing.
/// Socket-option failures are non-fatal; still attempt the remaining options.
pub fn configure_tcp_stream(stream: &mut tokio::net::TcpStream) {
    if let Err(error) = stream.set_nodelay(true) {
        tracing::warn!(%error, "[WEB] Failed to set TCP_NODELAY");
    }
    let keepalive = socket2::TcpKeepalive::new().with_time(Duration::from_secs(60));
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "ios",
        target_os = "visionos",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "windows",
        target_os = "cygwin",
    ))]
    let keepalive = keepalive
        .with_interval(Duration::from_secs(15))
        .with_retries(4);
    if let Err(error) = socket2::SockRef::from(&*stream).set_tcp_keepalive(&keepalive) {
        tracing::warn!(%error, "[WEB] Failed to set TCP keepalive");
    }
}

#[cfg(test)]
mod tests {
    use super::super::auth_limiter::tests::FakeClock;
    use super::*;
    use axum::{body::Body, extract::ConnectInfo, middleware, routing::get, Router};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use tower::ServiceExt;

    struct TestRouter {
        router: Router,
        clock: Arc<FakeClock>,
        _data: tempfile::TempDir,
        _static: tempfile::TempDir,
    }

    impl TestRouter {
        async fn new(token: &str) -> Self {
            let data = tempfile::tempdir().unwrap();
            let static_dir = tempfile::tempdir().unwrap();
            let db = crate::db::test_helpers::fresh_in_memory_db().await;
            let state = Arc::new(crate::app_state::AppState::new_for_test(
                db,
                data.path().into(),
            ));
            let clock = FakeClock::new();
            let router = super::super::router::build_router_with_auth_limiter(
                state,
                token.into(),
                static_dir.path().into(),
                Arc::new(super::super::shutdown::ShutdownSignal::new()),
                clock.limiter(),
            );
            Self {
                router,
                clock,
                _data: data,
                _static: static_dir,
            }
        }

        async fn call(
            &self,
            peer: Option<&str>,
            path: &str,
            header: Option<(&str, &str)>,
        ) -> Response {
            let mut builder = Request::builder()
                .method(if path.starts_with("/ws/") {
                    "GET"
                } else {
                    "POST"
                })
                .uri(path);
            if let Some((name, value)) = header {
                builder = builder.header(name, value);
            }
            let mut request = builder.body(Body::empty()).unwrap();
            if let Some(peer) = peer {
                with_peer(&mut request, peer.parse().unwrap());
            }
            self.router.clone().oneshot(request).await.unwrap()
        }
    }

    #[test]
    fn compare_rejects_mismatches_lengths_and_empty_server_tokens() {
        for (candidate, expected, matches) in [
            ("test-secret", "test-secret", true),
            ("Test-secret", "test-secret", false),
            ("test-secreT", "test-secret", false),
            ("test-secret-longer", "test-secret", false),
            ("test", "test-secret", false),
            ("", "test-secret", false),
            ("test-secret", "", false),
            ("", "", false),
            ("密钥", "密钥", true),
            ("密钥", "密匙", false),
        ] {
            assert_eq!(tokens_match(candidate, expected), matches);
        }
    }

    #[tokio::test]
    async fn router_shares_http_ws_failures_and_isolates_ips_until_ban_expires() {
        let app = TestRouter::new("test-secret").await;
        let peer_a = Some("192.0.2.20:1234");
        for _ in 0..5 {
            assert_eq!(app.call(peer_a, "/api/health", None).await.status(), 401);
        }
        for _ in 0..4 {
            assert_eq!(
                app.call(
                    peer_a,
                    "/ws/events",
                    Some(("sec-websocket-protocol", "codeg-token.aW52YWxpZA"))
                )
                .await
                .status(),
                401
            );
        }
        let response = app.call(peer_a, "/api/health", None).await;
        assert_eq!(response.status(), 429);
        assert_eq!(response.headers()[RETRY_AFTER], "300");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({
                "code": "too_many_auth_failures", "retry_after_secs": 300
            })
        );
        let valid = Some(("authorization", "Bearer test-secret"));
        assert_eq!(
            app.call(Some("192.0.2.20:5678"), "/api/health", valid)
                .await
                .status(),
            429
        );
        assert_eq!(
            app.call(Some("192.0.2.21:1234"), "/api/health", valid)
                .await
                .status(),
            200
        );
        let ws_header = format!(
            "codeg-events, codeg-token.{}",
            URL_SAFE_NO_PAD.encode("test-secret")
        );
        assert_eq!(
            app.call(
                peer_a,
                "/ws/events",
                Some(("sec-websocket-protocol", &ws_header))
            )
            .await
            .status(),
            429
        );
        assert_eq!(
            app.call(peer_a, "/api/get_system_language_settings", None)
                .await
                .status(),
            200
        );
        app.clock.advance(299_001);
        let response = app.call(peer_a, "/api/health", valid).await;
        assert_eq!(response.status(), 429);
        assert_eq!(response.headers()[RETRY_AFTER], "1");
        app.clock.advance(999);
        assert_eq!(app.call(peer_a, "/api/health", valid).await.status(), 200);
        assert_eq!(
            app.call(
                peer_a,
                "/api/health",
                Some(("sec-websocket-protocol", &ws_header))
            )
            .await
            .status(),
            200
        );
    }

    #[tokio::test]
    async fn router_missing_connect_info_uses_unknown_bucket_and_ignores_forwarded_headers() {
        let app = TestRouter::new("test-secret").await;
        for _ in 0..49 {
            assert_eq!(
                app.call(None, "/api/health", Some(("x-forwarded-for", "127.0.0.1")))
                    .await
                    .status(),
                401
            );
        }
        assert_eq!(app.call(None, "/api/health", None).await.status(), 429);
        let valid = Some(("authorization", "Bearer test-secret"));
        assert_eq!(app.call(None, "/api/health", valid).await.status(), 429);
        assert_eq!(
            app.call(Some("192.0.2.22:1234"), "/api/health", valid)
                .await
                .status(),
            200
        );
        app.clock.advance(300_000);
        assert_eq!(app.call(None, "/api/health", valid).await.status(), 200);
    }

    #[tokio::test]
    async fn router_loopback_stays_exempt() {
        let app = TestRouter::new("test-secret").await;
        for peer in ["127.0.0.1:1234", "[::1]:1234", "[::ffff:127.0.0.1]:1234"] {
            for _ in 0..60 {
                assert_eq!(
                    app.call(Some(peer), "/api/health", None).await.status(),
                    401
                );
            }
            assert_eq!(
                app.call(
                    Some(peer),
                    "/api/health",
                    Some(("authorization", "Bearer test-secret"))
                )
                .await
                .status(),
                200
            );
        }
    }

    #[tokio::test]
    async fn router_empty_server_token_always_fails_closed() {
        let app = TestRouter::new("").await;
        for _ in 0..60 {
            assert_eq!(
                app.call(
                    Some("192.0.2.23:1234"),
                    "/api/health",
                    Some(("authorization", "Bearer "))
                )
                .await
                .status(),
                401
            );
            assert_eq!(
                app.call(
                    None,
                    "/api/health",
                    Some(("sec-websocket-protocol", "codeg-token."))
                )
                .await
                .status(),
                401
            );
        }
    }

    #[tokio::test]
    async fn router_success_does_not_clear_recent_failures() {
        let app = TestRouter::new("test-secret").await;
        let peer = Some("192.0.2.24:1234");
        for _ in 0..9 {
            assert_eq!(app.call(peer, "/api/health", None).await.status(), 401);
        }
        assert_eq!(
            app.call(
                peer,
                "/api/health",
                Some(("authorization", "Bearer test-secret"))
            )
            .await
            .status(),
            200
        );
        assert_eq!(app.call(peer, "/api/health", None).await.status(), 429);
    }

    #[tokio::test]
    async fn accepted_tcp_stream_has_options_and_exposes_connect_info() {
        use axum::serve::ListenerExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().route(
            "/peer",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }),
        );
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(
                listener.tap_io(|stream| {
                    configure_tcp_stream(stream);
                    assert!(stream.nodelay().unwrap());
                    let socket = socket2::SockRef::from(&*stream);
                    assert!(socket.keepalive().unwrap());
                    #[cfg(any(target_os = "macos", target_os = "linux"))]
                    {
                        assert_eq!(
                            socket.tcp_keepalive_time().unwrap(),
                            Duration::from_secs(60)
                        );
                        assert_eq!(
                            socket.tcp_keepalive_interval().unwrap(),
                            Duration::from_secs(15)
                        );
                        assert_eq!(socket.tcp_keepalive_retries().unwrap(), 4);
                    }
                }),
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
        });
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        let peer = client.local_addr().unwrap();
        client
            .write_all(b"GET /peer HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with(&peer.to_string()));
        shutdown_tx.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn banned_peer_is_rejected_even_with_valid_token() {
        let router = Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(middleware::from_fn(|req, next| {
                require_token(req, next, "test-secret".into())
            }));
        for _ in 0..10 {
            let request = Request::builder()
                .uri("/protected")
                .extension(ConnectInfo(
                    "192.0.2.1:1234".parse::<std::net::SocketAddr>().unwrap(),
                ))
                .body(Body::empty())
                .unwrap();
            let _ = router.clone().oneshot(request).await.unwrap();
        }
        let request = Request::builder()
            .uri("/protected")
            .header("authorization", "Bearer test-secret")
            .extension(ConnectInfo(
                "192.0.2.1:5678".parse::<std::net::SocketAddr>().unwrap(),
            ))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router.oneshot(request).await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn parses_token_from_ws_protocols() {
        let token = "secret/token+value";
        let encoded = URL_SAFE_NO_PAD.encode(token);
        assert_eq!(
            token_from_ws_protocols(&format!("codeg-events, codeg-token.{encoded}")).as_deref(),
            Some(token)
        );
    }

    #[test]
    fn ignores_invalid_ws_protocol_token() {
        assert!(token_from_ws_protocols("codeg-events, codeg-token.not-valid-@@@@").is_none());
    }
}
