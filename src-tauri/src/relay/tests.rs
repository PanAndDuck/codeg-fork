use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{extract::ConnectInfo, routing::get, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use codeg_tunnel::{
    frame::CloseCode,
    mux::{self, MuxConfig, MuxHandle, MuxRole, OpenError, TunnelStream},
};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::{
    rustls::{self, pki_types::PrivatePkcs8KeyDer},
    TlsAcceptor,
};
use tokio_tungstenite::tungstenite::{
    self,
    client::IntoClientRequest,
    handshake::server::{Request, Response},
    Message,
};
use tokio_util::sync::CancellationToken;

use super::{
    config::{self, RelayConfig},
    status::RelayState,
    transport::{self, Observations},
    RelayManager,
};
use crate::{
    app_state::AppState,
    db::test_helpers::fresh_in_memory_db,
    web::{event_bridge::EventEmitter, shutdown::ShutdownSignal},
};

const AGENT_KEY: &str = "test-agent-key-123456789";
const TOKEN: &str = "test-web-token-123456789";
static GLOBAL_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Certificate {
    acceptor: TlsAcceptor,
    fingerprint: String,
}
impl Certificate {
    fn new() -> Self {
        let certified =
            rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
                .unwrap();
        let fingerprint = transport::certificate_fingerprint(certified.cert.der());
        let key = PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![certified.cert.der().clone()], key.into())
            .unwrap();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        Self {
            acceptor: TlsAcceptor::from(Arc::new(tls)),
            fingerprint,
        }
    }
}

struct FakeRelay {
    address: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<()>,
    connections: mpsc::UnboundedReceiver<MuxHandle>,
    upgrades: Arc<AtomicUsize>,
}

impl FakeRelay {
    async fn start(cert: &Certificate, address: SocketAddr, reject_auth: bool) -> Self {
        let listener = TcpListener::bind(address).await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = cert.acceptor.clone();
        let cancel = CancellationToken::new();
        let shutdown = cancel.clone();
        let upgrades = Arc::new(AtomicUsize::new(0));
        let counted = upgrades.clone();
        let (tx, connections) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = sessions.join_next(), if !sessions.is_empty() => {},
                    accepted = listener.accept() => {
                        let (tcp, _) = accepted.unwrap();
                        let acceptor = acceptor.clone();
                        let tx = tx.clone();
                        let counted = counted.clone();
                        let cancel = shutdown.clone();
                        sessions.spawn(async move {
                            let handshake = async {
                                let tls = acceptor.accept(tcp).await.ok()?;
                                tokio_tungstenite::accept_hdr_async(tls, move |request: &Request, mut response: Response| {
                                    counted.fetch_add(1, Ordering::SeqCst);
                                    assert_eq!(request.uri().path(), config::AGENT_PATH);
                                    assert_eq!(request.headers()["Sec-WebSocket-Protocol"], transport::PROTOCOL);
                                    assert!(request.headers()["User-Agent"].to_str().unwrap().starts_with("codeg/"));
                                    if reject_auth || request.headers().get("Authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {AGENT_KEY}")) {
                                        return Err(http::Response::builder().status(401).body(Some("denied".into())).unwrap());
                                    }
                                    response.headers_mut().insert("Sec-WebSocket-Protocol", transport::PROTOCOL.parse().unwrap());
                                    Ok(response)
                                }).await.ok()
                            };
                            let socket = tokio::select! { _ = cancel.cancelled() => return, socket = handshake => socket };
                            let Some(socket) = socket else { return; };
                            let (handle, _incoming, mut task) = mux::start(transport::adapter(socket, Arc::new(Mutex::new(Observations::default()))), MuxConfig { role: MuxRole::Opener, ..Default::default() });
                            let _ = tx.send(handle.clone());
                            tokio::select! {
                                _ = cancel.cancelled() => { handle.close(); let _ = task.await; }
                                _ = &mut task => {},
                            }
                        });
                    }
                }
            }
            drop(listener);
            while sessions.join_next().await.is_some() {}
        });
        Self {
            address,
            cancel,
            task,
            connections,
            upgrades,
        }
    }

    fn config(&self, cert: &Certificate) -> RelayConfig {
        RelayConfig {
            enabled: true,
            url: format!("https://{}", self.address),
            key: AGENT_KEY.into(),
            fingerprint: Some(cert.fingerprint.clone()),
            ..Default::default()
        }
    }

    async fn connected(&mut self) -> MuxHandle {
        tokio::time::timeout(Duration::from_secs(12), self.connections.recv())
            .await
            .expect("agent connected before deadline")
            .unwrap()
    }

    async fn stop(self) {
        self.cancel.cancel();
        self.task.await.unwrap();
    }
}

async fn fixture() -> (Arc<AppState>, Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(AppState::new_for_test(
        fresh_in_memory_db().await,
        dir.path().to_owned(),
    ));
    let router = crate::web::router::build_router(
        state.clone(),
        TOKEN.into(),
        dir.path().to_owned(),
        Arc::new(ShutdownSignal::new()),
    )
    .route(
        "/peer",
        get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }),
    );
    (state, router, dir)
}

async fn response(stream: &mut BufReader<TunnelStream>, request: &str) -> (u16, String) {
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        let status = line
            .split_whitespace()
            .nth(1)
            .expect("HTTP status")
            .parse()
            .unwrap();
        let mut size = 0;
        loop {
            line.clear();
            assert!(stream.read_line(&mut line).await.unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                size = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; size];
        stream.read_exact(&mut body).await.unwrap();
        (status, String::from_utf8(body).unwrap())
    })
    .await
    .unwrap()
}

fn health_request() -> String {
    format!("POST /api/health HTTP/1.1\r\nHost: codeg.test\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 0\r\n\r\n")
}

async fn wait_state(manager: &RelayManager, expected: RelayState) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while manager.status.snapshot().state != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {expected:?}, got {:?}", manager.status.snapshot()));
}

#[tokio::test]
async fn tls_tunnel_serves_real_health_websocket_keepalive_peer_and_reconnects() {
    let cert = Certificate::new();
    let mut relay = FakeRelay::start(&cert, "127.0.0.1:0".parse().unwrap(), false).await;
    let (state, router, _dir) = fixture().await;
    config::save(&state.db.conn, &relay.config(&cert))
        .await
        .unwrap();
    let manager = Arc::new(RelayManager::default());
    manager
        .web_started(&state.db.conn, state.emitter.clone(), router.clone())
        .await;
    let handle = relay.connected().await;
    wait_state(&manager, RelayState::Connected).await;
    let before = relay.upgrades.load(Ordering::SeqCst);
    assert!(
        manager
            .test_existing(&relay.config(&cert))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(relay.upgrades.load(Ordering::SeqCst), before);
    let mut stream = BufReader::new(handle.open("198.51.100.42", 51000).await.unwrap());
    let (status, body) = response(&mut stream, &health_request()).await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["status"],
        "ok"
    );
    for _ in 0..2 {
        let (status, body) = response(
            &mut stream,
            "GET /peer HTTP/1.1\r\nHost: codeg.test\r\nX-Forwarded-For: 127.0.0.1\r\n\r\n",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "198.51.100.42:51000");
    }
    assert!(matches!(
        handle.open("not-an-ip.test", 51000).await,
        Err(OpenError::Rejected {
            code: CloseCode::BadRequest,
            ..
        })
    ));

    let ws_stream = handle.open("198.51.100.43", 51001).await.unwrap();
    let mut request = "ws://codeg.test/ws/events".into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!(
            "codeg-events, codeg-token.{}",
            URL_SAFE_NO_PAD.encode(TOKEN)
        )
        .parse()
        .unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::client_async(request, ws_stream)
        .await
        .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(ready.to_text().unwrap()).unwrap()["channel"],
        "__ready__"
    );
    state
        .event_broadcaster
        .send("relay-test", &serde_json::json!({"ok":true}));
    let event = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(event.to_text().unwrap()).unwrap()["channel"],
        "relay-test"
    );
    ws.close(None).await.unwrap();
    drop(ws);
    drop(stream);

    let address = relay.address;
    relay.stop().await;
    wait_state(&manager, RelayState::Backoff).await;
    let mut restarted = FakeRelay::start(&cert, address, false).await;
    let handle = restarted.connected().await;
    let mut stream = BufReader::new(handle.open("198.51.100.42", 51000).await.unwrap());
    assert_eq!(response(&mut stream, &health_request()).await.0, 200);
    drop(stream);
    manager.web_stopped().await;
    assert_eq!(
        manager.status.snapshot().state,
        RelayState::WaitingForWebService
    );
    assert!(!handle.stats().connected || handle.open("198.51.100.42", 51000).await.is_err());
    manager
        .update(
            &state.db.conn,
            state.emitter.clone(),
            config::RelayConfigUpdate {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(manager.status.snapshot().state, RelayState::Disabled);
    restarted.stop().await;
}

#[tokio::test]
async fn trust_errors_return_presented_fingerprint_without_sending_credentials() {
    let cert = Certificate::new();
    let relay = FakeRelay::start(&cert, "127.0.0.1:0".parse().unwrap(), false).await;
    let db = fresh_in_memory_db().await;
    config::save(&db.conn, &relay.config(&cert)).await.unwrap();
    for (pin, code) in [
        (Some("00".repeat(32)), "fingerprint_mismatch"),
        (None, "tls_untrusted"),
    ] {
        let result = super::commands::test_relay_connection_core(
            &db.conn,
            super::commands::RelayTestConfig {
                url: format!("https://{}", relay.address),
                key: None,
                fingerprint: pin,
            },
        )
        .await
        .unwrap();
        assert!(!result.ok);
        assert_eq!(result.error_code, Some(code));
        assert_eq!(
            result.fingerprint.as_deref(),
            Some(cert.fingerprint.as_str())
        );
        assert_eq!(relay.upgrades.load(Ordering::SeqCst), 0);
    }
    let result = super::commands::test_relay_connection_core(
        &db.conn,
        super::commands::RelayTestConfig {
            url: format!("https://{}", relay.address),
            key: None,
            fingerprint: Some(cert.fingerprint.clone()),
        },
    )
    .await
    .unwrap();
    assert!(result.ok);
    assert_eq!(
        result.fingerprint.as_deref(),
        Some(cert.fingerprint.as_str())
    );
    relay.stop().await;
}

#[tokio::test]
async fn unauthorized_agent_enters_error_and_waits_five_minutes() {
    let cert = Certificate::new();
    let relay = FakeRelay::start(&cert, "127.0.0.1:0".parse().unwrap(), true).await;
    let db = fresh_in_memory_db().await;
    let config = relay.config(&cert);
    let error = transport::connect(&config).await.err().unwrap();
    assert_eq!(error.code, "auth_rejected");
    config::save(&db.conn, &config).await.unwrap();
    let manager = RelayManager::default();
    manager
        .web_started(&db.conn, EventEmitter::Noop, Router::new())
        .await;
    wait_state(&manager, RelayState::Error).await;
    let status = manager.status.snapshot();
    assert!(status.next_retry_in_ms.unwrap() > 299_000);
    assert!(!status.last_error.unwrap().contains(AGENT_KEY));
    let before = relay.upgrades.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(relay.upgrades.load(Ordering::SeqCst), before);
    manager.web_stopped().await;
    relay.stop().await;
}

#[tokio::test]
async fn upgraded_stream_keeps_ip_slot_until_close() {
    let cert = Certificate::new();
    let mut relay = FakeRelay::start(&cert, "127.0.0.1:0".parse().unwrap(), false).await;
    let (state, router, _dir) = fixture().await;
    config::save(&state.db.conn, &relay.config(&cert))
        .await
        .unwrap();
    let manager = RelayManager::default();
    manager
        .web_started(&state.db.conn, state.emitter.clone(), router)
        .await;
    let handle = relay.connected().await;
    let mut streams = Vec::new();
    for port in 1..=64 {
        streams.push(handle.open("198.51.100.9", port).await.unwrap());
    }
    assert!(matches!(
        handle.open("198.51.100.9", 65).await,
        Err(OpenError::Rejected {
            code: CloseCode::NotAllowed,
            ..
        })
    ));
    let stream = streams.pop().unwrap();
    let mut request = "ws://codeg.test/ws/events".into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {TOKEN}").parse().unwrap());
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "codeg-events".parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .unwrap();
    assert!(ws.next().await.unwrap().unwrap().is_text());
    assert!(matches!(
        handle.open("198.51.100.9", 65).await,
        Err(OpenError::Rejected {
            code: CloseCode::NotAllowed,
            ..
        })
    ));
    drop(ws);
    drop(streams);
    manager.web_stopped().await;
    relay.stop().await;
}

#[tokio::test]
async fn adapter_records_matching_pong_rtt_and_replacement_close() {
    let (client, server) = tokio::io::duplex(4096);
    let client = tokio_tungstenite::WebSocketStream::from_raw_socket(
        client,
        tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
        server,
        tungstenite::protocol::Role::Server,
        None,
    )
    .await;
    let observations = Arc::new(Mutex::new(Observations::default()));
    let mut adapter = Box::pin(transport::adapter(client, observations.clone()));
    adapter
        .send(mux::MuxMessage::Ping(b"probe".to_vec()))
        .await
        .unwrap();
    assert!(server.next().await.unwrap().unwrap().is_ping());
    server
        .send(Message::Pong(b"probe".to_vec().into()))
        .await
        .unwrap();
    assert!(matches!(
        adapter.next().await.unwrap().unwrap(),
        mux::MuxMessage::Pong(_)
    ));
    assert!(observations.lock().unwrap().rtt_ms.is_some());
    server
        .send(Message::Close(Some(tungstenite::protocol::CloseFrame {
            code: 4001.into(),
            reason: "replaced".into(),
        })))
        .await
        .unwrap();
    assert!(matches!(
        adapter.next().await.unwrap().unwrap(),
        mux::MuxMessage::Close
    ));
    assert!(observations.lock().unwrap().replaced);
}

#[tokio::test]
async fn relay_web_commands_share_named_shape_and_clear_nulls() {
    let _lock = GLOBAL_TEST_LOCK.lock().await;
    use tower::ServiceExt;
    let (state, router, _dir) = fixture().await;
    config::save(
        &state.db.conn,
        &RelayConfig {
            fingerprint: Some("ab".repeat(32)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for (body, expected) in [
        (r#"{"config":{}}"#, Some("ab".repeat(32))),
        (r#"{"config":{"fingerprint":null}}"#, None),
    ] {
        let request = http::Request::post("/api/update_relay_config")
            .header("Authorization", format!("Bearer {TOKEN}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["fingerprint"].as_str(), expected.as_deref());
        assert!(value.get("key").is_none());
    }
    for command in [
        "get_relay_config",
        "get_relay_status",
        "test_relay_connection",
    ] {
        let request = http::Request::post(format!("/api/{command}"))
            .header("Authorization", format!("Bearer {TOKEN}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_ne!(response.status(), 404);
    }
}

#[tokio::test]
async fn failed_tls_without_certificate_is_protocol_not_a_tofu_prompt() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut tcp, _) = listener.accept().await.unwrap();
            tcp.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        }
    });
    let config = RelayConfig {
        url: format!("https://{address}"),
        key: AGENT_KEY.into(),
        ..Default::default()
    };
    let error = transport::connect(&config).await.err().unwrap();
    assert_eq!(error.code, "protocol");
    assert!(error.fingerprint.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn stop_through_tunnel_completes_even_when_its_request_is_cancelled() {
    let _lock = GLOBAL_TEST_LOCK.lock().await;
    let cert = Certificate::new();
    let mut relay = FakeRelay::start(&cert, "127.0.0.1:0".parse().unwrap(), false).await;
    let (state, _router, dir) = fixture().await;
    config::save(&state.db.conn, &relay.config(&cert))
        .await
        .unwrap();
    let web = crate::web::do_start_web_server_with_state(
        state.clone(),
        dir.path().into(),
        Some(0),
        Some("127.0.0.1".into()),
        Some(TOKEN.into()),
    )
    .await
    .unwrap();
    let handle = relay.connected().await;
    let mut stream = handle.open("198.51.100.42", 51000).await.unwrap();
    stream.write_all(format!("POST /api/stop_web_server HTTP/1.1\r\nHost: codeg.test\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while crate::web::do_get_web_server_status(&state.web_server_state).is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        super::MANAGER.status.snapshot().state,
        RelayState::WaitingForWebService
    );
    let rebound = TcpListener::bind(("127.0.0.1", web.port)).await.unwrap();
    drop(rebound);
    relay.stop().await;
}

#[tokio::test]
async fn saving_config_restarts_then_disable_cancels_pending_retry() {
    let cert = Certificate::new();
    let mut relay = FakeRelay::start(&cert, "127.0.0.1:0".parse().unwrap(), false).await;
    let db = fresh_in_memory_db().await;
    config::save(&db.conn, &relay.config(&cert)).await.unwrap();
    let manager = Arc::new(RelayManager::default());
    manager
        .web_started(&db.conn, EventEmitter::Noop, Router::new())
        .await;
    relay.connected().await;
    manager
        .update(
            &db.conn,
            EventEmitter::Noop,
            config::RelayConfigUpdate {
                public_url: Some(Some("https://public.test".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    relay.connected().await;
    manager
        .update(
            &db.conn,
            EventEmitter::Noop,
            config::RelayConfigUpdate {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    wait_state(&manager, RelayState::Disabled).await;
    assert_eq!(manager.status.snapshot().active_streams, 0);
    manager.web_stopped().await;
    relay.stop().await;
}

// Opt-in fixture for scripts/fork-server-dev.sh + curl. It publishes only
// generated test credentials in this worktree's ignored .codex directory.
#[tokio::test]
#[ignore = "interactive local fork-server smoke fixture"]
async fn manual_fake_relay() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".codex");
    std::fs::create_dir_all(&root).unwrap();
    let stop = root.join("manual-relay.stop");
    let _ = std::fs::remove_file(&stop);
    let cert = Certificate::new();
    let mut relay = FakeRelay::start(&cert, "127.0.0.1:0".parse().unwrap(), false).await;
    let public = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let public_address = public.local_addr().unwrap();
    std::fs::write(
        root.join("manual-relay.json"),
        serde_json::json!({
            "url": format!("https://{}", relay.address), "key": AGENT_KEY,
            "fingerprint": cert.fingerprint, "public_url": format!("http://{public_address}")
        })
        .to_string(),
    )
    .unwrap();
    let mut connections = JoinSet::new();
    let deadline = tokio::time::sleep(Duration::from_secs(900));
    tokio::pin!(deadline);
    let mut handle: Option<MuxHandle> = None;
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            _ = tokio::time::sleep(Duration::from_millis(100)) => { if stop.exists() { break; } },
            next = relay.connections.recv() => { handle = next; },
            _ = connections.join_next(), if !connections.is_empty() => {},
            client = public.accept() => {
                let (mut tcp, peer) = client.unwrap();
                if let Some(handle) = handle.clone() {
                    connections.spawn(async move {
                        if let Ok(mut stream) = handle.open(&peer.ip().to_string(), peer.port()).await {
                            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                        }
                    });
                }
            }
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    relay.stop().await;
    let _ = std::fs::remove_file(root.join("manual-relay.json"));
}
