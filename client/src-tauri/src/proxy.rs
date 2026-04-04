/// WebSocket <-> UDP proxy.
///
/// Bridges local UDP port 51820 (where boringtun/WireGuard listens) to the
/// server's WebSocket data plane endpoint. This is needed because the TEE
/// server exposes WireGuard traffic over WebSocket instead of raw UDP.
///
/// Flow:
///   WireGuard (UDP :51820) <-> this proxy <-> Server WebSocket
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};
use tokio_util::sync::CancellationToken;

pub struct Proxy {
    cancel: CancellationToken,
}

impl Proxy {
    /// Start the proxy connecting to the given WebSocket URL.
    /// Returns immediately; the proxy runs as a background tokio task.
    pub async fn start(ws_url: String) -> Result<Self, String> {
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            if let Err(e) = run_proxy(ws_url, cancel_clone).await {
                log::error!("[proxy] fatal: {e}");
            }
        });

        Ok(Proxy { cancel })
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

async fn run_proxy(ws_url: String, cancel: CancellationToken) -> Result<(), String> {
    log::info!("[proxy] connecting to {ws_url}");

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| format!("WS connect failed: {e}"))?;

    log::info!("[proxy] WebSocket connected");

    let (ws_sink, ws_stream) = ws_stream.split();
    let ws_sink = Arc::new(Mutex::new(ws_sink));

    let udp = Arc::new(
        UdpSocket::bind("127.0.0.1:51820")
            .await
            .map_err(|e| format!("UDP bind :51820 failed: {e}"))?,
    );

    // Address of WireGuard/boringtun on the local side.
    // Learned dynamically on first packet from WireGuard.
    let wg_peer: Arc<Mutex<Option<std::net::SocketAddr>>> = Arc::new(Mutex::new(None));

    // ── UDP → WebSocket ──────────────────────────────────────────────────────
    {
        let udp = udp.clone();
        let ws_sink = ws_sink.clone();
        let wg_peer = wg_peer.clone();
        let cancel = cancel.clone();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        log::info!("[proxy] UDP→WS task cancelled");
                        break;
                    }
                    result = udp.recv_from(&mut buf) => {
                        match result {
                            Ok((n, addr)) => {
                                *wg_peer.lock().await = Some(addr);
                                let data = buf[..n].to_vec();
                                log::debug!("[proxy] UDP→WS {} bytes", n);
                                let _ = ws_sink
                                    .lock()
                                    .await
                                    .send(Message::Binary(data))
                                    .await;
                            }
                            Err(e) => {
                                log::error!("[proxy] UDP recv error: {e}");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    // ── WebSocket → UDP ──────────────────────────────────────────────────────
    let mut ws_stream = ws_stream;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                log::info!("[proxy] WS→UDP task cancelled");
                break;
            }
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if let Some(addr) = *wg_peer.lock().await {
                            log::debug!("[proxy] WS→UDP {} bytes", data.len());
                            let _ = udp.send_to(&data, addr).await;
                        }
                    }
                    Some(Ok(_)) => {} // ignore ping/pong/text frames
                    Some(Err(e)) => {
                        log::error!("[proxy] WS error: {e}");
                        break;
                    }
                    None => {
                        log::info!("[proxy] WS stream closed");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
