use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Extension, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{timeout, Instant, MissedTickBehavior};

use super::event_bridge::WebEvent;
use super::shutdown::ShutdownSignal;
use super::ws_attach::{self, ClientMsg, DetachReason, ServerMsg, OUTBOUND_CAPACITY};
use crate::app_state::AppState;
use crate::logging::throttle::{LagLogThrottle, LAG_LOG_WINDOW};

/// One entry per live attach subscription. The `epoch` is the per-WS-session
/// monotonic counter assigned at spawn time; it threads through the cleanup
/// channel so a stale exit signal from a now-replaced handle cannot
/// accidentally remove the fresh handle, even if re-attach wins the race
/// against processing the previous forwarder's cleanup signal.
struct ActiveSubscription {
    handle: JoinHandle<()>,
    epoch: u64,
}

impl Drop for ActiveSubscription {
    fn drop(&mut self) {
        // Also covers cancellation of the entire connection future.
        self.handle.abort();
    }
}

impl ActiveSubscription {
    async fn stop(mut self) {
        self.handle.abort();
        let _ = (&mut self.handle).await;
    }
}

/// Apply a forwarder self-cleanup signal: remove the handle for `sub_id`
/// only if the stored epoch matches `signal_epoch`. Extracted from the
/// select! branch so the epoch-matching invariant has a unit test.
fn apply_cleanup_signal(
    subscriptions: &mut HashMap<String, ActiveSubscription>,
    sub_id: &str,
    signal_epoch: u64,
) {
    if let Some(sub) = subscriptions.get(sub_id) {
        if sub.epoch == signal_epoch {
            subscriptions.remove(sub_id);
        }
    }
}

// MUST match `WS_READY_CHANNEL` in `src/lib/transport/constants.ts`.
// Drift between the two values silently breaks the handshake (the client
// keeps waiting and falls back to the timeout warning path after 5 s).
//
// Phase 1: kept active in parallel with the new attach protocol so existing
// clients (web / remote desktop) continue to work while transports migrate.
// Phase 4 will retire this channel.
const WS_READY_CHANNEL: &str = "__ready__";

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<AppState>>,
    Extension(shutdown_signal): Extension<Arc<ShutdownSignal>>,
) -> impl IntoResponse {
    ws.protocols([super::auth::WS_EVENT_PROTOCOL])
        .on_upgrade(|socket| handle_ws_connection(socket, state, shutdown_signal))
}

/// Production timing policy, passed explicitly to the transport-independent
/// core so backpressure and liveness tests need no real network or sleeps.
#[derive(Clone, Copy)]
struct WsTimings {
    ping_interval: Duration,
    idle_timeout: Duration,
    write_stall_timeout: Duration,
    attach_enqueue_timeout: Duration,
}

impl Default for WsTimings {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(45),
            write_stall_timeout: Duration::from_secs(30),
            attach_enqueue_timeout: Duration::from_secs(10),
        }
    }
}

enum ConnectionEnd {
    ServerShutdown,
    PeerClosed,
    Aborted,
}

enum WriterClose {
    ServerShutdown(Vec<String>),
    PeerClosed,
}

async fn handle_ws_connection(
    socket: WebSocket,
    state: Arc<AppState>,
    shutdown_signal: Arc<ShutdownSignal>,
) {
    run_connection(socket, state, shutdown_signal, WsTimings::default()).await;
}

async fn run_connection<S, E>(
    socket: S,
    state: Arc<AppState>,
    shutdown_signal: Arc<ShutdownSignal>,
    timings: WsTimings,
) where
    S: Sink<Message> + Stream<Item = Result<Message, E>> + Unpin + Send + 'static,
    S::Error: std::fmt::Display + Send,
{
    let (sink, stream) = socket.split();
    // Subscribe before __ready__: clients may emit as soon as they see it.
    // The global broadcast itself is bounded; the writer drains it directly.
    let global_rx = state.event_broadcaster.subscribe();
    let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let (command_tx, command_rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let (cleanup_tx, cleanup_rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let (close_tx, close_rx) = oneshot::channel();
    let mut subscriptions = HashMap::new();

    // JoinSet aborts the writer even if the upgrade/connection future is
    // externally cancelled. ActiveSubscription provides the same guard for
    // forwarders. On normal exit we also join every aborted task below.
    let mut writer = JoinSet::new();
    writer.spawn(run_writer(
        sink,
        global_rx,
        outbound_rx,
        close_rx,
        timings,
        !shutdown_signal.is_triggered(),
    ));

    let end = tokio::select! {
        biased;
        _ = shutdown_signal.wait() => ConnectionEnd::ServerShutdown,
        _ = writer.join_next() => ConnectionEnd::Aborted,
        end = read_client_messages(stream, command_tx, &outbound_tx, timings) => end,
        _ = process_client_messages(
            command_rx, &state, &outbound_tx, cleanup_tx, cleanup_rx,
            &mut subscriptions, timings,
        ) => ConnectionEnd::Aborted,
    };

    let close = match end {
        ConnectionEnd::ServerShutdown => Some(WriterClose::ServerShutdown(
            subscriptions.keys().cloned().collect(),
        )),
        ConnectionEnd::PeerClosed => Some(WriterClose::PeerClosed),
        ConnectionEnd::Aborted => None,
    };
    // Abort all producers before joining any of them or notifying the writer.
    for sub in subscriptions.values() {
        sub.handle.abort();
    }
    for (_, sub) in subscriptions.drain() {
        sub.stop().await;
    }

    if let Some(close) = close {
        // This one-shot bypasses a saturated data queue. Shutdown notices and
        // Close still have a bounded total grace period, even with many subs.
        let _ = close_tx.send(close);
        let _ = timeout(timings.write_stall_timeout, writer.join_next()).await;
    }
    // On idle, read error or attach timeout, drop the transport
    // promptly. A stalled socket cannot promise a graceful Close handshake.
    writer.abort_all();
    while writer.join_next().await.is_some() {}
}

/// Continuously poll the read half, including while an attach is waiting for
/// queue capacity. tungstenite can therefore drive automatic Pong/Close
/// replies on its read path even when the application writer is pending.
async fn read_client_messages<R, E>(
    mut stream: R,
    commands: mpsc::Sender<ClientMsg>,
    outbound: &mpsc::Sender<ServerMsg>,
    timings: WsTimings,
) -> ConnectionEnd
where
    R: Stream<Item = Result<Message, E>> + Unpin,
{
    loop {
        // Restart on *every* inbound frame, not just a protocol Pong.
        match timeout(timings.idle_timeout, stream.next()).await {
            Ok(Some(Ok(Message::Close(_)))) => return ConnectionEnd::PeerClosed,
            Ok(None) | Ok(Some(Err(_))) => return ConnectionEnd::Aborted,
            Err(_) => {
                tracing::debug!("[WS] inbound idle timeout");
                return ConnectionEnd::Aborted;
            }
            Ok(Some(Ok(Message::Text(text)))) => {
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::Ping) => {
                        // Other outbound traffic already proves liveness.
                        // Dropping a redundant Pong is preferable to blocking
                        // reads (and thus axum's automatic protocol replies).
                        let _ = outbound.try_send(ServerMsg::Pong);
                    }
                    Ok(msg) => {
                        // Bound pending client commands too. A client that
                        // outruns attach processing must reconnect/resync.
                        if commands.try_send(msg).is_err() {
                            tracing::debug!("[WS] client command queue full or closed");
                            return ConnectionEnd::Aborted;
                        }
                    }
                    Err(e) => tracing::warn!("[WS][WARN] malformed client message: {e}"),
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn process_client_messages(
    mut commands: mpsc::Receiver<ClientMsg>,
    state: &Arc<AppState>,
    outbound: &mpsc::Sender<ServerMsg>,
    cleanup_tx: mpsc::Sender<(String, u64)>,
    mut cleanup_rx: mpsc::Receiver<(String, u64)>,
    subscriptions: &mut HashMap<String, ActiveSubscription>,
    timings: WsTimings,
) {
    let mut next_epoch = 0;
    loop {
        tokio::select! {
            Some((sub_id, epoch)) = cleanup_rx.recv() => {
                apply_cleanup_signal(subscriptions, &sub_id, epoch);
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                if handle_client_msg(
                    command, state, outbound, &cleanup_tx, subscriptions,
                    &mut next_epoch, timings.attach_enqueue_timeout,
                ).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn write_frame<S: Sink<Message> + Unpin>(
    sink: &mut S,
    frame: Message,
    timings: WsTimings,
) -> Result<(), ()>
where
    S::Error: std::fmt::Display,
{
    match timeout(timings.write_stall_timeout, sink.send(frame)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            tracing::debug!("[WS] write failed: {e}");
            Err(())
        }
        Err(_) => {
            tracing::debug!("[WS] frame write stalled");
            Err(())
        }
    }
}

async fn run_writer<S: Sink<Message> + Unpin>(
    mut sink: S,
    mut global_rx: broadcast::Receiver<WebEvent>,
    mut outbound_rx: mpsc::Receiver<ServerMsg>,
    mut close_rx: oneshot::Receiver<WriterClose>,
    timings: WsTimings,
    send_ready: bool,
) where
    S::Error: std::fmt::Display,
{
    if send_ready {
        let ready = serde_json::json!({ "channel": WS_READY_CHANNEL, "payload": null });
        if write_frame(&mut sink, Message::Text(ready.to_string().into()), timings)
            .await
            .is_err()
        {
            return;
        }
    }
    let mut heartbeat = tokio::time::interval_at(
        Instant::now() + timings.ping_interval,
        timings.ping_interval,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut lag_throttle = LagLogThrottle::new(LAG_LOG_WINDOW);
    loop {
        let frame = tokio::select! {
            // Control has priority, but the two data sources remain fair.
            biased;
            close = &mut close_rx => {
                match close {
                    Ok(WriterClose::ServerShutdown(subscriptions)) => {
                        for subscription_id in subscriptions {
                            let msg = ServerMsg::Detached {
                                subscription_id,
                                reason: DetachReason::ServerShutdown,
                            };
                            if let Ok(text) = serde_json::to_string(&msg) {
                                if write_frame(&mut sink, Message::Text(text.into()), timings).await.is_err() {
                                    return;
                                }
                            }
                        }
                        let _ = write_frame(&mut sink, Message::Close(None), timings).await;
                    }
                    Ok(WriterClose::PeerClosed) => {
                        // tungstenite already queued the peer's Close reply.
                        // Flush it; sending another Close after receiving one
                        // would instead fail with SendAfterClosing.
                        let _ = timeout(timings.write_stall_timeout, sink.flush()).await;
                    }
                    Err(_) => {}
                }
                return;
            }
            _ = heartbeat.tick() => Message::Ping(Vec::new().into()),
            text = next_data_frame(&mut outbound_rx, &mut global_rx, &mut lag_throttle) => {
                let Some(text) = text else { return };
                Message::Text(text.into())
            }
        };
        if write_frame(&mut sink, frame, timings).await.is_err() {
            return;
        }
    }
}

async fn next_data_frame(
    outbound: &mut mpsc::Receiver<ServerMsg>,
    global: &mut broadcast::Receiver<WebEvent>,
    lag_throttle: &mut LagLogThrottle,
) -> Option<String> {
    loop {
        let serialized = tokio::select! {
            msg = outbound.recv() => serde_json::to_string(&msg?),
            event = global.recv() => {
                match event {
                    Ok(event) => serde_json::to_string(&event),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        if let Some(s) = lag_throttle.record(n) {
                            tracing::warn!(
                                "[WS][WARN] global receiver lagged: skipped {} events across \
                                 {} occurrence(s) in the last {}s",
                                s.dropped, s.occurrences, LAG_LOG_WINDOW.as_secs()
                            );
                        }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        };
        match serialized {
            Ok(text) => return Some(text),
            Err(e) => tracing::warn!("[WS][WARN] failed to serialize outbound frame: {e}"),
        }
    }
}

async fn handle_client_msg(
    msg: ClientMsg,
    state: &Arc<AppState>,
    outbound_tx: &mpsc::Sender<ServerMsg>,
    cleanup_tx: &mpsc::Sender<(String, u64)>,
    subscriptions: &mut HashMap<String, ActiveSubscription>,
    next_epoch: &mut u64,
    enqueue_timeout: Duration,
) -> Result<(), ()> {
    // Bound the entire attach operation, including manager/state lock waits.
    // Only the command processor waits here; the socket reader keeps polling.
    timeout(enqueue_timeout, async {
        match msg {
            ClientMsg::Attach {
                subscription_id,
                connection_id,
                since_seq,
            } => {
                if let Some(old) = subscriptions.remove(&subscription_id) {
                    old.stop().await;
                }
                match ws_attach::handle_attach(
                    &state.connection_manager,
                    state.acp_event_bus.metrics(),
                    subscription_id.clone(),
                    connection_id,
                    since_seq,
                )
                .await
                {
                    Ok(outcome) => {
                        // Both the initial frame and events use the same FIFO.
                        // Spawn only AFTER enqueueing the snapshot/replay, so
                        // even an already-buffered event cannot overtake it.
                        outbound_tx
                            .send(outcome.initial_msg)
                            .await
                            .map_err(|_| ())?;
                        *next_epoch = next_epoch.wrapping_add(1);
                        let epoch = *next_epoch;
                        let handle = ws_attach::spawn_forwarder(
                            subscription_id.clone(),
                            epoch,
                            state.acp_event_bus.metrics().clone(),
                            outcome.receiver,
                            outbound_tx.clone(),
                            cleanup_tx.clone(),
                        );
                        subscriptions.insert(subscription_id, ActiveSubscription { handle, epoch });
                    }
                    Err(reason) => {
                        outbound_tx
                            .send(ServerMsg::Detached {
                                subscription_id,
                                reason,
                            })
                            .await
                            .map_err(|_| ())?;
                    }
                }
            }
            ClientMsg::Detach { subscription_id } => {
                if let Some(sub) = subscriptions.remove(&subscription_id) {
                    sub.stop().await;
                }
            }
            ClientMsg::Ping => {
                let _ = outbound_tx.try_send(ServerMsg::Pong);
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| {
        tracing::debug!("[WS] attach enqueue timeout");
    })?
}

#[cfg(test)]
#[path = "ws_tests.rs"]
mod connection_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backpressure_ping_then_attach_has_a_deadline() {
        let data_dir = tempfile::tempdir().unwrap();
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let state = Arc::new(AppState::new_for_test(db, data_dir.path().to_path_buf()));
        tokio::time::pause();

        // A stalled writer cannot drain this queue. This is the exact
        // state in which the old reader awaited capacity it alone freed.
        let (outbound_tx, _outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
        for _ in 0..OUTBOUND_CAPACITY {
            outbound_tx.try_send(ServerMsg::Pong).unwrap();
        }
        let (cleanup_tx, _cleanup_rx) = mpsc::channel(OUTBOUND_CAPACITY);
        let mut subscriptions = HashMap::new();
        let mut next_epoch = 0;
        let handled = tokio::time::timeout(std::time::Duration::from_secs(11), async {
            let _ = handle_client_msg(
                ClientMsg::Ping,
                &state,
                &outbound_tx,
                &cleanup_tx,
                &mut subscriptions,
                &mut next_epoch,
                Duration::from_secs(10),
            )
            .await;
            let _ = handle_client_msg(
                ClientMsg::Attach {
                    subscription_id: "backpressure".into(),
                    connection_id: "missing".into(),
                    since_seq: None,
                },
                &state,
                &outbound_tx,
                &cleanup_tx,
                &mut subscriptions,
                &mut next_epoch,
                Duration::from_secs(10),
            )
            .await;
        })
        .await;
        assert!(handled.is_ok(), "full outbound queue deadlocked the reader");
    }

    /// Build an ActiveSubscription wrapping a no-op spawned task. The
    /// JoinHandle is real so abort() in cleanup paths is realistic.
    async fn dummy_sub(epoch: u64) -> ActiveSubscription {
        let handle = tokio::spawn(async {
            // Park forever; aborted by cleanup paths or dropped at test end.
            std::future::pending::<()>().await;
        });
        ActiveSubscription { handle, epoch }
    }

    #[tokio::test]
    async fn cleanup_signal_with_matching_epoch_removes_entry() {
        let mut subs = HashMap::new();
        subs.insert("sub-A".to_string(), dummy_sub(1).await);

        apply_cleanup_signal(&mut subs, "sub-A", 1);

        assert!(!subs.contains_key("sub-A"));
    }

    #[tokio::test]
    async fn cleanup_signal_with_stale_epoch_keeps_fresh_handle() {
        // Simulate the race: old forwarder exited with epoch=1; client
        // re-attached and a new forwarder was spawned with epoch=2 in the
        // same map slot. The old cleanup signal must NOT remove the fresh
        // handle, otherwise the new forwarder is orphaned.
        let mut subs = HashMap::new();
        let fresh = dummy_sub(2).await;
        subs.insert("sub-A".to_string(), fresh);

        apply_cleanup_signal(&mut subs, "sub-A", 1);

        assert!(
            subs.contains_key("sub-A"),
            "stale cleanup must not evict the freshly re-attached handle"
        );
        assert_eq!(subs.get("sub-A").unwrap().epoch, 2);
    }

    #[tokio::test]
    async fn cleanup_signal_for_unknown_sub_id_is_noop() {
        let mut subs = HashMap::new();
        subs.insert("other".to_string(), dummy_sub(1).await);

        apply_cleanup_signal(&mut subs, "missing", 1);

        assert!(subs.contains_key("other"));
    }
}
