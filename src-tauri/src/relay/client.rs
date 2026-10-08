use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::Router;
use codeg_tunnel::mux::{self, MuxConfig, MuxRole};
use rand::Rng;
use tokio::{task::JoinSet, time::Instant};
use tokio_util::sync::CancellationToken;

use super::{
    config::RelayConfig,
    serve::{self, StreamLimits},
    status::{RelayState, StatusStore},
    transport::{self, Observations},
};

#[derive(Default)]
pub struct Backoff {
    failures: usize,
}
impl Backoff {
    pub fn next_delay(&mut self, jitter: f64) -> Duration {
        let seconds = [1, 2, 4, 8, 16, 30][self.failures.min(5)];
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs_f64(seconds as f64 * (1.0 + jitter.clamp(-0.2, 0.2)))
    }
    pub fn connected_for(&mut self, duration: Duration) {
        if duration >= Duration::from_secs(60) {
            self.failures = 0;
        }
    }
}

pub async fn run(
    config: RelayConfig,
    router: Router,
    status: Arc<StatusStore>,
    cancel: CancellationToken,
) {
    let mut backoff = Backoff::default();
    let mut total = status.snapshot().total_streams;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        status.transition(
            RelayState::Connecting,
            Some(config.url.clone()),
            None,
            None,
            None,
        );
        let connection = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            value = transport::connect(&config) => value,
        };
        let (error, auth_rejected, fingerprint) = match connection {
            Err(error) => (
                error.message.to_string(),
                error.code == "auth_rejected",
                error.fingerprint,
            ),
            Ok(connection) => {
                let fingerprint = connection.fingerprint;
                status.transition(
                    RelayState::Connected,
                    Some(config.url.clone()),
                    None,
                    None,
                    fingerprint.clone(),
                );
                let started = Instant::now();
                let observed = Arc::new(Mutex::new(Observations::default()));
                let (handle, mut incoming, mut task) = mux::start(
                    transport::adapter(connection.socket, observed.clone()),
                    MuxConfig {
                        role: MuxRole::Acceptor,
                        ..Default::default()
                    },
                );
                let limits = Arc::new(StreamLimits::default());
                let mut streams = JoinSet::new();
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut joined = false;
                loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break,
                        _ = &mut task => { joined = true; break; }
                        _ = streams.join_next(), if !streams.is_empty() => {},
                        request = incoming.accept() => match request {
                            Some(request) => { streams.spawn(serve::serve(request, router.clone(), limits.clone())); }
                            None => break,
                        },
                        _ = tick.tick() => {
                            let stats = handle.stats();
                            status.counters(stats.active_streams, total.saturating_add(stats.total_streams), observed.lock().unwrap().rtt_ms);
                        }
                    }
                }
                total = total.saturating_add(handle.stats().total_streams);
                handle.close();
                if !joined {
                    let _ = task.await;
                }
                streams.abort_all();
                while streams.join_next().await.is_some() {}
                status.counters(0, total, None);
                if cancel.is_cancelled() {
                    return;
                }
                backoff.connected_for(started.elapsed());
                let replaced = observed.lock().unwrap().replaced;
                if replaced {
                    tracing::warn!("[RELAY] Agent connection replaced (4001)");
                }
                (
                    if replaced {
                        "Relay connection replaced (4001)"
                    } else {
                        "Relay connection closed"
                    }
                    .to_string(),
                    false,
                    fingerprint,
                )
            }
        };
        let delay = if auth_rejected {
            Duration::from_secs(300)
        } else {
            backoff.next_delay(rand::thread_rng().gen_range(-0.2..=0.2))
        };
        status.transition(
            if auth_rejected {
                RelayState::Error
            } else {
                RelayState::Backoff
            },
            Some(config.url.clone()),
            Some(error),
            Some(delay),
            fingerprint,
        );
        tokio::select! { _ = cancel.cancelled() => return, _ = tokio::time::sleep(delay) => {} }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn retries_follow_schedule_and_only_stable_connections_reset_it() {
        let mut backoff = Backoff::default();
        for seconds in [1, 2, 4, 8, 16, 30, 30] {
            let start = Instant::now();
            tokio::time::sleep(backoff.next_delay(0.0)).await;
            assert_eq!(start.elapsed(), Duration::from_secs(seconds));
        }
        backoff.connected_for(Duration::from_secs(59));
        assert_eq!(backoff.next_delay(-0.2), Duration::from_secs(24));
        backoff.connected_for(Duration::from_secs(60));
        assert_eq!(backoff.next_delay(0.2), Duration::from_millis(1200));
    }
}
