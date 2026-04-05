mod vpn;

use std::sync::Arc;
use tokio::sync::Mutex;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition,
};

use vpn::{ConnectedInfo, VpnManager, VpnStateEvent, VpnStatus, query_gateway_balance, get_wallet_address};

struct AppState {
    vpn: Mutex<VpnManager>,
    world_id_session: Mutex<Option<Arc<idkit::BridgeConnection>>>,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
async fn connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    world_proof: Option<serde_json::Value>,
    server_ip: Option<String>,
) -> Result<ConnectedInfo, String> {
    // Emit connecting state
    let _ = app.emit(
        "vpn-state",
        &VpnStateEvent {
            status: VpnStatus::Connecting,
            assigned_ip: None,
            error: None,
        },
    );

    let result = state.vpn.lock().await.connect(app.clone(), world_proof, server_ip).await;

    match &result {
        Ok(info) => {
            let _ = app.emit(
                "vpn-state",
                &VpnStateEvent {
                    status: VpnStatus::Connected,
                    assigned_ip: Some(info.assigned_ip.clone()),
                    error: None,
                },
            );
        }
        Err(e) => {
            let _ = app.emit(
                "vpn-state",
                &VpnStateEvent {
                    status: VpnStatus::Error,
                    assigned_ip: None,
                    error: Some(e.clone()),
                },
            );
        }
    }

    result
}

#[tauri::command]
async fn disconnect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let _ = app.emit(
        "vpn-state",
        &VpnStateEvent {
            status: VpnStatus::Disconnecting,
            assigned_ip: None,
            error: None,
        },
    );

    let result = state.vpn.lock().await.disconnect();

    let _ = app.emit(
        "vpn-state",
        &VpnStateEvent {
            status: VpnStatus::Disconnected,
            assigned_ip: None,
            error: result.as_ref().err().cloned(),
        },
    );

    result
}

#[tauri::command]
async fn get_status(state: tauri::State<'_, AppState>) -> Result<VpnStatus, String> {
    Ok(state.vpn.lock().await.status())
}

#[tauri::command]
async fn refresh_balance(wallet_address: String) -> Result<String, String> {
    query_gateway_balance(&wallet_address).await
}

#[tauri::command]
fn get_pubkey() -> Result<String, String> {
    vpn::get_pubkey_b64()
}

#[tauri::command]
async fn start_world_id(
    state: tauri::State<'_, AppState>,
    server_ip: Option<String>,
) -> Result<String, String> {
    use idkit::bridge::{BridgeConnectionParams, RequestKind, Environment};
    use idkit::{AppId, RpContext, VerificationLevel, Preset};

    // 1. Fetch RP context from VPN server
    let api_base = match &server_ip {
        Some(ip) => format!("http://{}:8080", ip),
        None => vpn::api_base(),
    };
    let resp: serde_json::Value = reqwest::get(format!("{}/v1/rp-context", api_base))
        .await
        .map_err(|e| format!("Failed to reach server: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Parse error: {e}"))?;

    if let Some(err) = resp.get("error") {
        return Err(err.as_str().unwrap_or("unknown").to_string());
    }

    let rp_context = RpContext::new(
        resp["rp_id"].as_str().unwrap_or(""),
        resp["nonce"].as_str().unwrap_or(""),
        resp["created_at"].as_u64().unwrap_or(0),
        resp["expires_at"].as_u64().unwrap_or(0),
        resp["signature"].as_str().unwrap_or(""),
    ).map_err(|e| format!("Invalid RP context: {e}"))?;

    let action = resp["action"].as_str().unwrap_or("veil-vpn-connect").to_string();

    // 2. Get signal (WireGuard pubkey)
    let signal = vpn::get_pubkey_b64().unwrap_or_default();

    // 3. Build params using OrbLegacy preset
    let preset = Preset::OrbLegacy { signal: Some(signal) };
    let (constraints, verification_level, legacy_signal) = preset.to_bridge_params();

    let params = BridgeConnectionParams {
        app_id: AppId::new("app_59ecb9a350d70a30543cb847da635d31").map_err(|e| format!("{e}"))?,
        kind: RequestKind::Uniqueness { action },
        constraints: Some(constraints),
        rp_context,
        action_description: None,
        legacy_verification_level: verification_level,
        legacy_signal: legacy_signal.unwrap_or_default(),
        bridge_url: None,
        allow_legacy_proofs: true,
        override_connect_base_url: None,
        return_to: None,
        environment: Some(Environment::Production),
    };

    // 4. Create bridge session
    let session = idkit::BridgeConnection::create(params)
        .await
        .map_err(|e| format!("Failed to create World ID session: {e}"))?;

    let url = session.connect_url();

    // Store session for polling
    *state.world_id_session.lock().await = Some(Arc::new(session));

    Ok(url)
}

#[tauri::command]
async fn poll_world_id(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let session = state.world_id_session.lock().await;
    let session = session.as_ref().ok_or("No active World ID session")?;

    match session.poll_for_status().await.map_err(|e| format!("{e}"))? {
        idkit::Status::WaitingForConnection => {
            Ok(serde_json::json!({ "status": "waiting" }))
        }
        idkit::Status::AwaitingConfirmation => {
            Ok(serde_json::json!({ "status": "confirming" }))
        }
        idkit::Status::Confirmed(result) => {
            Ok(serde_json::json!({
                "status": "confirmed",
                "result": result,
            }))
        }
        idkit::Status::Failed(err) => {
            Err(format!("World ID verification failed: {err}"))
        }
    }
}

#[tauri::command]
async fn get_wallet_info() -> Result<WalletInfo, String> {
    let address = get_wallet_address()?;
    let balance = query_gateway_balance(&address).await.unwrap_or_else(|_| "0.000000".into());
    Ok(WalletInfo { address, balance })
}

#[derive(serde::Serialize)]
struct WalletInfo {
    address: String,
    balance: String,
}

// ── App setup ─────────────────────────────────────────────────────────────────

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_localhost::Builder::new(1421).build())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            vpn: Mutex::new(VpnManager::new()),
            world_id_session: Mutex::new(None),
        })
        .setup(move |app| {
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))
                .expect("failed to load tray icon");
            let _tray = TrayIconBuilder::new()
                .tooltip("Veil VPN")
                .icon(tray_icon)
                .icon_as_template(true)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle(), position);
                    }
                })
                .build(app)?;

            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == "main" {
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![connect, disconnect, get_status, refresh_balance, get_wallet_info, get_pubkey, start_world_id, poll_world_id])
        .run(tauri::generate_context!())
        .expect("error while running vpntee");
}

fn toggle_window(app: &tauri::AppHandle, tray_pos: PhysicalPosition<f64>) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            position_below_tray(&win, tray_pos);
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

fn position_below_tray(win: &tauri::WebviewWindow, tray_pos: PhysicalPosition<f64>) {
    let scale = win
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);

    let win_size = win.outer_size().unwrap_or(tauri::PhysicalSize {
        width: 300,
        height: 420,
    });

    let margin = (8.0 * scale) as i32;

    let x = (tray_pos.x as i32) - (win_size.width as i32 / 2);
    let y = (tray_pos.y as i32) + margin;

    let _ = win.set_position(tauri::PhysicalPosition { x, y });
}
