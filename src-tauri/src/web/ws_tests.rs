use super::*;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

use crate::acp::types::AcpEvent;
use crate::models::agent::AgentType;
use crate::web::event_bridge::emit_with_state;
use futures::task::AtomicWaker;
use serde_json::{json, Value};

#[derive(Default)]
struct SocketControl {
    stalled: AtomicBool,
    blocked: AtomicBool,
    dropped: AtomicBool,
    reads: AtomicUsize,
    writer_waker: AtomicWaker,
}

struct MemorySocket {
    incoming: mpsc::Receiver<Message>,
    outgoing: mpsc::Sender<Message>,
    control: Arc<SocketControl>,
}

impl Drop for MemorySocket {
    fn drop(&mut self) {
        self.control.dropped.store(true, Ordering::SeqCst);
    }
}

impl Stream for MemorySocket {
    type Item = Result<Message, &'static str>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming.poll_recv(cx).map(|msg| {
            msg.map(|msg| {
                self.control.reads.fetch_add(1, Ordering::SeqCst);
                Ok(msg)
            })
        })
    }
}

impl Sink<Message> for MemorySocket {
    type Error = &'static str;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        self.outgoing.try_send(item).map_err(|_| "test output full")
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.control.writer_waker.register(cx.waker());
        if self.control.stalled.load(Ordering::SeqCst) {
            self.control.blocked.store(true, Ordering::SeqCst);
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

struct Peer {
    incoming: mpsc::Sender<Message>,
    outgoing: mpsc::Receiver<Message>,
    control: Arc<SocketControl>,
    task: JoinHandle<()>,
}

impl Peer {
    fn start(state: Arc<AppState>, shutdown: Arc<ShutdownSignal>, timings: WsTimings) -> Self {
        let (incoming, rx) = mpsc::channel(OUTBOUND_CAPACITY * 2);
        let (tx, outgoing) = mpsc::channel(OUTBOUND_CAPACITY * 2);
        let control = Arc::new(SocketControl::default());
        let socket = MemorySocket {
            incoming: rx,
            outgoing: tx,
            control: control.clone(),
        };
        let task = tokio::spawn(run_connection(socket, state, shutdown, timings));
        Self {
            incoming,
            outgoing,
            control,
            task,
        }
    }

    async fn send(&self, value: Value) {
        self.incoming
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }

    async fn recv(&mut self) -> Message {
        tokio::time::timeout(Duration::from_secs(1), self.outgoing.recv())
            .await
            .expect("frame deadline")
            .expect("socket open")
    }

    async fn json(&mut self) -> Value {
        let Message::Text(text) = self.recv().await else {
            panic!("expected text frame");
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn closed(&mut self) {
        tokio::time::timeout(Duration::from_secs(1), &mut self.task)
            .await
            .expect("connection must terminate")
            .unwrap();
        assert!(self.control.dropped.load(Ordering::SeqCst));
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn state() -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::test_helpers::fresh_in_memory_db().await;
    let state = Arc::new(AppState::new_for_test(db, dir.path().to_path_buf()));
    state
        .connection_manager
        .insert_test_connection("conn", AgentType::ClaudeCode, None, state.emitter.clone())
        .await;
    (state, dir)
}

async fn until(mut predicate: impl FnMut() -> bool) {
    for _ in 0..100 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    assert!(predicate(), "condition did not become true");
}

async fn attach(peer: &Peer, sub: &str) {
    peer.send(json!({
        "action": "attach", "subscription_id": sub, "connection_id": "conn"
    }))
    .await;
}

#[tokio::test]
async fn stalled_writer_full_queue_keeps_reading_ping_and_attach_then_closes() {
    let (state, _dir) = state().await;
    tokio::time::pause();
    let mut peer = Peer::start(
        state.clone(),
        Arc::new(ShutdownSignal::new()),
        WsTimings::default(),
    );
    assert_eq!(
        peer.json().await,
        json!({"channel":"__ready__","payload":null})
    );
    peer.control.stalled.store(true, Ordering::SeqCst);
    peer.send(json!({"action": "ping"})).await;
    until(|| peer.control.blocked.load(Ordering::SeqCst)).await;
    // One Pong is in the stalled send; all 64 remaining slots now fill.
    for _ in 0..OUTBOUND_CAPACITY {
        peer.send(json!({"action": "ping"})).await;
    }
    until(|| peer.control.reads.load(Ordering::SeqCst) == OUTBOUND_CAPACITY + 1).await;
    peer.send(json!({"action": "ping"})).await;
    attach(&peer, "blocked").await;
    peer.incoming
        .send(Message::Ping(vec![1].into()))
        .await
        .unwrap();
    until(|| peer.control.reads.load(Ordering::SeqCst) == OUTBOUND_CAPACITY + 4).await;
    until(|| {
        state
            .acp_event_bus
            .metrics()
            .snapshot_cold_count
            .load(Ordering::Relaxed)
            == 1
    })
    .await;
    tokio::time::advance(Duration::from_secs(10)).await;
    peer.closed().await;
}

#[tokio::test]
async fn silent_client_receives_heartbeat_and_closes_at_idle_deadline() {
    let (state, _dir) = state().await;
    tokio::time::pause();
    let mut peer = Peer::start(state, Arc::new(ShutdownSignal::new()), WsTimings::default());
    peer.json().await;
    for _ in 0..2 {
        tokio::time::advance(Duration::from_secs(15)).await;
        assert!(matches!(peer.recv().await, Message::Ping(_)));
    }
    assert!(!peer.task.is_finished());
    tokio::time::advance(Duration::from_secs(15)).await;
    peer.closed().await;
}

#[tokio::test]
async fn pong_only_client_stays_alive() {
    let (state, _dir) = state().await;
    tokio::time::pause();
    let mut peer = Peer::start(state, Arc::new(ShutdownSignal::new()), WsTimings::default());
    peer.json().await;
    for count in 1..=5 {
        tokio::time::advance(Duration::from_secs(15)).await;
        let Message::Ping(payload) = peer.recv().await else {
            panic!("expected heartbeat")
        };
        peer.incoming.send(Message::Pong(payload)).await.unwrap();
        until(|| peer.control.reads.load(Ordering::SeqCst) == count).await;
        assert!(!peer.task.is_finished());
    }
    peer.incoming.send(Message::Close(None)).await.unwrap();
    peer.closed().await;
}

#[tokio::test]
async fn stalled_ready_frame_times_out_and_drops_both_socket_halves() {
    let (state, _dir) = state().await;
    tokio::time::pause();
    let mut peer = Peer::start(state, Arc::new(ShutdownSignal::new()), WsTimings::default());
    peer.control.stalled.store(true, Ordering::SeqCst);
    until(|| peer.control.blocked.load(Ordering::SeqCst)).await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!peer.task.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    peer.closed().await;
}

#[tokio::test]
async fn snapshot_precedes_live_event_even_when_writer_is_stalled() {
    let (state, _dir) = state().await;
    let metrics = state.acp_event_bus.metrics().clone();
    let metric_owners = Arc::strong_count(&metrics);
    tokio::time::pause();
    let mut peer = Peer::start(
        state.clone(),
        Arc::new(ShutdownSignal::new()),
        WsTimings::default(),
    );
    peer.json().await;
    peer.control.stalled.store(true, Ordering::SeqCst);
    attach(&peer, "ordered").await;
    until(|| peer.control.blocked.load(Ordering::SeqCst)).await;
    let session = state.connection_manager.get_state("conn").await.unwrap();
    emit_with_state(
        &session,
        &state.emitter,
        AcpEvent::ContentDelta {
            text: "after-snapshot".into(),
            parent_tool_use_id: None,
        },
    )
    .await;
    peer.control.stalled.store(false, Ordering::SeqCst);
    peer.control.writer_waker.wake();
    let snapshot = peer.json().await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["event_seq"], 0);
    let event = peer.json().await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["subscription_id"], "ordered");
    assert_eq!(event["envelope"]["seq"], 1);
    peer.incoming.send(Message::Close(None)).await.unwrap();
    peer.closed().await;
    assert_eq!(
        Arc::strong_count(&metrics),
        metric_owners,
        "forwarder must release its captured state on close"
    );
}

#[tokio::test]
async fn shutdown_sends_detached_then_close_and_legacy_shape_is_unchanged() {
    let (state, _dir) = state().await;
    tokio::time::pause();
    let shutdown = Arc::new(ShutdownSignal::new());
    let mut peer = Peer::start(state.clone(), shutdown.clone(), WsTimings::default());
    peer.json().await;
    attach(&peer, "shutdown").await;
    assert_eq!(peer.json().await["type"], "snapshot");
    state
        .event_broadcaster
        .send("test://legacy", &json!({"value":1}));
    assert_eq!(
        peer.json().await,
        json!({"channel":"test://legacy","payload":{"value":1}})
    );
    shutdown.trigger();
    assert_eq!(
        peer.json().await,
        json!({"type":"detached","subscription_id":"shutdown","reason":"server_shutdown"})
    );
    assert!(matches!(peer.recv().await, Message::Close(_)));
    peer.closed().await;
}

#[tokio::test]
async fn shutdown_during_stalled_write_is_bounded_and_reaps_forwarders() {
    let (state, _dir) = state().await;
    let metrics = state.acp_event_bus.metrics().clone();
    let metric_owners = Arc::strong_count(&metrics);
    tokio::time::pause();
    let shutdown = Arc::new(ShutdownSignal::new());
    let mut peer = Peer::start(state.clone(), shutdown.clone(), WsTimings::default());
    peer.json().await;
    attach(&peer, "stalled-shutdown").await;
    peer.json().await;
    peer.control.stalled.store(true, Ordering::SeqCst);
    peer.send(json!({"action": "ping"})).await;
    until(|| peer.control.blocked.load(Ordering::SeqCst)).await;
    shutdown.trigger();
    tokio::time::advance(Duration::from_secs(30)).await;
    peer.closed().await;
    assert_eq!(Arc::strong_count(&metrics), metric_owners);
}

#[tokio::test]
async fn cancelling_connection_aborts_writer_and_forwarders() {
    let (state, _dir) = state().await;
    let metrics = state.acp_event_bus.metrics().clone();
    let metric_owners = Arc::strong_count(&metrics);
    let mut peer = Peer::start(
        state.clone(),
        Arc::new(ShutdownSignal::new()),
        WsTimings::default(),
    );
    peer.json().await;
    attach(&peer, "cancelled").await;
    peer.json().await;
    peer.task.abort();
    assert!((&mut peer.task).await.unwrap_err().is_cancelled());
    until(|| peer.control.dropped.load(Ordering::SeqCst)).await;
    until(|| Arc::strong_count(&metrics) == metric_owners).await;
}

#[tokio::test]
async fn late_shutdown_closes_without_ready() {
    let (state, _dir) = state().await;
    let shutdown = Arc::new(ShutdownSignal::new());
    shutdown.trigger();
    let mut peer = Peer::start(state, shutdown, WsTimings::default());
    assert!(matches!(peer.recv().await, Message::Close(_)));
    peer.closed().await;
}

#[tokio::test]
async fn binary_frames_reset_injected_idle_timeout() {
    let (state, _dir) = state().await;
    tokio::time::pause();
    let timings = WsTimings {
        ping_interval: Duration::from_secs(60),
        idle_timeout: Duration::from_secs(3),
        ..WsTimings::default()
    };
    let mut peer = Peer::start(state, Arc::new(ShutdownSignal::new()), timings);
    peer.json().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    peer.incoming
        .send(Message::Binary(vec![1].into()))
        .await
        .unwrap();
    until(|| peer.control.reads.load(Ordering::SeqCst) == 1).await;
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(!peer.task.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    peer.closed().await;
}

#[tokio::test]
async fn axum_automatically_answers_protocol_ping_and_close() {
    use tokio_tungstenite::tungstenite::Message as WireMessage;

    let (state, _dir) = state().await;
    let app = axum::Router::new()
        .route("/ws", axum::routing::get(ws_handler))
        .layer(Extension(state))
        .layer(Extension(Arc::new(ShutdownSignal::new())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut server = JoinSet::new();
    server.spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .unwrap();
    let ready = timeout(Duration::from_secs(1), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ready: Value = serde_json::from_str(ready.to_text().unwrap()).unwrap();
    assert_eq!(ready["channel"], "__ready__");
    ws.send(WireMessage::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();
    let pong = timeout(Duration::from_secs(1), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(pong, WireMessage::Pong(vec![1, 2, 3].into()));
    ws.send(WireMessage::Close(None)).await.unwrap();
    let close = timeout(Duration::from_secs(1), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(close, WireMessage::Close(_)));
    drop(ws);
    server.abort_all();
    while server.join_next().await.is_some() {}
}

#[tokio::test]
async fn lagged_global_firehose_continues_with_remaining_event() {
    let (_outbound_tx, mut outbound_rx) = mpsc::channel(1);
    let (global_tx, mut global_rx) = broadcast::channel(1);
    for value in 0..3 {
        global_tx
            .send(WebEvent {
                channel: "legacy".into(),
                payload: Arc::new(json!(value)),
            })
            .unwrap();
    }
    let mut throttle = LagLogThrottle::new(LAG_LOG_WINDOW);
    let text = next_data_frame(&mut outbound_rx, &mut global_rx, &mut throttle)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({"channel":"legacy","payload":2})
    );
}
