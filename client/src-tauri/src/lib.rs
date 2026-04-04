mod ens;
mod proxy;
mod vpn;

use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, PhysicalPosition,
};

use ens::Server;
use vpn::{ConnectedInfo, VpnManager};

struct AppState {
    vpn: Mutex<VpnManager>,
    servers: Mutex<Vec<Server>>,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
async fn fetch_servers(state: tauri::State<'_, AppState>) -> Result<Vec<Server>, String> {
    let servers = ens::fetch_servers_from_ens().await?;
    *state.servers.lock().await = servers.clone();
    Ok(servers)
}

#[tauri::command]
async fn connect_vpn(
    server_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<ConnectedInfo, String> {
    let server = {
        let servers = state.servers.lock().await;
        servers
            .iter()
            .find(|s| s.id == server_id)
            .cloned()
            .ok_or_else(|| format!("Server '{server_id}' not found"))?
    };

    let info = state
        .vpn
        .lock()
        .await
        .connect(
            server.name.clone(),
            server.api_url.clone(),
            server.ws_url.clone(),
        )
        .await?;

    Ok(info)
}

#[tauri::command]
async fn disconnect_vpn(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.vpn.lock().await.disconnect()
}

#[tauri::command]
async fn connect_no_tunnel(state: tauri::State<'_, AppState>) -> Result<ConnectedInfo, String> {
    state.vpn.lock().await.connect_no_tunnel().await
}

#[tauri::command]
async fn get_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.vpn.lock().await.is_connected())
}

// ── App setup ─────────────────────────────────────────────────────────────────

pub fn run() {
    // Shared state for double-click detection and debounce
    let last_click: Arc<StdMutex<Option<Instant>>> = Arc::new(StdMutex::new(None));
    let pending_handle: Arc<StdMutex<Option<tokio::task::JoinHandle<()>>>> =
        Arc::new(StdMutex::new(None));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            vpn: Mutex::new(VpnManager::new()),
            servers: Mutex::new(vec![]),
        })
        .setup(move |app| {
            let last_click = last_click.clone();
            let pending_handle = pending_handle.clone();

            let _tray = TrayIconBuilder::new()
                .tooltip("VPN TEE")
                .icon(app.default_window_icon().unwrap().clone())
                .icon_as_template(true)
                .on_tray_icon_event(move |tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        let now = Instant::now();
                        let is_double = {
                            let mut last = last_click.lock().unwrap();
                            let double = last
                                .map(|t| now.duration_since(t) < Duration::from_millis(400))
                                .unwrap_or(false);
                            *last = Some(now);
                            double
                        };

                        if is_double {
                            // Cancel any pending single-click toggle and quit
                            if let Some(handle) = pending_handle.lock().unwrap().take() {
                                handle.abort();
                            }
                            tray.app_handle().exit(0);
                        } else {
                            // Debounce: wait briefly to see if a second click arrives
                            let app = tray.app_handle().clone();
                            let click_pos = position;
                            let handle = tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(220)).await;
                                toggle_window(&app, click_pos);
                            });
                            let mut lock = pending_handle.lock().unwrap();
                            if let Some(old) = lock.take() {
                                old.abort();
                            }
                            *lock = Some(handle);
                        }
                    }
                })
                .build(app)?;

            // Hide from Dock on macOS — tray-only app
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            fetch_servers,
            connect_vpn,
            connect_no_tunnel,
            disconnect_vpn,
            get_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running vpntee");
}

fn toggle_window(app: &tauri::AppHandle, tray_pos: PhysicalPosition<f64>) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
            #[cfg(target_os = "macos")]
            let _ = app.hide();
        } else {
            position_below_tray(&win, tray_pos);
            // On macOS with Accessory policy, we must activate the app
            // before showing the window so it actually receives focus.
            #[cfg(target_os = "macos")]
            let _ = app.show();
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

/// Positions the popup directly below the tray icon that was clicked.
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

    // Center the window horizontally on the tray icon, place it just below
    let x = (tray_pos.x as i32) - (win_size.width as i32 / 2);
    let y = (tray_pos.y as i32) + margin;

    let _ = win.set_position(tauri::PhysicalPosition { x, y });
}
