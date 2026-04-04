use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_BODY_BYTES: usize = 1024;
const STALE_PEER_SECS: u64 = 600; // 10 minutes
const STALE_NEVER_CONNECTED_SECS: u64 = 600; // 10 min for peers that never handshaked
const REAPER_INTERVAL_SECS: u64 = 60;
const RATE_LIMIT_PER_SEC: f64 = 1.0;
const RATE_LIMIT_BURST: u32 = 10;
const RATE_LIMITER_PRUNE_SECS: u64 = 300; // prune entries older than 5 min
const UAPI_TIMEOUT: Duration = Duration::from_secs(5);
const UAPI_SOCK_DIR: &str = "/var/run/wireguard";

/// Snapshot of payment config for registration response.
#[derive(Clone)]
pub struct PaymentConfigSnapshot {
    pub chain_id: u64,
    pub gateway_wallet: [u8; 20],
    pub usdc_contract: [u8; 20],
    pub amount_per_quota: u64,
    pub quota_bytes: u64,
}

pub struct RegistrationState {
    inner: Mutex<RegistrationInner>,
    rate_limiter: Mutex<HashMap<IpAddr, TokenBucket>>,
    pub tun_name: String,
    pub subnet_prefix: [u8; 3],
    pub payment_config: PaymentConfigSnapshot,
    pub public_ip: String,
}

struct RegistrationInner {
    registered_peers: HashSet<String>,
    available_ips: HashSet<u8>,
    peer_to_octet: HashMap<String, u8>,
    /// Track when each peer was registered (for zombie reaping — P1-A fix).
    peer_registered_at: HashMap<String, Instant>,
    /// Lazily cached server public key (base64).
    server_pubkey_cache: Option<String>,
    /// Lazily cached listen port.
    listen_port_cache: Option<u16>,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new() -> Self {
        Self {
            tokens: RATE_LIMIT_BURST as f64,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * RATE_LIMIT_PER_SEC).min(RATE_LIMIT_BURST as f64);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl RegistrationState {
    pub fn new(
        tun_name: String,
        subnet_prefix: [u8; 3],
        payment_config: PaymentConfigSnapshot,
        public_ip: String,
    ) -> Self {
        let mut available_ips = HashSet::new();
        for octet in 2..=254u8 {
            available_ips.insert(octet);
        }
        Self {
            inner: Mutex::new(RegistrationInner {
                registered_peers: HashSet::new(),
                available_ips,
                peer_to_octet: HashMap::new(),
                peer_registered_at: HashMap::new(),
                server_pubkey_cache: None,
                listen_port_cache: None,
            }),
            rate_limiter: Mutex::new(HashMap::new()),
            tun_name,
            subnet_prefix,
            payment_config,
            public_ip,
        }
    }
}

// === UAPI Client ===

/// Query UAPI get=1. Returns map of hex_pubkey → Option<last_handshake_time_sec>.
fn uapi_get_peers(tun_name: &str) -> Result<(Option<String>, Option<u16>, HashMap<String, Option<u64>>), String> {
    let path = format!("{}/{}.sock", UAPI_SOCK_DIR, tun_name);
    let stream = UnixStream::connect(&path).map_err(|e| format!("UAPI connect: {}", e))?;
    stream.set_read_timeout(Some(UAPI_TIMEOUT)).ok();
    stream.set_write_timeout(Some(UAPI_TIMEOUT)).ok();

    let mut writer = std::io::BufWriter::new(&stream);
    write!(writer, "get=1\n").map_err(|e| format!("UAPI write: {}", e))?;
    writer.flush().map_err(|e| format!("UAPI flush: {}", e))?;
    drop(writer);

    let reader = BufReader::new(&stream);
    let mut peers: HashMap<String, Option<u64>> = HashMap::new();
    let mut current_pubkey: Option<String> = None;
    let mut current_handshake: Option<u64> = None;
    let mut own_pubkey: Option<String> = None;
    let mut listen_port: Option<u16> = None;

    for line in reader.lines() {
        let line = line.map_err(|e| format!("UAPI read: {}", e))?;

        // Empty lines separate peer blocks — flush current peer but keep reading
        if line.is_empty() {
            if let Some(pk) = current_pubkey.take() {
                peers.insert(pk, current_handshake.take());
            }
            continue;
        }

        // errno= terminates the UAPI response
        if line.starts_with("errno=") {
            if let Some(pk) = current_pubkey.take() {
                peers.insert(pk, current_handshake.take());
            }
            break;
        }
        if let Some((key, val)) = line.split_once('=') {
            match key {
                "own_public_key" => {
                    // Convert hex to base64
                    if let Ok(bytes) = hex::decode(val) {
                        own_pubkey = Some(base64::encode(&bytes));
                    }
                }
                "listen_port" => {
                    listen_port = val.parse().ok();
                }
                "public_key" => {
                    if let Some(pk) = current_pubkey.take() {
                        peers.insert(pk, current_handshake.take());
                    }
                    current_pubkey = Some(val.to_string());
                    current_handshake = None;
                }
                "last_handshake_time_sec" => {
                    current_handshake = val.parse().ok();
                }
                _ => {}
            }
        }
    }
    if let Some(pk) = current_pubkey.take() {
        peers.insert(pk, current_handshake.take());
    }

    Ok((own_pubkey, listen_port, peers))
}

/// Add a peer via UAPI set=1. Returns Ok(()) on errno=0.
fn uapi_set_peer(tun_name: &str, pubkey_hex: &str, allowed_ip: Ipv4Addr) -> Result<(), String> {
    let path = format!("{}/{}.sock", UAPI_SOCK_DIR, tun_name);
    let stream = UnixStream::connect(&path).map_err(|e| format!("UAPI connect: {}", e))?;
    stream.set_read_timeout(Some(UAPI_TIMEOUT)).ok();
    stream.set_write_timeout(Some(UAPI_TIMEOUT)).ok();

    let mut writer = std::io::BufWriter::new(&stream);
    write!(
        writer,
        "set=1\npublic_key={}\nreplace_allowed_ips=true\nallowed_ip={}/32\n\n",
        pubkey_hex, allowed_ip
    )
    .map_err(|e| format!("UAPI write: {}", e))?;
    writer.flush().map_err(|e| format!("UAPI flush: {}", e))?;
    drop(writer);

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader.read_line(&mut response).map_err(|e| format!("UAPI read: {}", e))?;

    if response.trim() == "errno=0" {
        Ok(())
    } else {
        Err(format!("UAPI set failed: {}", response.trim()))
    }
}

/// Remove a peer via UAPI. Returns true on errno=0 (P1-B fix: check errno).
fn uapi_remove_peer(tun_name: &str, pubkey_hex: &str) -> Result<bool, String> {
    let path = format!("{}/{}.sock", UAPI_SOCK_DIR, tun_name);
    let stream = UnixStream::connect(&path).map_err(|e| format!("UAPI connect: {}", e))?;
    stream.set_read_timeout(Some(UAPI_TIMEOUT)).ok();
    stream.set_write_timeout(Some(UAPI_TIMEOUT)).ok();

    let mut writer = std::io::BufWriter::new(&stream);
    write!(writer, "set=1\npublic_key={}\nremove=true\n\n", pubkey_hex)
        .map_err(|e| format!("UAPI write: {}", e))?;
    writer.flush().map_err(|e| format!("UAPI flush: {}", e))?;
    drop(writer);

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader.read_line(&mut response).map_err(|e| format!("UAPI read: {}", e))?;

    Ok(response.trim() == "errno=0")
}

// === Registration Handler ===

fn handle_registration(state: &RegistrationState, pubkey_base64: &str) -> (u16, String) {
    // Input validation: decode base64, verify 32 bytes
    let pubkey_bytes = match base64::decode(pubkey_base64.trim()) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        Ok(bytes) => {
            tracing::warn!("Registration rejected: invalid key length {} bytes", bytes.len());
            return (400, r#"{"error":"public key must be exactly 32 bytes"}"#.into());
        }
        Err(_) => {
            tracing::warn!("Registration rejected: invalid base64");
            return (400, r#"{"error":"invalid base64 encoding"}"#.into());
        }
    };
    let pubkey_hex = hex::encode(&pubkey_bytes);

    // Lock held across entire operation (P0-2 fix: no race condition)
    let mut inner = state.inner.lock().unwrap();

    // Lazy-populate server info from UAPI
    if inner.server_pubkey_cache.is_none() {
        match uapi_get_peers(&state.tun_name) {
            Ok((Some(pk), port, _)) => {
                inner.server_pubkey_cache = Some(pk);
                inner.listen_port_cache = port;
            }
            Ok((None, _, _)) => {
                return (503, r#"{"error":"server not configured yet"}"#.into());
            }
            Err(e) => {
                tracing::error!("UAPI query failed: {}", e);
                return (500, format!(r#"{{"error":"internal error: {}"}}"#, e));
            }
        }
    }

    // P0-1 fix: check if peer already exists (prevents update_peer panic)
    if inner.registered_peers.contains(&pubkey_hex) {
        tracing::info!(pubkey = %pubkey_hex, "Peer already registered (idempotent)");
        if let Some(&octet) = inner.peer_to_octet.get(&pubkey_hex) {
            let ip = Ipv4Addr::new(
                state.subnet_prefix[0],
                state.subnet_prefix[1],
                state.subnet_prefix[2],
                octet,
            );
            return (200, build_success_response(
                ip, &state.payment_config, &state.public_ip,
                inner.server_pubkey_cache.as_deref().unwrap_or(""),
                inner.listen_port_cache.unwrap_or(51820),
            ));
        }
    }

    // Allocate IP from pool
    let octet = match inner.available_ips.iter().next().copied() {
        Some(o) => o,
        None => {
            tracing::warn!("IP pool exhausted");
            return (503, r#"{"error":"no addresses available"}"#.into());
        }
    };
    let assigned_ip = Ipv4Addr::new(
        state.subnet_prefix[0],
        state.subnet_prefix[1],
        state.subnet_prefix[2],
        octet,
    );

    // UAPI set=1 (while mutex held — P0-2 fix)
    match uapi_set_peer(&state.tun_name, &pubkey_hex, assigned_ip) {
        Ok(()) => {
            inner.available_ips.remove(&octet);
            inner.registered_peers.insert(pubkey_hex.clone());
            inner.peer_to_octet.insert(pubkey_hex.clone(), octet);
            inner.peer_registered_at.insert(pubkey_hex.clone(), Instant::now());

            tracing::info!(
                pubkey = %pubkey_hex,
                ip = %assigned_ip,
                pool_remaining = inner.available_ips.len(),
                "Peer registered"
            );

            (200, build_success_response(
                assigned_ip, &state.payment_config, &state.public_ip,
                inner.server_pubkey_cache.as_deref().unwrap_or(""),
                inner.listen_port_cache.unwrap_or(51820),
            ))
        }
        Err(e) => {
            tracing::warn!(pubkey = %pubkey_hex, error = %e, "UAPI set_peer failed");
            (500, format!(r#"{{"error":"internal error: {}"}}"#, e))
        }
    }
}

fn build_success_response(
    ip: Ipv4Addr,
    pc: &PaymentConfigSnapshot,
    public_ip: &str,
    server_pubkey: &str,
    listen_port: u16,
) -> String {
    format!(
        r#"{{"status":"ok","server_public_key":"{}","endpoint":"{}:{}","assigned_ip":"{}/32","chain_id":{},"gateway_wallet":"0x{}","usdc_contract":"0x{}","amount_per_quota":{},"quota_bytes":{}}}"#,
        server_pubkey,
        public_ip,
        listen_port,
        ip,
        pc.chain_id,
        hex::encode(pc.gateway_wallet),
        hex::encode(pc.usdc_contract),
        pc.amount_per_quota,
        pc.quota_bytes,
    )
}

fn extract_pubkey(body: &str) -> Option<String> {
    // Try JSON first
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        if let Some(pk) = v.get("public_key").and_then(|v| v.as_str()) {
            return Some(pk.to_string());
        }
    }
    None
}

// === HTTP Server ===

pub fn run_http_server(
    state: Arc<RegistrationState>,
    bind_addr: &str,
    shutdown_flag: Arc<AtomicBool>,
) {
    let server = match tiny_http::Server::http(bind_addr) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to bind HTTP server on {}: {}", bind_addr, e);
            return;
        }
    };
    tracing::info!("Registration API listening on {}", bind_addr);

    loop {
        if shutdown_flag.load(Ordering::Relaxed) {
            break;
        }

        let mut request = match server.recv_timeout(Duration::from_secs(1)) {
            Ok(Some(req)) => req,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("HTTP accept error: {}", e);
                continue;
            }
        };

        // P0-3 fix: enforce body size limit
        if request.body_length().unwrap_or(0) > MAX_BODY_BYTES {
            let resp = tiny_http::Response::from_string(r#"{"error":"request body too large"}"#)
                .with_status_code(413);
            let _ = request.respond(resp);
            continue;
        }

        // Rate limiting (P1-4)
        if let Some(addr) = request.remote_addr() {
            let ip = addr.ip();
            let mut rl = state.rate_limiter.lock().unwrap();
            let bucket = rl.entry(ip).or_insert_with(TokenBucket::new);
            if !bucket.try_consume() {
                tracing::warn!(source = %ip, "Rate limited");
                let resp = tiny_http::Response::from_string(r#"{"error":"rate limited"}"#)
                    .with_status_code(429);
                let _ = request.respond(resp);
                continue;
            }
        }

        let (status, body) = match (request.method(), request.url()) {
            (tiny_http::Method::Get, "/health") => {
                let inner = state.inner.lock().unwrap();
                (
                    200u16,
                    format!(
                        r#"{{"pool_available":{},"peer_count":{}}}"#,
                        inner.available_ips.len(),
                        inner.registered_peers.len(),
                    ),
                )
            }
            (tiny_http::Method::Post, "/v1/register") => {
                let mut body_buf = Vec::new();
                let reader = request.as_reader();
                match reader.take(MAX_BODY_BYTES as u64 + 1).read_to_end(&mut body_buf) {
                    Ok(n) if n > MAX_BODY_BYTES => {
                        (413u16, r#"{"error":"request body too large"}"#.into())
                    }
                    Ok(_) => {
                        let body_str = String::from_utf8_lossy(&body_buf);
                        match extract_pubkey(&body_str) {
                            Some(pk) => handle_registration(&state, &pk),
                            None => (400, r#"{"error":"missing public_key field"}"#.into()),
                        }
                    }
                    Err(_) => (400, r#"{"error":"failed to read body"}"#.into()),
                }
            }
            _ => (404, r#"{"error":"not found"}"#.into()),
        };

        let resp = tiny_http::Response::from_string(&body)
            .with_status_code(status)
            .with_header(
                tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
            );
        let _ = request.respond(resp);
    }
}

// === Reaper Thread ===

pub fn run_reaper(state: Arc<RegistrationState>, shutdown_flag: Arc<AtomicBool>) {
    tracing::info!(
        "Peer reaper started (interval={}s, stale={}s)",
        REAPER_INTERVAL_SECS,
        STALE_PEER_SECS
    );

    loop {
        std::thread::sleep(Duration::from_secs(REAPER_INTERVAL_SECS));
        if shutdown_flag.load(Ordering::Relaxed) {
            break;
        }

        // Query current peers from UAPI
        let (_pubkey, _port, uapi_peers) = match uapi_get_peers(&state.tun_name) {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!("Reaper: UAPI query failed: {}", e);
                continue;
            }
        };

        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let now_instant = Instant::now();

        let mut inner = state.inner.lock().unwrap();

        // Debug: dump UAPI keys vs registered keys for comparison
        let uapi_keys: Vec<&String> = uapi_peers.keys().collect();
        let reg_keys: Vec<&String> = inner.peer_to_octet.keys().collect();
        tracing::info!(
            uapi_count = uapi_peers.len(),
            reg_count = inner.peer_to_octet.len(),
            uapi_keys = ?uapi_keys,
            reg_keys = ?reg_keys,
            "Reaper: UAPI vs registered"
        );

        // Identify stale peers
        let mut to_remove = Vec::new();
        for (pubkey_hex, &octet) in inner.peer_to_octet.iter() {
            let uapi_lookup = uapi_peers.get(pubkey_hex);
            tracing::info!(
                reg_key = %pubkey_hex,
                found_in_uapi = uapi_lookup.is_some(),
                "Reaper: lookup result"
            );
            match uapi_lookup {
                Some(Some(handshake_sec)) if *handshake_sec > 0 => {
                    // handshake_sec is a Duration (seconds since last handshake), NOT an epoch timestamp
                    let age = *handshake_sec;
                    tracing::info!(age_secs = age, threshold = STALE_PEER_SECS, "Reaper: has handshake");
                    if age > STALE_PEER_SECS {
                        to_remove.push((pubkey_hex.clone(), octet));
                    }
                }
                Some(Some(_)) | Some(None) => {
                    if let Some(registered_at) = inner.peer_registered_at.get(pubkey_hex) {
                        let age = now_instant.duration_since(*registered_at).as_secs();
                        tracing::info!(age_secs = age, threshold = STALE_NEVER_CONNECTED_SECS, "Reaper: never handshaked");
                        if age > STALE_NEVER_CONNECTED_SECS {
                            to_remove.push((pubkey_hex.clone(), octet));
                        }
                    }
                }
                None => {
                    tracing::warn!(reg_key = %pubkey_hex, "Reaper: NOT FOUND in UAPI — will remove");
                    to_remove.push((pubkey_hex.clone(), octet));
                }
            }
        }

        // Remove stale peers
        for (pubkey_hex, octet) in &to_remove {
            // P1-B fix: only commit state if UAPI removal succeeds
            match uapi_remove_peer(&state.tun_name, pubkey_hex) {
                Ok(true) => {
                    // errno=0 — safe to update state
                    inner.available_ips.insert(*octet);
                    inner.registered_peers.remove(pubkey_hex);
                    inner.peer_to_octet.remove(pubkey_hex);
                    inner.peer_registered_at.remove(pubkey_hex);
                    tracing::info!(
                        pubkey = %pubkey_hex,
                        octet = octet,
                        pool_remaining = inner.available_ips.len(),
                        "Reaper: removed stale peer"
                    );
                }
                Ok(false) => {
                    // UAPI returned non-zero errno — don't touch state, retry next cycle
                    tracing::warn!(
                        pubkey = %pubkey_hex,
                        "Reaper: UAPI remove returned error, skipping state update"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        pubkey = %pubkey_hex,
                        error = %e,
                        "Reaper: UAPI remove failed, will retry"
                    );
                }
            }
        }

        // Prune old rate limiter entries (P2-A fix)
        {
            let mut rl = state.rate_limiter.lock().unwrap();
            rl.retain(|_, bucket| {
                bucket.last_refill.elapsed().as_secs() < RATE_LIMITER_PRUNE_SECS
            });
        }

        if !to_remove.is_empty() {
            tracing::info!("Reaper: processed {} stale peers", to_remove.len());
        }
    }
}
