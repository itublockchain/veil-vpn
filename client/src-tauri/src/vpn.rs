/// VPN connection manager.
///
/// Handles the full connect/disconnect lifecycle:
///  1. Generate WireGuard key pair
///  2. Fetch server public key via HTTP GET /pubkey
///  3. Register client public key via HTTP POST /register → get assigned VPN IP
///  4. Start the WS↔UDP proxy (proxy.rs)
///  5. Launch boringtun-cli as a subprocess
///  6. Configure the TUN interface (IP, routes)
///
/// On disconnect:
///  1. Kill boringtun-cli
///  2. Stop the proxy
///  3. Tear down TUN interface
use std::process::{Child, Command};
use x25519_dalek::{PublicKey, StaticSecret};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};

use crate::proxy::Proxy;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectedInfo {
    pub assigned_ip: String,
    pub server_name: String,
}

#[derive(Debug, Deserialize)]
struct RegisterResponse {
    server_public_key: String,
    endpoint: String,
    assigned_ip: String,
}

pub struct VpnManager {
    boringtun_proc: Option<Child>,
    proxy: Option<Proxy>,
    iface: Option<String>,
}

impl VpnManager {
    pub fn new() -> Self {
        VpnManager {
            boringtun_proc: None,
            proxy: None,
            iface: None,
        }
    }

    pub async fn connect(
        &mut self,
        server_name: String,
        api_url: String,
        ws_url: String,
    ) -> Result<ConnectedInfo, String> {
        // ── 1. Generate key pair ─────────────────────────────────────────────
        let private = StaticSecret::random_from_rng(rand_core::OsRng);
        let public = PublicKey::from(&private);
        let priv_b64 = B64.encode(private.as_bytes());
        let pub_b64 = B64.encode(public.as_bytes());

        // ── 2. Fetch server public key ───────────────────────────────────────
        let client = reqwest::Client::new();
        let server_pub = client
            .get(format!("{api_url}/pubkey"))
            .send()
            .await
            .map_err(|e| format!("Failed to reach server: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Failed to read server pubkey: {e}"))?;

        let server_pub = server_pub.trim().to_string();
        if server_pub.is_empty() {
            return Err("Server returned empty public key".into());
        }

        // ── 3. Register client, get assigned IP ─────────────────────────────
        let assigned_ip = client
            .post(format!("{api_url}/register"))
            .body(pub_b64.clone())
            .send()
            .await
            .map_err(|e| format!("Failed to register: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Failed to read assigned IP: {e}"))?;

        let assigned_ip = assigned_ip.trim().to_string();
        if assigned_ip.is_empty() {
            return Err("Server returned empty IP assignment".into());
        }

        log::info!("[vpn] server_pub={server_pub} assigned_ip={assigned_ip}");

        // ── 4. Start WS↔UDP proxy ────────────────────────────────────────────
        let proxy = Proxy::start(ws_url).await?;
        self.proxy = Some(proxy);

        // ── 5. Launch boringtun-cli ──────────────────────────────────────────
        kill_stale_boringtun();
        let iface = find_available_iface()?;
        self.iface = Some(iface.clone());

        let boringtun_path = boringtun_binary()?;

        let proc = Command::new("sudo")
            .arg(&boringtun_path)
            .arg(&iface)
            .arg("--disable-drop-privileges")
            .arg("--foreground")
            .spawn()
            .map_err(|e| format!("Failed to start boringtun-cli: {e}"))?;

        self.boringtun_proc = Some(proc);

        // Give boringtun a moment to create the interface
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // ── 6. Configure interface ───────────────────────────────────────────
        configure_interface(&iface, &priv_b64, &server_pub, &assigned_ip, "127.0.0.1:51820")?;

        Ok(ConnectedInfo {
            assigned_ip,
            server_name,
        })
    }

    /// Connect directly to the server without WS proxy.
    /// Generates a key pair, registers with the server API, gets back
    /// server_public_key + assigned_ip + endpoint, then launches boringtun-cli
    /// and configures the TUN interface + peer.
    pub async fn connect_no_tunnel(&mut self) -> Result<ConnectedInfo, String> {
        const REGISTER_URL: &str = "http://37.27.29.160:8080/v1/register";

        // ── 1. Generate key pair ─────────────────────────────────────────────
        let private = StaticSecret::random_from_rng(rand_core::OsRng);
        let public = PublicKey::from(&private);
        let priv_b64 = B64.encode(private.as_bytes());
        let pub_b64 = B64.encode(public.as_bytes());

        // ── 2. Register with server ──────────────────────────────────────────
        let client = reqwest::Client::new();
        let body = serde_json::json!({ "public_key": pub_b64 });

        let resp = client
            .post(REGISTER_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to reach server: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Server returned {}", resp.status()));
        }

        let reg: RegisterResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse register response: {e}"))?;

        let server_pub = reg.server_public_key;
        let assigned_ip = reg.assigned_ip; // e.g. "10.0.0.185/32"
        let endpoint = reg.endpoint;       // e.g. "37.27.29.160:51820"

        log::info!("[vpn] registered: server_pub={server_pub} ip={assigned_ip} endpoint={endpoint}");

        // ── 3. Launch boringtun-cli ──────────────────────────────────────────
        kill_stale_boringtun();
        let iface = find_available_iface()?;
        self.iface = Some(iface.clone());

        let boringtun_path = boringtun_binary()?;
        let proc = Command::new("sudo")
            .arg(&boringtun_path)
            .arg(&iface)
            .arg("--disable-drop-privileges")
            .arg("--foreground")
            .spawn()
            .map_err(|e| format!("Failed to start boringtun-cli: {e}"))?;

        self.boringtun_proc = Some(proc);

        // Give boringtun a moment to create the interface
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // ── 4. Configure interface + peer ────────────────────────────────────
        configure_interface(&iface, &priv_b64, &server_pub, &assigned_ip, &endpoint)?;

        let ip_display = assigned_ip.split('/').next().unwrap_or(&assigned_ip).to_string();

        Ok(ConnectedInfo {
            assigned_ip: ip_display,
            server_name: "Direct (No Tunnel)".to_string(),
        })
    }

    pub fn disconnect(&mut self) -> Result<(), String> {
        // Stop proxy
        if let Some(proxy) = self.proxy.take() {
            proxy.stop();
        }

        // Kill boringtun
        if let Some(mut proc) = self.boringtun_proc.take() {
            let _ = proc.kill();
        }

        // Tear down interface
        if let Some(iface) = self.iface.take() {
            teardown_interface(&iface);
        }

        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        self.boringtun_proc.is_some()
    }
}

// ── Platform-specific helpers ────────────────────────────────────────────────

fn boringtun_binary() -> Result<String, String> {
    // Check common locations
    let candidates = [
        // Bundled with the app (sidecar)
        "./boringtun-cli",
        "/usr/local/bin/boringtun-cli",
        "/opt/homebrew/bin/boringtun-cli",
        // Project root (dev)
        "../../boringtun/boringtun-cli",
    ];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    // Try PATH
    which_boringtun()
}

fn which_boringtun() -> Result<String, String> {
    let output = Command::new("which")
        .arg("boringtun-cli")
        .output()
        .map_err(|_| "boringtun-cli not found in PATH".to_string())?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err("boringtun-cli not found. Install it or place it in the app directory.".into())
    }
}

/// Find an available utun/wg interface name by trying candidates in order.
#[cfg(target_os = "macos")]
fn find_available_iface() -> Result<String, String> {
    // Check which utun interfaces already exist
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
    Err("No available utun interface found (tried utun9-utun30)".into())
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
    Err("No available wg interface found (tried wg0-wg20)".into())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn find_available_iface() -> Result<String, String> {
    Ok("wg0".to_string())
}

#[cfg(target_os = "macos")]
fn configure_interface(
    iface: &str,
    priv_key: &str,
    server_pub: &str,
    assigned_ip: &str,
    endpoint: &str,
) -> Result<(), String> {
    // Write private key to temp file (wg set reads from file)
    let key_file = format!("/tmp/vpntee_{iface}.key");
    std::fs::write(&key_file, priv_key)
        .map_err(|e| format!("Failed to write key file: {e}"))?;

    run_sudo(&["wg", "set", iface,
        "private-key", &key_file,
        "peer", server_pub,
        "allowed-ips", "0.0.0.0/0",
        "endpoint", endpoint,
        "persistent-keepalive", "25",
    ])?;

    // Strip CIDR suffix for ifconfig (e.g. "10.0.0.185/32" -> "10.0.0.185")
    let ip_bare = assigned_ip.split('/').next().unwrap_or(assigned_ip);

    // Set IP: on macOS utun uses point-to-point addressing
    run_sudo(&["ifconfig", iface, ip_bare, ip_bare, "up"])?;

    // Add route for VPN subnet
    run_sudo(&["route", "add", "-net", "10.0.0.0/24", "-interface", iface])?;

    let _ = std::fs::remove_file(&key_file);
    Ok(())
}

#[cfg(target_os = "linux")]
fn configure_interface(
    iface: &str,
    priv_key: &str,
    server_pub: &str,
    assigned_ip: &str,
    endpoint: &str,
) -> Result<(), String> {
    let key_file = format!("/tmp/vpntee_{iface}.key");
    std::fs::write(&key_file, priv_key)
        .map_err(|e| format!("Failed to write key file: {e}"))?;

    run_sudo(&["ip", "link", "add", iface, "type", "wireguard"])?;

    run_sudo(&["wg", "set", iface,
        "private-key", &key_file,
        "peer", server_pub,
        "allowed-ips", "0.0.0.0/0",
        "endpoint", endpoint,
        "persistent-keepalive", "25",
    ])?;

    // Use CIDR notation as-is (e.g. "10.0.0.185/32")
    let ip_cidr = if assigned_ip.contains('/') {
        assigned_ip.to_string()
    } else {
        format!("{assigned_ip}/32")
    };

    run_sudo(&["ip", "addr", "add", &ip_cidr, "dev", iface])?;
    run_sudo(&["ip", "link", "set", iface, "up"])?;
    run_sudo(&["ip", "route", "add", "10.0.0.0/24", "dev", iface])?;

    let _ = std::fs::remove_file(&key_file);
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn configure_interface(
    _iface: &str,
    _priv_key: &str,
    _server_pub: &str,
    _assigned_ip: &str,
    _endpoint: &str,
) -> Result<(), String> {
    Err("Unsupported platform".into())
}

#[cfg(target_os = "macos")]
fn teardown_interface(iface: &str) {
    let _ = run_sudo(&["route", "delete", "-net", "10.0.0.0/24", "-interface", iface]);
    let _ = run_sudo(&["ifconfig", iface, "down"]);
    let _ = run_sudo(&["ifconfig", iface, "destroy"]);
}

#[cfg(target_os = "linux")]
fn teardown_interface(iface: &str) {
    let _ = run_sudo(&["ip", "route", "delete", "10.0.0.0/24", "dev", iface]);
    let _ = run_sudo(&["ip", "link", "delete", iface]);
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn teardown_interface(_iface: &str) {}

/// Kill any leftover boringtun-cli processes from previous runs.
fn kill_stale_boringtun() {
    let _ = Command::new("sudo")
        .args(["pkill", "-f", "boringtun-cli"])
        .status();
    // Small delay for the OS to release the interface
    std::thread::sleep(std::time::Duration::from_millis(300));
}

fn run_sudo(args: &[&str]) -> Result<(), String> {
    let status = Command::new("sudo")
        .args(args)
        .status()
        .map_err(|e| format!("Failed to run sudo {:?}: {e}", args))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Command failed: sudo {:?}", args))
    }
}
