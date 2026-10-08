use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use axum::Router;
use codeg_tunnel::{
    frame::CloseCode,
    mux::{IncomingStream, TunnelStream},
};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tower::ServiceExt;

#[derive(Default)]
pub struct StreamLimits(Mutex<HashMap<IpAddr, usize>>);

impl StreamLimits {
    fn acquire(self: &Arc<Self>, mut ip: IpAddr) -> Option<Permit> {
        if let IpAddr::V6(v6) = ip {
            if let Some(v4) = v6.to_ipv4_mapped() {
                ip = IpAddr::V4(v4);
            }
        }
        let mut peers = self.0.lock().unwrap();
        if peers.values().sum::<usize>() >= 256 || peers.get(&ip).copied().unwrap_or(0) >= 64 {
            return None;
        }
        *peers.entry(ip).or_default() += 1;
        Some(Permit {
            limits: self.clone(),
            ip,
        })
    }
}

struct Permit {
    limits: Arc<StreamLimits>,
    ip: IpAddr,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut peers = self.limits.0.lock().unwrap();
        if let Some(count) = peers.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                peers.remove(&self.ip);
            }
        }
    }
}

// Hyper moves this IO into upgraded WebSockets. Keep the admission permit on
// the IO itself, so an upgrade cannot release its per-IP slot prematurely.
struct AdmittedStream {
    stream: TunnelStream,
    _permit: Permit,
}
impl AsyncRead for AdmittedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for AdmittedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

pub async fn serve(incoming: IncomingStream, router: Router, limits: Arc<StreamLimits>) {
    let Ok(ip) = incoming.host().parse::<IpAddr>() else {
        let _ = incoming
            .reject(CloseCode::BadRequest, "OPEN host must be an IP literal")
            .await;
        return;
    };
    let peer = SocketAddr::new(ip, incoming.port());
    let Some(permit) = limits.acquire(ip) else {
        tracing::warn!(%peer, "[RELAY] Stream limit reached");
        let _ = incoming
            .reject(CloseCode::NotAllowed, "Relay stream limit reached")
            .await;
        return;
    };
    let Ok(stream) = incoming.accept().await else {
        return;
    };
    let io = TokioIo::new(AdmittedStream {
        stream,
        _permit: permit,
    });
    let service = hyper::service::service_fn(move |mut request| {
        crate::web::auth::with_peer(&mut request, peer);
        router.clone().oneshot(request)
    });
    // Errors can include request-controlled values; do not log those values.
    let _ = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .keep_alive(true)
        .header_read_timeout(Duration::from_secs(30))
        .serve_connection(io, service)
        .with_upgrades()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_survive_until_io_drop_and_normalize_mapped_ipv4() {
        let limits = Arc::new(StreamLimits::default());
        let ip = "192.0.2.1".parse().unwrap();
        let mut slots: Vec<_> = (0..64).map(|_| limits.acquire(ip).unwrap()).collect();
        assert!(limits
            .acquire("::ffff:192.0.2.1".parse().unwrap())
            .is_none());
        slots.pop();
        slots.push(limits.acquire(ip).unwrap());
        for n in 2..5 {
            for _ in 0..64 {
                slots.push(
                    limits
                        .acquire(format!("192.0.2.{n}").parse().unwrap())
                        .unwrap(),
                );
            }
        }
        assert!(limits.acquire("192.0.2.5".parse().unwrap()).is_none());
        drop(slots);
        assert!(limits.0.lock().unwrap().is_empty());
    }
}
