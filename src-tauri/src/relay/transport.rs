use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use codeg_tunnel::mux::MuxMessage;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use sha2::{Digest, Sha256};
use tokio::{net::TcpStream, time::Instant};
use tokio_tungstenite::{
    tungstenite::{self, client::IntoClientRequest, protocol::WebSocketConfig, Message},
    MaybeTlsStream, WebSocketStream,
};

use super::config::{validate_key, RelayConfig};

pub const PROTOCOL: &str = "codeg-relay.v1";
pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Debug)]
pub struct ConnectError {
    pub code: &'static str,
    pub message: &'static str,
    pub fingerprint: Option<String>,
}

impl ConnectError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            fingerprint: None,
        }
    }
    fn with_fingerprint(mut self, fingerprint: Option<String>) -> Self {
        self.fingerprint = fingerprint;
        self
    }
}

pub struct Connected {
    pub socket: Socket,
    pub fingerprint: Option<String>,
    pub rtt_ms: u64,
}

pub fn certificate_fingerprint(der: &[u8]) -> String {
    format!("{:x}", Sha256::digest(der))
}

async fn tls(
    host: &str,
    port: u16,
    allow_untrusted: bool,
) -> Result<(tokio_native_tls::TlsStream<TcpStream>, String), ConnectError> {
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|_| ConnectError::new("unreachable", "Cannot reach relay"))?;
    let _ = tcp.set_nodelay(true);
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(allow_untrusted)
        .danger_accept_invalid_hostnames(allow_untrusted)
        .build()
        .map_err(|_| ConnectError::new("protocol", "Cannot initialize TLS"))?;
    let stream = tokio_native_tls::TlsConnector::from(connector)
        .connect(host, tcp)
        .await
        .map_err(|_| {
            ConnectError::new(
                "tls_untrusted",
                "Relay TLS certificate could not be verified",
            )
        })?;
    let cert = stream
        .get_ref()
        .peer_certificate()
        .ok()
        .flatten()
        .and_then(|v| v.to_der().ok())
        .ok_or_else(|| ConnectError::new("protocol", "Relay did not present a certificate"))?;
    Ok((stream, certificate_fingerprint(&cert)))
}

pub async fn connect(config: &RelayConfig) -> Result<Connected, ConnectError> {
    tokio::time::timeout(Duration::from_secs(10), connect_inner(config))
        .await
        .unwrap_or_else(|_| Err(ConnectError::new("timeout", "Relay connection timed out")))
}

async fn connect_inner(config: &RelayConfig) -> Result<Connected, ConnectError> {
    let started = Instant::now();
    validate_key(&config.key).map_err(|_| ConnectError::new("protocol", "Invalid relay key"))?;
    let url = config
        .agent_url()
        .map_err(|_| ConnectError::new("protocol", "Invalid relay URL"))?;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = url.port_or_known_default().unwrap_or(443);
    let (stream, fingerprint) = if url.scheme() == "wss" {
        let (stream, fingerprint) = match tls(host, port, config.fingerprint.is_some()).await {
            Ok(result) => result,
            Err(error) if error.code == "tls_untrusted" && config.fingerprint.is_none() => {
                // TOFU probe sends no HTTP request, bearer key, or application data.
                return Err(match tls(host, port, true).await {
                    Ok((_, fp)) => error.with_fingerprint(Some(fp)),
                    Err(probe) if probe.code == "unreachable" => probe,
                    Err(_) => ConnectError::new(
                        "protocol",
                        "Relay TLS handshake failed without a certificate",
                    ),
                });
            }
            Err(error) if error.code == "tls_untrusted" => {
                return Err(ConnectError::new(
                    "protocol",
                    "Relay TLS handshake failed without a certificate",
                ))
            }
            Err(error) => return Err(error),
        };
        if config
            .fingerprint
            .as_ref()
            .is_some_and(|pin| pin != &fingerprint)
        {
            return Err(ConnectError::new(
                "fingerprint_mismatch",
                "Relay certificate fingerprint does not match",
            )
            .with_fingerprint(Some(fingerprint)));
        }
        (MaybeTlsStream::NativeTls(stream), Some(fingerprint))
    } else {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|_| ConnectError::new("unreachable", "Cannot reach relay"))?;
        let peer = tcp
            .peer_addr()
            .map_err(|_| ConnectError::new("unreachable", "Cannot reach relay"))?;
        if !(peer.ip().is_loopback()
            || matches!(peer.ip(), std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some_and(|v| v.is_loopback())))
        {
            return Err(ConnectError::new("protocol", "Plaintext requires loopback"));
        }
        let _ = tcp.set_nodelay(true);
        (MaybeTlsStream::Plain(tcp), None)
    };
    // Authentication is constructed only after trust has been established.
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| ConnectError::new("protocol", "Invalid relay request"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.key)
            .parse()
            .map_err(|_| ConnectError::new("protocol", "Invalid relay key"))?,
    );
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", PROTOCOL.parse().unwrap());
    request.headers_mut().insert(
        "User-Agent",
        concat!("codeg/", env!("CARGO_PKG_VERSION"))
            .parse()
            .unwrap(),
    );
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(32773))
        .max_frame_size(Some(32773));
    let (socket, response) =
        tokio_tungstenite::client_async_with_config(request, stream, Some(ws_config))
            .await
            .map_err(|error| {
                let error = match error {
                    tungstenite::Error::Http(response)
                        if matches!(response.status().as_u16(), 401 | 403) =>
                    {
                        ConnectError::new("auth_rejected", "Relay rejected the agent key")
                    }
                    tungstenite::Error::Io(_) => {
                        ConnectError::new("unreachable", "Relay connection closed during handshake")
                    }
                    _ => ConnectError::new("protocol", "Relay WebSocket handshake failed"),
                };
                error.with_fingerprint(fingerprint.clone())
            })?;
    if response
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
        != Some(PROTOCOL)
    {
        return Err(
            ConnectError::new("protocol", "Relay did not negotiate codeg-relay.v1")
                .with_fingerprint(fingerprint),
        );
    }
    Ok(Connected {
        socket,
        fingerprint,
        rtt_ms: started.elapsed().as_millis() as u64,
    })
}

#[derive(Default)]
pub struct Observations {
    ping: Option<(Vec<u8>, Instant)>,
    pub rtt_ms: Option<u64>,
    pub replaced: bool,
}

pub fn adapter<S>(
    socket: WebSocketStream<S>,
    observations: Arc<Mutex<Observations>>,
) -> impl Stream<Item = io::Result<MuxMessage>> + Sink<MuxMessage, Error = tungstenite::Error> + Send
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let sent = observations.clone();
    socket
        .with(move |message: MuxMessage| {
            futures_util::future::ready(Ok(match message {
                MuxMessage::Binary(v) => Message::Binary(v.into()),
                MuxMessage::Ping(v) => {
                    sent.lock().unwrap().ping = Some((v.clone(), Instant::now()));
                    Message::Ping(v.into())
                }
                MuxMessage::Pong(v) => Message::Pong(v.into()),
                MuxMessage::Close => Message::Close(None),
            }))
        })
        .map(move |message| {
            Ok(
                match message.map_err(|_| io::Error::other("Relay WebSocket transport failed"))? {
                    Message::Binary(v) => MuxMessage::Binary(v.to_vec()),
                    Message::Ping(v) => MuxMessage::Ping(v.to_vec()),
                    Message::Pong(v) => {
                        let mut observed = observations.lock().unwrap();
                        if let Some((payload, at)) = &observed.ping {
                            if payload.as_slice() == v.as_ref() {
                                observed.rtt_ms = Some(at.elapsed().as_millis() as u64);
                                observed.ping = None;
                            }
                        }
                        MuxMessage::Pong(v.to_vec())
                    }
                    Message::Close(frame) => {
                        observations.lock().unwrap().replaced =
                            frame.is_some_and(|v| u16::from(v.code) == 4001);
                        MuxMessage::Close
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Expected relay binary frame",
                        ))
                    }
                },
            )
        })
}
