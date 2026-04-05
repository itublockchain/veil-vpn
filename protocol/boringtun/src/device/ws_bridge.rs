use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tungstenite::protocol::Message;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;

const UDP_READ_TIMEOUT_MS: u64 = 50;
const WS_READ_TIMEOUT_MS: u64 = 100;
const MAX_WG_PACKET: usize = 1500;
const RECONNECT_DELAY_MS: u64 = 2000;

/// Client-side WebSocket-to-UDP bridge.
///
/// Boringtun client sends/receives WireGuard UDP packets on a local port.
/// This bridge forwards them over a WebSocket connection to the server's WS proxy,
/// and relays responses back over UDP.
///
///   Boringtun client ↔ UDP (local_port) ↔ WS Bridge ↔ WebSocket ↔ Server WS Proxy
pub fn run_ws_bridge(local_udp_port: u16, ws_url: &str, shutdown_flag: Arc<AtomicBool>) {
    let udp = match UdpSocket::bind(format!("127.0.0.1:{}", local_udp_port)) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                "WS bridge: failed to bind UDP on port {}: {}",
                local_udp_port,
                e
            );
            return;
        }
    };
    udp.set_read_timeout(Some(Duration::from_millis(UDP_READ_TIMEOUT_MS)))
        .ok();

    tracing::info!("WS bridge: UDP listening on 127.0.0.1:{}", local_udp_port);

    let mut client_addr: Option<SocketAddr> = None;
    let mut udp_buf = [0u8; MAX_WG_PACKET];
    let mut udp_to_ws: u64 = 0;
    let mut ws_to_udp: u64 = 0;

    while !shutdown_flag.load(Ordering::Relaxed) {
        // Connect to server WebSocket
        tracing::info!("WS bridge: connecting to {}...", ws_url);
        let mut ws = match connect_ws(ws_url) {
            Some(ws) => ws,
            None => {
                tracing::warn!(
                    "WS bridge: connection failed, retrying in {}ms",
                    RECONNECT_DELAY_MS
                );
                std::thread::sleep(Duration::from_millis(RECONNECT_DELAY_MS));
                continue;
            }
        };
        tracing::info!("WS bridge: connected to {}", ws_url);

        // Main relay loop
        loop {
            if shutdown_flag.load(Ordering::Relaxed) {
                break;
            }

            // 1. Read from local UDP (boringtun client) → send to WebSocket
            match udp.recv_from(&mut udp_buf) {
                Ok((n, from)) => {
                    client_addr = Some(from);
                    udp_to_ws += 1;
                    if let Err(e) = ws.send(Message::Binary(udp_buf[..n].to_vec().into())) {
                        tracing::warn!("WS bridge: UDP→WS send error: {}, reconnecting", e);
                        break;
                    }
                    tracing::trace!(
                        "WS bridge: UDP→WS #{} | {} bytes from {} | first4=[{:02x}{:02x}{:02x}{:02x}]",
                        udp_to_ws, n, from,
                        udp_buf[0], udp_buf[1.min(n-1)], udp_buf[2.min(n-1)], udp_buf[3.min(n-1)]
                    );
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    tracing::warn!("WS bridge: UDP recv error: {}", e);
                    break;
                }
            }

            // 2. Read from WebSocket → send to local UDP (boringtun client)
            match ws.read() {
                Ok(Message::Binary(data)) => {
                    ws_to_udp += 1;
                    if let Some(addr) = client_addr {
                        if let Err(e) = udp.send_to(&data, addr) {
                            tracing::warn!("WS bridge: WS→UDP send error: {}", e);
                        } else {
                            tracing::trace!(
                                "WS bridge: WS→UDP #{} | {} bytes → {} | first4=[{:02x}{:02x}{:02x}{:02x}]",
                                ws_to_udp, data.len(), addr,
                                data[0], data[1.min(data.len()-1)], data[2.min(data.len()-1)], data[3.min(data.len()-1)]
                            );
                        }
                    } else {
                        tracing::warn!("WS bridge: WS→UDP #{} | dropped, no client yet", ws_to_udp);
                    }
                }
                Ok(Message::Ping(payload)) => {
                    let _ = ws.send(Message::Pong(payload));
                }
                Ok(Message::Close(_)) => {
                    tracing::info!("WS bridge: server closed connection");
                    break;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    tracing::warn!("WS bridge: WS read error: {}, reconnecting", e);
                    break;
                }
            }
        }

        let _ = ws.close(None);
        tracing::info!(
            "WS bridge: session ended | UDP→WS: {} | WS→UDP: {}",
            udp_to_ws,
            ws_to_udp
        );

        if !shutdown_flag.load(Ordering::Relaxed) {
            tracing::info!("WS bridge: reconnecting in {}ms...", RECONNECT_DELAY_MS);
            std::thread::sleep(Duration::from_millis(RECONNECT_DELAY_MS));
        }
    }

    tracing::info!("WS bridge: shutting down");
}

fn connect_ws(url: &str) -> Option<WebSocket<MaybeTlsStream<std::net::TcpStream>>> {
    match tungstenite::connect(url) {
        Ok((ws, _response)) => {
            // Set read timeout on the underlying TCP stream
            if let MaybeTlsStream::Plain(ref stream) = ws.get_ref() {
                stream
                    .set_read_timeout(Some(Duration::from_millis(WS_READ_TIMEOUT_MS)))
                    .ok();
                stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            }
            Some(ws)
        }
        Err(e) => {
            tracing::warn!("WS bridge: connect error: {}", e);
            None
        }
    }
}
