//! Sliding-window authentication failures, shared by HTTP and WebSocket auth.
//!
//! A ban rejects even valid credentials for five minutes. Successful requests
//! do not reset failures, allocate entries, or take the write lock. Loopback
//! skips locking entirely. At capacity the least recently failing IP is evicted
//! (including an active ban); the single unknown-peer bucket is never evicted.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const BAN_DURATION: Duration = Duration::from_secs(300);
const IP_THRESHOLD: usize = 10;
const UNKNOWN_THRESHOLD: usize = 50;
const MAX_IPS: usize = 10_000;

#[derive(Default)]
struct Entry {
    failures: VecDeque<Instant>,
    banned_until: Option<Instant>,
}

#[derive(Default)]
struct LimiterState {
    entries: HashMap<Option<IpAddr>, Entry>,
    oldest: BTreeSet<(Instant, IpAddr)>,
}

pub struct AuthFailureLimiter {
    state: RwLock<LimiterState>,
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl Default for AuthFailureLimiter {
    fn default() -> Self {
        Self::with_clock(Arc::new(Instant::now))
    }
}

impl AuthFailureLimiter {
    fn with_clock(clock: Arc<dyn Fn() -> Instant + Send + Sync>) -> Self {
        Self {
            state: RwLock::new(LimiterState::default()),
            clock,
        }
    }

    /// Remaining ban time, rounded up for the HTTP Retry-After header.
    pub fn retry_after(&self, peer: Option<IpAddr>) -> Option<u64> {
        let peer = peer.map(|ip| ip.to_canonical());
        if peer.is_some_and(|ip| ip.is_loopback()) {
            return None;
        }
        let state = self.state.read().unwrap();
        let until = state.entries.get(&peer)?.banned_until?;
        remaining_secs(until, (self.clock)())
    }

    /// Record one failed request, returning the ban triggered by this failure.
    /// Requests during an existing ban never extend it or emit another warning.
    pub fn record_failure(&self, peer: Option<IpAddr>) -> Option<u64> {
        let peer = peer.map(|ip| ip.to_canonical());
        if peer.is_some_and(|ip| ip.is_loopback()) {
            return None;
        }
        let mut state = self.state.write().unwrap();
        let now = (self.clock)();
        if let Some(retry_after) = state
            .entries
            .get(&peer)
            .and_then(|entry| entry.banned_until)
            .and_then(|until| remaining_secs(until, now))
        {
            return Some(retry_after);
        }

        if let Some(ip) = peer {
            if let Some(previous) = state
                .entries
                .get(&peer)
                .and_then(|entry| entry.failures.back().copied())
            {
                state.oldest.remove(&(previous, ip));
            } else if state.oldest.len() == MAX_IPS {
                if let Some((_, oldest_ip)) = state.oldest.pop_first() {
                    state.entries.remove(&Some(oldest_ip));
                }
            }
            state.oldest.insert((now, ip));
        }

        let entry = state.entries.entry(peer).or_default();
        entry.banned_until = None;
        while entry
            .failures
            .front()
            .is_some_and(|at| now.duration_since(*at) >= FAILURE_WINDOW)
        {
            entry.failures.pop_front();
        }
        entry.failures.push_back(now);
        let threshold = if peer.is_some() {
            IP_THRESHOLD
        } else {
            UNKNOWN_THRESHOLD
        };
        if entry.failures.len() < threshold {
            return None;
        }
        entry.banned_until = Some(now + BAN_DURATION);
        drop(state);
        // Only the transition into a ban logs, once per ban, without headers
        // or credentials. Further attempts are handled by retry_after.
        tracing::warn!(
            ?peer,
            "[WEB] Too many authentication failures; banned for 300 seconds"
        );
        Some(BAN_DURATION.as_secs())
    }
}

fn remaining_secs(until: Instant, now: Instant) -> Option<u64> {
    until
        .checked_duration_since(now)
        .filter(|d| !d.is_zero())
        .map(|d| d.as_secs() + u64::from(d.subsec_nanos() != 0))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) struct FakeClock {
        start: Instant,
        millis: AtomicU64,
    }

    impl FakeClock {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                start: Instant::now(),
                millis: AtomicU64::new(0),
            })
        }

        pub(crate) fn advance(&self, millis: u64) {
            self.millis.fetch_add(millis, Ordering::Relaxed);
        }

        pub(crate) fn limiter(self: &Arc<Self>) -> Arc<AuthFailureLimiter> {
            let clock = self.clone();
            Arc::new(AuthFailureLimiter::with_clock(Arc::new(move || {
                clock.start + Duration::from_millis(clock.millis.load(Ordering::Relaxed))
            })))
        }
    }

    fn ip(value: &str) -> Option<IpAddr> {
        Some(value.parse().unwrap())
    }

    #[test]
    fn bans_at_tenth_failure_and_expires_without_extending_on_retries() {
        let clock = FakeClock::new();
        let limiter = clock.limiter();
        let peer = ip("192.0.2.10");
        for _ in 0..9 {
            assert_eq!(limiter.record_failure(peer), None);
        }
        assert_eq!(limiter.record_failure(peer), Some(300));
        clock.advance(1);
        assert_eq!(limiter.retry_after(peer), Some(300));
        clock.advance(298_999);
        assert_eq!(limiter.record_failure(peer), Some(1));
        clock.advance(1_000);
        assert_eq!(limiter.retry_after(peer), None);
        assert_eq!(limiter.record_failure(peer), None);
    }

    #[test]
    fn counts_a_sliding_window_and_discards_failures_at_sixty_seconds() {
        let clock = FakeClock::new();
        let limiter = clock.limiter();
        let peer = ip("192.0.2.11");
        limiter.record_failure(peer);
        clock.advance(30_000);
        for _ in 0..8 {
            limiter.record_failure(peer);
        }
        clock.advance(30_000);
        assert_eq!(limiter.record_failure(peer), None);
        // Nine failures in the preceding 30 seconds, despite crossing the
        // first failure's fixed-window boundary.
        assert_eq!(limiter.record_failure(peer), Some(300));
    }

    #[test]
    fn exempts_loopback_including_ipv4_mapped_addresses() {
        let limiter = AuthFailureLimiter::default();
        for peer in ["127.0.0.1", "127.2.3.4", "::1", "::ffff:127.0.0.1"] {
            for _ in 0..60 {
                assert_eq!(limiter.record_failure(ip(peer)), None);
                assert_eq!(limiter.retry_after(ip(peer)), None);
            }
        }
        assert!(limiter.state.read().unwrap().entries.is_empty());
    }

    #[test]
    fn unknown_peer_has_a_separate_fifty_failure_threshold() {
        let clock = FakeClock::new();
        let limiter = clock.limiter();
        for _ in 0..49 {
            assert_eq!(limiter.record_failure(None), None);
        }
        assert_eq!(limiter.record_failure(None), Some(300));
        assert_eq!(limiter.retry_after(ip("192.0.2.12")), None);
        clock.advance(300_000);
        assert_eq!(limiter.retry_after(None), None);
    }

    #[test]
    fn ipv4_mapped_and_native_addresses_share_a_bucket() {
        let limiter = AuthFailureLimiter::default();
        for _ in 0..9 {
            limiter.record_failure(ip("192.0.2.13"));
        }
        assert_eq!(limiter.record_failure(ip("::ffff:192.0.2.13")), Some(300));
        assert_eq!(limiter.retry_after(ip("192.0.2.13")), Some(300));
    }

    #[test]
    fn evicts_oldest_failure_at_capacity_without_evicting_unknown_peer() {
        let clock = FakeClock::new();
        let limiter = clock.limiter();
        for _ in 0..50 {
            limiter.record_failure(None);
        }
        for number in 0..10_000u32 {
            let peer = Some(IpAddr::V4((0xc000_0000 + number).into()));
            limiter.record_failure(peer);
        }
        clock.advance(1);
        // Refresh the oldest IP; the next-oldest must be evicted instead.
        limiter.record_failure(ip("192.0.0.0"));
        limiter.record_failure(ip("198.51.100.1"));
        let state = limiter.state.read().unwrap();
        assert_eq!(state.entries.len(), 10_001);
        assert_eq!(state.oldest.len(), 10_000);
        assert!(state.entries.contains_key(&ip("192.0.0.0")));
        assert!(!state.entries.contains_key(&ip("192.0.0.1")));
        drop(state);
        assert_eq!(limiter.retry_after(None), Some(300));
    }

    #[test]
    fn successful_checks_do_not_allocate_peer_entries() {
        let limiter = AuthFailureLimiter::default();
        for number in 0..20_000u32 {
            assert_eq!(
                limiter.retry_after(Some(IpAddr::V4((0xc000_0000 + number).into()))),
                None
            );
        }
        assert!(limiter.state.read().unwrap().entries.is_empty());
    }

    #[test]
    fn concurrent_failures_cannot_lose_the_threshold() {
        let limiter = Arc::new(AuthFailureLimiter::default());
        std::thread::scope(|scope| {
            for _ in 0..10 {
                let limiter = limiter.clone();
                scope.spawn(move || limiter.record_failure(ip("192.0.2.14")));
            }
        });
        assert_eq!(limiter.retry_after(ip("192.0.2.14")), Some(300));
    }
}
