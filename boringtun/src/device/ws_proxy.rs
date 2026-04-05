use std::net::{TcpListener, TcpStream, UdpSocket, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tungstenite::protocol::Message;
use tungstenite::accept;

const WS_READ_TIMEOUT_MS: u64 = 100;
const UDP_READ_TIMEOUT_MS: u64 = 50;
const MAX_WG_PACKET: usize = 1500;

/// Run the WebSocket-to-WireGuard proxy server.
///
/// Each incoming WebSocket connection gets a dedicated UDP socket bound to an
/// ephemeral port. WireGuard packets are relayed bidirectionally:
///   Client ↔ WebSocket ↔ UDP ↔ WireGuard (127.0.0.1:{wg_port})
///
/// This allows TEE nodes to expose only HTTP+WS ports (no raw UDP).
pub fn run_ws_proxy(
    bind_addr: &str,
    wg_port: u16,
    shutdown_flag: Arc<AtomicBool>,
) {
    let listener = match TcpListener::bind(bind_addr) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("WS proxy: failed to bind on {}: {}", bind_addr, e);
            return;
        }
    };

    // Set a timeout on accept so we periodically check shutdown
    set_tcp_listener_timeout(&listener, Duration::from_secs(2));

    tracing::info!("WS proxy listening on {}", bind_addr);

    let wg_addr: SocketAddr = format!("127.0.0.1:{}", wg_port).parse().unwrap();

    while !shutdown_flag.load(Ordering::Relaxed) {
        let stream = match listener.accept() {
            Ok((stream, peer_addr)) => {
                tracing::info!("WS proxy: new connection from {}", peer_addr);
                stream
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {
                continue;
            }
            Err(e) => {
                tracing::warn!("WS proxy: accept error: {}", e);
                continue;
            }
        };

        let shutdown = Arc::clone(&shutdown_flag);
        std::thread::Builder::new()
            .name("ws-client".into())
            .spawn(move || {
                handle_ws_client(stream, wg_addr, shutdown);
            })
            .ok();
    }

    tracing::info!("WS proxy shutting down");
}

fn handle_ws_client(
    stream: TcpStream,
    wg_addr: SocketAddr,
    shutdown_flag: Arc<AtomicBool>,
) {
    let peer_addr = stream.peer_addr().ok();

    // Set TCP timeouts for the WebSocket handshake and reads
    stream.set_read_timeout(Some(Duration::from_millis(WS_READ_TIMEOUT_MS))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let mut ws = match accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!("WS proxy: handshake failed from {:?}: {}", peer_addr, e);
            return;
        }
    };

    // Create an UNCONNECTED UDP socket to talk to the local WireGuard instance.
    // We use send_to/recv_from instead of connect/send/recv because boringtun
    // may respond from a different port (connected UDP sockets per peer).
    let udp = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("WS proxy: failed to bind UDP socket: {}", e);
            let _ = ws.close(None);
            return;
        }
    };
    udp.set_read_timeout(Some(Duration::from_millis(UDP_READ_TIMEOUT_MS))).ok();

    let local_udp = udp.local_addr().unwrap();
    tracing::info!(
        "WS proxy: session started for {:?}, UDP {} → {}",
        peer_addr,
        local_udp,
        wg_addr
    );

    let mut udp_buf = [0u8; MAX_WG_PACKET];
    let mut ws_to_udp_count: u64 = 0;
    let mut udp_to_ws_count: u64 = 0;

    loop {
        if shutdown_flag.load(Ordering::Relaxed) {
            tracing::info!("WS proxy [{:?}]: shutdown flag set, exiting", peer_addr);
            break;
        }

        // 1. Read from WebSocket → send to WireGuard UDP
        match ws.read() {
            Ok(Message::Binary(data)) => {
                ws_to_udp_count += 1;
                tracing::trace!(
                    "WS proxy [{:?}]: WS→UDP #{} | {} bytes | sending to {}",
                    peer_addr, ws_to_udp_count, data.len(), wg_addr
                );
                match udp.send_to(&data, wg_addr) {
                    Ok(sent) => {
                        tracing::trace!(
                            "WS proxy [{:?}]: WS→UDP #{} | sent {} bytes to {}",
                            peer_addr, ws_to_udp_count, sent, wg_addr
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "WS proxy [{:?}]: WS→UDP #{} | send error: {}",
                            peer_addr, ws_to_udp_count, e
                        );
                        break;
                    }
                }
            }
            Ok(Message::Close(frame)) => {
                tracing::info!("WS proxy [{:?}]: received Close frame: {:?}", peer_addr, frame);
                break;
            }
            Ok(Message::Ping(payload)) => {
                tracing::debug!("WS proxy [{:?}]: received Ping ({} bytes)", peer_addr, payload.len());
                let _ = ws.send(Message::Pong(payload));
            }
            Ok(Message::Text(text)) => {
                tracing::info!("WS proxy [{:?}]: received unexpected Text: {}", peer_addr, text);
            }
            Ok(msg) => {
                tracing::info!("WS proxy [{:?}]: received other message type: {:?}", peer_addr, msg);
            }
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Timeout — fall through to check UDP
            }
            Err(e) => {
                tracing::info!("WS proxy [{:?}]: WS read error: {}", peer_addr, e);
                break;
            }
        }

        // 2. Read from WireGuard UDP → send to WebSocket
        //    Accept packets from ANY source (boringtun may use connected sockets
        //    on different ports for the data fast-path).
        match udp.recv_from(&mut udp_buf) {
            Ok((n, from)) => {
                udp_to_ws_count += 1;
                tracing::trace!(
                    "WS proxy [{:?}]: UDP→WS #{} | {} bytes from {} | forwarding to WS",
                    peer_addr, udp_to_ws_count, n, from
                );
                match ws.send(Message::Binary(udp_buf[..n].to_vec().into())) {
                    Ok(_) => {
                        tracing::trace!(
                            "WS proxy [{:?}]: UDP→WS #{} | sent {} bytes to WS",
                            peer_addr, udp_to_ws_count, n
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "WS proxy [{:?}]: UDP→WS #{} | WS send error: {}",
                            peer_addr, udp_to_ws_count, e
                        );
                        break;
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // No data from WG yet, loop back
            }
            Err(e) => {
                tracing::warn!("WS proxy [{:?}]: UDP recv error: {}", peer_addr, e);
                break;
            }
        }
    }

    tracing::info!(
        "WS proxy [{:?}]: session stats | WS→UDP: {} packets | UDP→WS: {} packets",
        peer_addr, ws_to_udp_count, udp_to_ws_count
    );

    let _ = ws.close(None);
    tracing::info!("WS proxy: session ended for {:?}", peer_addr);
}

#[cfg(unix)]
fn set_tcp_listener_timeout(listener: &TcpListener, timeout: Duration) {
    use std::os::unix::io::AsRawFd;
    let fd = listener.as_raw_fd();
    let tv = libc::timeval {
        tv_sec: timeout.as_secs() as _,
        tv_usec: timeout.subsec_micros() as _,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as u32,
        );
    }
}
