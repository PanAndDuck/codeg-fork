use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;

use crate::web::event_bridge::{emit_event, EventEmitter};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    #[default]
    Disabled,
    WaitingForWebService,
    Connecting,
    Connected,
    Backoff,
    Error,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RelayStatus {
    pub state: RelayState,
    pub url: Option<String>,
    pub connected_since: Option<String>,
    pub last_rtt_ms: Option<u64>,
    pub active_streams: usize,
    pub total_streams: u64,
    pub last_error: Option<String>,
    pub next_retry_in_ms: Option<u64>,
    pub relay_cert_fingerprint: Option<String>,
}

struct Inner {
    status: RelayStatus,
    retry_at: Option<Instant>,
    last_emit: Option<Instant>,
    dirty: bool,
    emitter: EventEmitter,
}

pub struct StatusStore(Mutex<Inner>);

impl Default for StatusStore {
    fn default() -> Self {
        Self(Mutex::new(Inner {
            status: RelayStatus::default(),
            retry_at: None,
            last_emit: None,
            dirty: false,
            emitter: EventEmitter::Noop,
        }))
    }
}

impl StatusStore {
    pub fn set_emitter(&self, emitter: EventEmitter) {
        self.0.lock().unwrap().emitter = emitter;
    }

    pub fn snapshot(&self) -> RelayStatus {
        let inner = self.0.lock().unwrap();
        let mut status = inner.status.clone();
        status.next_retry_in_ms = inner
            .retry_at
            .map(|at| at.saturating_duration_since(Instant::now()).as_millis() as u64);
        status
    }

    pub fn transition(
        &self,
        state: RelayState,
        url: Option<String>,
        error: Option<String>,
        retry: Option<Duration>,
        fingerprint: Option<String>,
    ) {
        let mut inner = self.0.lock().unwrap();
        let changed = inner.status.state != state;
        inner.status.state = state;
        inner.status.url = url;
        inner.status.last_error = error;
        inner.retry_at = retry.map(|v| Instant::now() + v);
        inner.status.next_retry_in_ms = retry.map(|v| v.as_millis() as u64);
        inner.status.relay_cert_fingerprint = fingerprint;
        if state == RelayState::Connected {
            if changed {
                inner.status.connected_since = Some(chrono::Utc::now().to_rfc3339());
            }
        } else {
            inner.status.connected_since = None;
            inner.status.last_rtt_ms = None;
            inner.status.active_streams = 0;
        }
        if changed {
            emit_event(&inner.emitter, "relay://status", &inner.status);
            inner.last_emit = Some(Instant::now());
            inner.dirty = false;
        }
    }

    pub fn counters(&self, active: usize, total: u64, rtt: Option<u64>) {
        let mut inner = self.0.lock().unwrap();
        let changed = inner.status.active_streams != active
            || inner.status.total_streams != total
            || inner.status.last_rtt_ms != rtt;
        inner.status.active_streams = active;
        inner.status.total_streams = total;
        inner.status.last_rtt_ms = rtt;
        inner.dirty |= changed;
        if inner.dirty
            && inner
                .last_emit
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(1))
        {
            emit_event(&inner.emitter, "relay://status", &inner.status);
            inner.last_emit = Some(Instant::now());
            inner.dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::event_bridge::WebEventBroadcaster;
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn transitions_emit_immediately_counters_are_throttled_and_retry_counts_down() {
        let bus = Arc::new(WebEventBroadcaster::new());
        let mut events = bus.subscribe();
        let store = StatusStore::default();
        store.set_emitter(EventEmitter::test_web_only(bus));
        store.transition(RelayState::Connecting, None, None, None, None);
        assert_eq!(events.recv().await.unwrap().payload["state"], "connecting");
        store.transition(RelayState::Connected, None, None, None, None);
        assert!(store.snapshot().connected_since.is_some());
        events.recv().await.unwrap();
        store.counters(1, 1, Some(30));
        assert!(events.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        store.counters(1, 1, Some(30));
        assert_eq!(events.try_recv().unwrap().payload["active_streams"], 1);
        store.transition(
            RelayState::Backoff,
            None,
            Some("connection lost".into()),
            Some(Duration::from_secs(2)),
            None,
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(store.snapshot().next_retry_in_ms, Some(1000));
        assert_eq!(store.snapshot().active_streams, 0);
        assert!(store.snapshot().connected_since.is_none());
    }
}
