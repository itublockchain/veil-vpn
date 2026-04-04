mod vpn;

use tokio::sync::Mutex;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition,
};

use vpn::{ConnectedInfo, VpnManager, VpnStateEvent, VpnStatus, query_gateway_balance, get_wallet_address};

struct AppState {
    vpn: Mutex<VpnManager>,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
async fn connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
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

    let result = state.vpn.lock().await.connect(app.clone()).await;

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
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            vpn: Mutex::new(VpnManager::new()),
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
        .invoke_handler(tauri::generate_handler![connect, disconnect, get_status, refresh_balance, get_wallet_info])
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
