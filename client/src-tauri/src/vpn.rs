/// VPN connection manager.
///
/// Single connect/disconnect lifecycle:
///  1. Generate WireGuard key pair
///  2. Register with server API → get assigned IP + server pubkey
///  3. Launch boringtun-cli subprocess
///  4. Poll until TUN interface is ready
///  5. Configure WireGuard peer (wg set)
///  6. Save original gateway + DNS
///  7. Configure full-tunnel routing
///  8. Set DNS
///
/// On disconnect: reverse all steps, restore original state.
/// On any failure: rollback all completed steps automatically.
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hkdf::Hkdf;
use k256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sha3::{Digest, Keccak256};
use tauri::Emitter;
use tokio_util::sync::CancellationToken;
use x25519_dalek::{PublicKey, StaticSecret};

// ── Server config ────────────────────────────────────────────────────────────

const API_BASE: &str = "http://37.27.29.160:8080";
const SERVER_IP: &str = "37.27.29.160";
const RPC_URL: &str = "https://rpc.testnet.arc.network";
const GATEWAY_WALLET: &str = "0077777d7EBA4688BDeF3E311b846F25870A19B9";
const USDC_CONTRACT: &str = "3600000000000000000000000000000000000000";

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectedInfo {
    pub assigned_ip: String,
    pub server_endpoint: String,
    pub wallet_address: String,
    pub gateway_balance: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VpnStatus {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct VpnStateEvent {
    pub status: VpnStatus,
    pub assigned_ip: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthEvent {
    pub connected: bool,
    pub process_alive: bool,
    pub handshake_age_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RegisterResponse {
    server_public_key: String,
    endpoint: String,
    assigned_ip: String,
}

/// Saved system state for restoring on disconnect.
struct ConnectionContext {
    iface: String,
    boringtun_proc: Child,
    original_gateway: String,
    #[allow(dead_code)]
    original_phys_iface: String,
    original_dns: OriginalDns,
    server_ip: String,
    health_cancel: CancellationToken,
}

#[cfg(target_os = "macos")]
struct OriginalDns {
    service: String,
    servers: Vec<String>,
}

#[cfg(target_os = "linux")]
struct OriginalDns {
    resolv_conf_backup: String,
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
struct OriginalDns;

// ── VpnManager ───────────────────────────────────────────────────────────────

pub struct VpnManager {
    ctx: Option<ConnectionContext>,
}

impl VpnManager {
    pub fn new() -> Self {
        VpnManager { ctx: None }
    }

    pub fn is_connected(&self) -> bool {
        self.ctx.is_some()
    }

    pub fn status(&self) -> VpnStatus {
        if self.ctx.is_some() {
            VpnStatus::Connected
        } else {
            VpnStatus::Disconnected
        }
    }

    pub async fn connect(
        &mut self,
        app_handle: tauri::AppHandle,
    ) -> Result<ConnectedInfo, String> {
        if self.ctx.is_some() {
            return Err("Already connected".into());
        }

        // ── 1. Generate key pair + derive wallet ───────────────────────────
        let private = StaticSecret::random_from_rng(rand_core::OsRng);
        let public = PublicKey::from(&private);
        let priv_b64 = B64.encode(private.as_bytes());
        let pub_b64 = B64.encode(public.as_bytes());

        let wallet_address = derive_evm_address(private.as_bytes());
        log::info!("[vpn] derived wallet: {wallet_address}");

        // ── 2. Register with server ────────────────────────────────────────
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("HTTP client error: {e}"))?;

        let body = serde_json::json!({ "public_key": pub_b64 });

        let resp = client
            .post(format!("{API_BASE}/v1/register"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to reach server: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Server returned {status}: {text}"));
        }

        let reg: RegisterResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse register response: {e}"))?;

        let server_pub = reg.server_public_key;
        let assigned_ip = reg.assigned_ip.clone(); // e.g. "10.0.0.185/32"
        let endpoint = reg.endpoint; // e.g. "37.27.29.160:51820"
        let ip_bare = assigned_ip
            .split('/')
            .next()
            .unwrap_or(&assigned_ip)
            .to_string();

        log::info!("[vpn] registered: server_pub={server_pub} ip={assigned_ip} endpoint={endpoint}");

        // ── 3. Launch boringtun-cli ────────────────────────────────────────
        kill_stale_boringtun();
        let iface = find_available_iface()?;
        let boringtun_path = boringtun_binary()?;

        let proc = Command::new("sudo")
            .arg(&boringtun_path)
            .arg(&iface)
            .arg("--disable-drop-privileges")
            .arg("--foreground")
            .spawn()
            .map_err(|e| format!("Failed to start boringtun-cli: {e}"))?;

        // From here, every error must clean up previous steps.
        let mut cleanup = Cleanup::new();
        let iface_clone = iface.clone();
        cleanup.push("kill boringtun", move || {
            let _ = Command::new("sudo")
                .args(["pkill", "-f", "boringtun-cli"])
                .status();
            std::thread::sleep(Duration::from_millis(200));
        });

        // ── 4. Wait for interface ──────────────────────────────────────────
        if let Err(e) = wait_for_interface(&iface, Duration::from_secs(5)).await {
            cleanup.run();
            return Err(e);
        }

        // ── 5. Configure WireGuard peer ────────────────────────────────────
        if let Err(e) = configure_wireguard(&iface, &priv_b64, &server_pub, &endpoint) {
            cleanup.run();
            return Err(e);
        }

        // ── 6. Set interface IP and bring up ───────────────────────────────
        if let Err(e) = configure_interface_ip(&iface, &ip_bare, &assigned_ip) {
            cleanup.run();
            return Err(e);
        }

        let iface_clone2 = iface.clone();
        cleanup.push("teardown interface", move || {
            teardown_interface(&iface_clone2);
        });

        // ── 7. Save original gateway + DNS ─────────────────────────────────
        let original_gw = match get_default_gateway() {
            Ok(gw) => gw,
            Err(e) => {
                cleanup.run();
                return Err(format!("Failed to get default gateway: {e}"));
            }
        };

        let original_dns = match save_original_dns() {
            Ok(dns) => dns,
            Err(e) => {
                cleanup.run();
                return Err(format!("Failed to save DNS state: {e}"));
            }
        };

        #[cfg(target_os = "macos")]
        let original_phys_iface = get_default_interface().unwrap_or_else(|_| "en0".into());
        #[cfg(not(target_os = "macos"))]
        let original_phys_iface = String::new();

        log::info!("[vpn] original gateway={original_gw} iface={original_phys_iface}");

        // ── 8. Configure full-tunnel routing ───────────────────────────────
        if let Err(e) = configure_full_tunnel(&iface, SERVER_IP, &original_gw) {
            cleanup.run();
            return Err(e);
        }

        let iface_clone3 = iface.clone();
        let gw_clone = original_gw.clone();
        cleanup.push("teardown routes", move || {
            teardown_routes(&iface_clone3, SERVER_IP, &gw_clone);
        });

        // ── 9. Set DNS ─────────────────────────────────────────────────────
        if let Err(e) = set_vpn_dns() {
            cleanup.run();
            return Err(e);
        }

        cleanup.push("restore DNS", move || {
            let _ = restore_dns_best_effort();
        });

        // ── Success: disarm cleanup, store context ─────────────────────────
        cleanup.disarm();

        let health_cancel = CancellationToken::new();
        start_health_check(app_handle, iface.clone(), health_cancel.clone());

        self.ctx = Some(ConnectionContext {
            iface,
            boringtun_proc: proc,
            original_gateway: original_gw,
            original_phys_iface,
            original_dns,
            server_ip: SERVER_IP.to_string(),
            health_cancel,
        });

        // ── 10. Query gateway balance ──────────────────────────────────────
        let gateway_balance = query_gateway_balance(&wallet_address)
            .await
            .unwrap_or_else(|e| {
                log::warn!("[vpn] balance query failed: {e}");
                "0.000000".to_string()
            });

        Ok(ConnectedInfo {
            assigned_ip: ip_bare,
            server_endpoint: endpoint,
            wallet_address,
            gateway_balance,
        })
    }

    pub fn disconnect(&mut self) -> Result<(), String> {
        let mut ctx = self
            .ctx
            .take()
            .ok_or_else(|| "Not connected".to_string())?;

        log::info!("[vpn] disconnecting...");

        // Stop health check
        ctx.health_cancel.cancel();

        // Restore DNS first (while we still have connectivity context)
        let _ = restore_dns(&ctx.original_dns);

        // Remove full-tunnel routes
        teardown_routes(&ctx.iface, &ctx.server_ip, &ctx.original_gateway);

        // Tear down interface
        teardown_interface(&ctx.iface);

        // Kill boringtun
        let _ = ctx.boringtun_proc.kill();
        let _ = Command::new("sudo")
            .args(["pkill", "-f", "boringtun-cli"])
            .status();

        log::info!("[vpn] disconnected");
        Ok(())
    }

    /// Check if tunnel is healthy. Used by health check loop.
    pub fn check_health(&mut self) -> HealthEvent {
        let ctx = match &mut self.ctx {
            Some(c) => c,
            None => {
                return HealthEvent {
                    connected: false,
                    process_alive: false,
                    handshake_age_secs: None,
                }
            }
        };

        let process_alive = match ctx.boringtun_proc.try_wait() {
            Ok(None) => true,  // still running
            Ok(Some(_)) => false, // exited
            Err(_) => false,
        };

        let handshake_age = get_handshake_age(&ctx.iface);

        let connected = process_alive && handshake_age.map(|a| a < 300).unwrap_or(false);

        HealthEvent {
            connected,
            process_alive,
            handshake_age_secs: handshake_age,
        }
    }
}

// ── Cleanup guard ────────────────────────────────────────────────────────────

struct Cleanup {
    steps: Vec<(&'static str, Box<dyn FnOnce() + Send>)>,
    armed: bool,
}

impl Cleanup {
    fn new() -> Self {
        Cleanup {
            steps: Vec::new(),
            armed: true,
        }
    }

    fn push<F: FnOnce() + Send + 'static>(&mut self, name: &'static str, f: F) {
        self.steps.push((name, Box::new(f)));
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn run(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        for (name, step) in self.steps.drain(..).rev() {
            log::info!("[vpn] cleanup: {name}");
            step();
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.armed {
            self.run();
        }
    }
}

// ── Health check ─────────────────────────────────────────────────────────────

fn start_health_check(
    app_handle: tauri::AppHandle,
    iface: String,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        // Grace period after connect
        tokio::time::sleep(Duration::from_secs(5)).await;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(3)) => {
                    let process_alive = Command::new("pgrep")
                        .args(["-f", "boringtun-cli"])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);

                    let handshake_age = get_handshake_age(&iface);
                    let connected = process_alive
                        && handshake_age.map(|a| a < 300).unwrap_or(false);

                    let event = HealthEvent {
                        connected,
                        process_alive,
                        handshake_age_secs: handshake_age,
                    };

                    let _ = app_handle.emit("vpn-health", &event);

                    if !process_alive {
                        let _ = app_handle.emit("vpn-state", &VpnStateEvent {
                            status: VpnStatus::Error,
                            assigned_ip: None,
                            error: Some("boringtun process died".into()),
                        });
                        break;
                    }
                }
            }
        }
    });
}

fn get_handshake_age(iface: &str) -> Option<u64> {
    let output = Command::new("sudo")
        .args(["wg", "show", iface, "latest-handshakes"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    // Format: "<pubkey>\t<unix_timestamp>\n"
    let ts_str = text.split_whitespace().nth(1)?;
    let ts: u64 = ts_str.parse().ok()?;

    if ts == 0 {
        return None; // No handshake yet
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    Some(now.saturating_sub(ts))
}

// ── WireGuard configuration ─────────────────────────────────────────────────

fn configure_wireguard(
    iface: &str,
    priv_key: &str,
    server_pub: &str,
    endpoint: &str,
) -> Result<(), String> {
    let key_file = format!("/tmp/vpntee_{iface}.key");
    std::fs::write(&key_file, priv_key)
        .map_err(|e| format!("Failed to write key file: {e}"))?;

    let result = run_sudo(&[
        "wg",
        "set",
        iface,
        "private-key",
        &key_file,
        "peer",
        server_pub,
        "allowed-ips",
        "0.0.0.0/0",
        "endpoint",
        endpoint,
        "persistent-keepalive",
        "25",
    ]);

    let _ = std::fs::remove_file(&key_file);
    result
}

// ── Interface IP configuration ───────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn configure_interface_ip(iface: &str, ip_bare: &str, _assigned_ip: &str) -> Result<(), String> {
    run_sudo(&["ifconfig", iface, ip_bare, ip_bare, "up"])
}

#[cfg(target_os = "linux")]
fn configure_interface_ip(iface: &str, _ip_bare: &str, assigned_ip: &str) -> Result<(), String> {
    let ip_cidr = if assigned_ip.contains('/') {
        assigned_ip.to_string()
    } else {
        format!("{assigned_ip}/32")
    };
    run_sudo(&["ip", "addr", "add", &ip_cidr, "dev", iface])?;
    run_sudo(&["ip", "link", "set", iface, "up"])
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn configure_interface_ip(_: &str, _: &str, _: &str) -> Result<(), String> {
    Err("Unsupported platform".into())
}

// ── Full-tunnel routing ──────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn configure_full_tunnel(iface: &str, server_ip: &str, original_gw: &str) -> Result<(), String> {
    // Find the physical interface for the default route (e.g. en0)
    let phys_iface = get_default_interface().unwrap_or_else(|_| "en0".to_string());

    // 1. Explicit route for server via original gateway on physical interface
    //    This MUST come before the split routes so WG UDP packets bypass the tunnel
    run_sudo(&[
        "route", "add", "-host", server_ip, original_gw,
    ])?;
    // 2. Split default: 0.0.0.0/1 + 128.0.0.0/1 override default without deleting it
    run_sudo(&["route", "add", "-net", "0.0.0.0/1", "-interface", iface])?;
    run_sudo(&["route", "add", "-net", "128.0.0.0/1", "-interface", iface])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn configure_full_tunnel(iface: &str, _server_ip: &str, _original_gw: &str) -> Result<(), String> {
    // fwmark approach (same as wg-quick)
    run_sudo(&["wg", "set", iface, "fwmark", "51820"])?;
    run_sudo(&["ip", "rule", "add", "not", "fwmark", "51820", "table", "51820"])?;
    run_sudo(&["ip", "route", "add", "default", "dev", iface, "table", "51820"])?;
    run_sudo(&[
        "ip", "rule", "add", "table", "main", "suppress_prefixlength", "0",
    ])?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn configure_full_tunnel(_: &str, _: &str, _: &str) -> Result<(), String> {
    Err("Unsupported platform".into())
}

#[cfg(target_os = "macos")]
fn teardown_routes(iface: &str, server_ip: &str, _original_gw: &str) {
    let _ = run_sudo(&["route", "delete", "-net", "0.0.0.0/1", "-interface", iface]);
    let _ = run_sudo(&["route", "delete", "-net", "128.0.0.0/1", "-interface", iface]);
    let _ = run_sudo(&["route", "delete", "-host", server_ip]);
}

#[cfg(target_os = "linux")]
fn teardown_routes(iface: &str, _server_ip: &str, _original_gw: &str) {
    let _ = run_sudo(&["ip", "rule", "delete", "not", "fwmark", "51820", "table", "51820"]);
    let _ = run_sudo(&["ip", "rule", "delete", "table", "main", "suppress_prefixlength", "0"]);
    let _ = run_sudo(&["ip", "route", "flush", "table", "51820"]);
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn teardown_routes(_: &str, _: &str, _: &str) {}

// ── Default gateway detection ────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn get_default_gateway() -> Result<String, String> {
    let output = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .map_err(|e| format!("route -n get default failed: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("gateway:") {
            return Ok(trimmed
                .strip_prefix("gateway:")
                .unwrap()
                .trim()
                .to_string());
        }
    }
    Err("Could not determine default gateway".into())
}

#[cfg(target_os = "macos")]
fn get_default_interface() -> Result<String, String> {
    let output = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .map_err(|e| format!("route -n get default failed: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("interface:") {
            return Ok(trimmed
                .strip_prefix("interface:")
                .unwrap()
                .trim()
                .to_string());
        }
    }
    Err("Could not determine default interface".into())
}

#[cfg(target_os = "linux")]
fn get_default_gateway() -> Result<String, String> {
    let output = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .map_err(|e| format!("ip route show default failed: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    // "default via 192.168.1.1 dev eth0 ..."
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() >= 3 && parts[0] == "default" && parts[1] == "via" {
        return Ok(parts[2].to_string());
    }
    Err("Could not determine default gateway".into())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn get_default_gateway() -> Result<String, String> {
    Err("Unsupported platform".into())
}

// ── DNS management ───────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn get_primary_network_service() -> Result<String, String> {
    // Find the interface used for default route
    let output = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .map_err(|e| format!("route failed: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    let mut iface = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("interface:") {
            iface = Some(
                trimmed
                    .strip_prefix("interface:")
                    .unwrap()
                    .trim()
                    .to_string(),
            );
            break;
        }
    }

    let iface = iface.ok_or("Could not find default interface")?;

    // Map interface to network service name
    let output = Command::new("networksetup")
        .args(["-listallhardwareports"])
        .output()
        .map_err(|e| format!("networksetup failed: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    let mut current_service = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("Hardware Port: ") {
            current_service = name.to_string();
        }
        if let Some(dev) = line.strip_prefix("Device: ") {
            if dev.trim() == iface {
                return Ok(current_service);
            }
        }
    }

    // Fallback
    Ok("Wi-Fi".to_string())
}

#[cfg(target_os = "macos")]
fn save_original_dns() -> Result<OriginalDns, String> {
    let service = get_primary_network_service()?;

    let output = Command::new("networksetup")
        .args(["-getdnsservers", &service])
        .output()
        .map_err(|e| format!("networksetup -getdnsservers failed: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let servers = if text.contains("any DNS Servers set") {
        vec![] // no custom DNS configured
    } else {
        text.lines().map(|l| l.trim().to_string()).collect()
    };

    log::info!("[vpn] saved DNS for service={service}: {servers:?}");

    Ok(OriginalDns { service, servers })
}

#[cfg(target_os = "macos")]
fn set_vpn_dns() -> Result<(), String> {
    let service = get_primary_network_service()?;
    // Use public DNS that will be routed through the tunnel
    run_sudo(&[
        "networksetup",
        "-setdnsservers",
        &service,
        "1.1.1.1",
        "8.8.8.8",
    ])
}

#[cfg(target_os = "macos")]
fn restore_dns(dns: &OriginalDns) -> Result<(), String> {
    if dns.servers.is_empty() {
        // Reset to DHCP-provided DNS
        run_sudo(&["networksetup", "-setdnsservers", &dns.service, "Empty"])
    } else {
        let mut args = vec!["networksetup", "-setdnsservers", &dns.service];
        let refs: Vec<&str> = dns.servers.iter().map(|s| s.as_str()).collect();
        args.extend(refs);
        run_sudo(&args)
    }
}

#[cfg(target_os = "macos")]
fn restore_dns_best_effort() -> Result<(), String> {
    // Used during cleanup when we may not have the original context
    let service = get_primary_network_service().unwrap_or_else(|_| "Wi-Fi".into());
    run_sudo(&["networksetup", "-setdnsservers", &service, "Empty"])
}

#[cfg(target_os = "linux")]
fn save_original_dns() -> Result<OriginalDns, String> {
    let content = std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default();
    Ok(OriginalDns {
        resolv_conf_backup: content,
    })
}

#[cfg(target_os = "linux")]
fn set_vpn_dns() -> Result<(), String> {
    // Write DNS through resolvconf if available, otherwise direct write
    let output = Command::new("which")
        .arg("resolvconf")
        .output();

    if output.map(|o| o.status.success()).unwrap_or(false) {
        let child = Command::new("sudo")
            .args(["resolvconf", "-a", "tun.vpntee"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("resolvconf failed: {e}"))?;

        if let Some(mut stdin) = child.stdin {
            use std::io::Write;
            let _ = write!(stdin, "nameserver 1.1.1.1\nnameserver 8.8.8.8\n");
        }
    } else {
        std::fs::write(
            "/etc/resolv.conf",
            "# Set by vpntee\nnameserver 1.1.1.1\nnameserver 8.8.8.8\n",
        )
        .map_err(|e| format!("Failed to write resolv.conf: {e}"))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn restore_dns(dns: &OriginalDns) -> Result<(), String> {
    if !dns.resolv_conf_backup.is_empty() {
        std::fs::write("/etc/resolv.conf", &dns.resolv_conf_backup)
            .map_err(|e| format!("Failed to restore resolv.conf: {e}"))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn restore_dns_best_effort() -> Result<(), String> {
    // Try resolvconf first
    let _ = Command::new("sudo")
        .args(["resolvconf", "-d", "tun.vpntee"])
        .status();
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn save_original_dns() -> Result<OriginalDns, String> {
    Ok(OriginalDns)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn set_vpn_dns() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn restore_dns(_: &OriginalDns) -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn restore_dns_best_effort() -> Result<(), String> {
    Ok(())
}

// ── Interface readiness polling ──────────────────────────────────────────────

async fn wait_for_interface(iface: &str, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if interface_exists(iface) {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(format!(
                "Interface {iface} not ready after {}s",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(target_os = "macos")]
fn interface_exists(iface: &str) -> bool {
    Command::new("ifconfig")
        .arg(iface)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn interface_exists(iface: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{iface}")).exists()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn interface_exists(_: &str) -> bool {
    false
}

// ── Interface teardown ───────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn teardown_interface(iface: &str) {
    let _ = run_sudo(&["ifconfig", iface, "down"]);
    let _ = run_sudo(&["ifconfig", iface, "destroy"]);
}

#[cfg(target_os = "linux")]
fn teardown_interface(iface: &str) {
    let _ = run_sudo(&["ip", "link", "delete", iface]);
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn teardown_interface(_: &str) {}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn boringtun_binary() -> Result<String, String> {
    let candidates = [
        "./boringtun-cli",
        "/usr/local/bin/boringtun-cli",
        "/opt/homebrew/bin/boringtun-cli",
        "../../target/release/boringtun-cli",
    ];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    let output = Command::new("which")
        .arg("boringtun-cli")
        .output()
        .map_err(|_| "boringtun-cli not found".to_string())?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err("boringtun-cli not found in PATH or known locations".into())
    }
}

#[cfg(target_os = "macos")]
fn find_available_iface() -> Result<String, String> {
    let output = Command::new("ifconfig")
        .arg("-l")
        .output()
        .map_err(|e| format!("ifconfig -l failed: {e}"))?;
    let existing = String::from_utf8_lossy(&output.stdout);

    for i in 9..=30 {
        let name = format!("utun{i}");
        if !existing.split_whitespace().any(|x| x == name) {
            return Ok(name);
        }
    }
    Err("No available utun interface found".into())
}

#[cfg(target_os = "linux")]
fn find_available_iface() -> Result<String, String> {
    let output = Command::new("ip")
        .args(["link", "show"])
        .output()
        .map_err(|e| format!("ip link show failed: {e}"))?;
    let existing = String::from_utf8_lossy(&output.stdout);

    for i in 0..=20 {
        let name = format!("wg{i}");
        if !existing.contains(&format!("{name}:")) {
            return Ok(name);
        }
    }
    Err("No available wg interface found".into())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn find_available_iface() -> Result<String, String> {
    Ok("wg0".to_string())
}

fn kill_stale_boringtun() {
    let _ = Command::new("sudo")
        .args(["pkill", "-f", "boringtun-cli"])
        .status();
    std::thread::sleep(Duration::from_millis(300));
}

fn run_sudo(args: &[&str]) -> Result<(), String> {
    log::info!("[vpn] sudo {}", args.join(" "));
    let output = Command::new("sudo")
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run sudo {:?}: {e}", args))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Command failed: sudo {} — {}",
            args.join(" "),
            stderr.trim()
        ))
    }
}

// ── Wallet derivation (same as server's payment::wallet) ─────────────────────

fn derive_evm_address(x25519_private_bytes: &[u8; 32]) -> String {
    let hk = Hkdf::<Sha256>::new(Some(b"boringtun-payment-v1"), x25519_private_bytes);
    let mut derived = [0u8; 32];
    hk.expand(b"secp256k1-signing-key", &mut derived)
        .expect("32 bytes is valid HKDF output");

    let signing_key =
        SigningKey::from_bytes((&derived).into()).expect("HKDF output is valid scalar");

    let verify_key = k256::ecdsa::VerifyingKey::from(&signing_key);
    let pubkey_point = verify_key.to_encoded_point(false);
    let pubkey_bytes = pubkey_point.as_bytes();
    // Skip 0x04 prefix, hash x||y (64 bytes)
    let hash = Keccak256::digest(&pubkey_bytes[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

// ── Gateway balance query ────────────────────────────────────────────────────

pub async fn query_gateway_balance(wallet_address: &str) -> Result<String, String> {
    let addr_hex = wallet_address
        .strip_prefix("0x")
        .unwrap_or(wallet_address);

    // balanceOf(address token, address account) selector = 0x3ccb64ae
    // token = USDC_CONTRACT, account = wallet_address
    let calldata = format!(
        "0x3ccb64ae000000000000000000000000{USDC_CONTRACT}000000000000000000000000{addr_hex}"
    );

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{
            "to": format!("0x{GATEWAY_WALLET}"),
            "data": calldata,
        }, "latest"],
        "id": 1
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp: serde_json::Value = client
        .post(RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("RPC request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("RPC parse failed: {e}"))?;

    if let Some(err) = resp.get("error") {
        return Err(format!("RPC error: {err}"));
    }

    let hex_str = resp["result"]
        .as_str()
        .ok_or("No result in RPC response")?;

    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let raw = u128::from_str_radix(hex_str, 16).unwrap_or(0);
    // USDC has 6 decimals
    let whole = raw / 1_000_000;
    let frac = raw % 1_000_000;
    Ok(format!("{whole}.{frac:06}"))
}
