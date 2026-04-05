// Copyright (c) 2019 Cloudflare, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause

use boringtun::device::drop_privileges::drop_privileges;
use boringtun::device::{DeviceConfig, DeviceHandle};

/// Find the actual UAPI socket name for an interface.
/// On macOS, "utun" becomes "utun6" etc. Scan /var/run/wireguard/ for a matching socket.
#[cfg(feature = "payment")]
fn find_uapi_interface(hint: &str) -> Option<String> {
    let dir = std::fs::read_dir("/var/run/wireguard/").ok()?;
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(hint) && name.ends_with(".sock") {
            return Some(name.trim_end_matches(".sock").to_string());
        }
    }
    None
}
use clap::{Arg, Command};
use daemonize::Daemonize;
use std::fs::File;
use std::os::unix::net::UnixDatagram;
use std::process::exit;
use tracing::Level;

fn check_tun_name(_v: String) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    {
        if boringtun::device::tun::parse_utun_name(&_v).is_ok() {
            Ok(())
        } else {
            Err("Tunnel name must have the format 'utun[0-9]+', use 'utun' for automatic assignment".to_owned())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

fn main() {
    let matches = Command::new("boringtun")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Vlad Krasnov <vlad@cloudflare.com>")
        .args(&[
            Arg::new("INTERFACE_NAME")
                .required(true)
                .takes_value(true)
                .validator(|tunname| check_tun_name(tunname.to_string()))
                .help("The name of the created interface"),
            Arg::new("foreground")
                .long("foreground")
                .short('f')
                .help("Run and log in the foreground"),
            Arg::new("threads")
                .takes_value(true)
                .long("threads")
                .short('t')
                .env("WG_THREADS")
                .help("Number of OS threads to use")
                .default_value("4"),
            Arg::new("verbosity")
                .takes_value(true)
                .long("verbosity")
                .short('v')
                .env("WG_LOG_LEVEL")
                .possible_values(["error", "info", "debug", "trace"])
                .help("Log verbosity")
                .default_value("error"),
            Arg::new("uapi-fd")
                .long("uapi-fd")
                .env("WG_UAPI_FD")
                .help("File descriptor for the user API")
                .default_value("-1"),
            Arg::new("tun-fd")
                .long("tun-fd")
                .env("WG_TUN_FD")
                .help("File descriptor for an already-existing TUN device")
                .default_value("-1"),
            Arg::new("log")
                .takes_value(true)
                .long("log")
                .short('l')
                .env("WG_LOG_FILE")
                .help("Log file")
                .default_value("/tmp/boringtun.out"),
            Arg::new("disable-drop-privileges")
                .long("disable-drop-privileges")
                .env("WG_SUDO")
                .help("Do not drop sudo privileges"),
            Arg::new("disable-connected-udp")
                .long("disable-connected-udp")
                .help("Disable connected UDP sockets to each peer"),
            Arg::new("ws-bind")
                .long("ws-bind")
                .takes_value(true)
                .env("BT_WS_BIND")
                .help("WebSocket proxy bind address (e.g. 0.0.0.0:8443)")
                .default_value(""),
            Arg::new("ws-connect")
                .long("ws-connect")
                .takes_value(true)
                .env("BT_WS_CONNECT")
                .help("Client WS bridge: connect to server WS URL (e.g. ws://1.2.3.4:8443)")
                .default_value(""),
            Arg::new("ws-local-port")
                .long("ws-local-port")
                .takes_value(true)
                .env("BT_WS_LOCAL_PORT")
                .help("Client WS bridge: local UDP port for boringtun ↔ bridge")
                .default_value("51821"),
            #[cfg(target_os = "linux")]
            Arg::new("disable-multi-queue")
                .long("disable-multi-queue")
                .help("Disable using multiple queues for the tunnel interface"),
        ])
        .get_matches();

    let background = !matches.is_present("foreground");
    #[cfg(target_os = "linux")]
    let uapi_fd: i32 = matches.value_of_t("uapi-fd").unwrap_or_else(|e| e.exit());
    let tun_fd: isize = matches.value_of_t("tun-fd").unwrap_or_else(|e| e.exit());
    let mut tun_name = matches.value_of("INTERFACE_NAME").unwrap();
    if tun_fd >= 0 {
        tun_name = matches.value_of("tun-fd").unwrap();
    }
    let n_threads: usize = matches.value_of_t("threads").unwrap_or_else(|e| e.exit());
    let log_level: Level = matches.value_of_t("verbosity").unwrap_or_else(|e| e.exit());

    // Create a socketpair to communicate between forked processes
    let (sock1, sock2) = UnixDatagram::pair().unwrap();
    let _ = sock1.set_nonblocking(true);

    let _guard;

    if background {
        let log = matches.value_of("log").unwrap();

        let log_file =
            File::create(log).unwrap_or_else(|_| panic!("Could not create log file {}", log));

        let (non_blocking, guard) = tracing_appender::non_blocking(log_file);

        _guard = guard;

        tracing_subscriber::fmt()
            .with_max_level(log_level)
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();

        let daemonize = Daemonize::new()
            .working_directory("/tmp")
            .exit_action(move || {
                let mut b = [0u8; 1];
                if sock2.recv(&mut b).is_ok() && b[0] == 1 {
                    println!("BoringTun started successfully");
                } else {
                    eprintln!("BoringTun failed to start");
                    exit(1);
                };
            });

        match daemonize.start() {
            Ok(_) => tracing::info!("BoringTun started successfully"),
            Err(e) => {
                tracing::error!(error = ?e);
                exit(1);
            }
        }
    } else {
        tracing_subscriber::fmt()
            .pretty()
            .with_max_level(log_level)
            .init();
    }

    let config = DeviceConfig {
        n_threads,
        #[cfg(target_os = "linux")]
        uapi_fd,
        use_connected_socket: !matches.is_present("disable-connected-udp"),
        #[cfg(target_os = "linux")]
        use_multi_queue: !matches.is_present("disable-multi-queue"),
    };

    let mut device_handle: DeviceHandle = match DeviceHandle::new(tun_name, config) {
        Ok(d) => d,
        Err(e) => {
            // Notify parent that tunnel initialization failed
            tracing::error!(message = "Failed to initialize tunnel", error=?e);
            sock1.send(&[0]).unwrap();
            exit(1);
        }
    };

    if !matches.is_present("disable-drop-privileges") {
        if let Err(e) = drop_privileges() {
            tracing::error!(message = "Failed to drop privileges", error = ?e);
            sock1.send(&[0]).unwrap();
            exit(1);
        }
    }

    // Notify parent that tunnel initialization succeeded
    sock1.send(&[1]).unwrap();
    drop(sock1);

    tracing::info!("BoringTun started successfully");

    // Payment features: server API, WS proxy, client WS bridge
    #[cfg(feature = "payment")]
    {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let shutdown_flag = Arc::new(AtomicBool::new(false));

        let is_server = std::env::var("BT_PAYMENT_SERVER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if is_server {
            let bind_addr =
                std::env::var("BT_HTTP_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
            let public_ip =
                std::env::var("BT_PUBLIC_IP").unwrap_or_else(|_| "127.0.0.1".to_string());

            let payment_config = boringtun::payment::PaymentConfig::default();
            let snapshot = boringtun::device::http_api::PaymentConfigSnapshot {
                chain_id: payment_config.chain_id,
                gateway_wallet: payment_config.gateway_wallet,
                usdc_contract: payment_config.usdc_contract,
                amount_per_quota: payment_config.amount_per_quota,
                quota_bytes: payment_config.quota_bytes,
            };

            // On macOS, "utun" gets assigned as "utun6" etc. Find the actual socket.
            let actual_tun_name =
                find_uapi_interface(tun_name).unwrap_or_else(|| tun_name.to_string());
            tracing::info!("Registration API using interface: {}", actual_tun_name);

            let state = Arc::new(boringtun::device::http_api::RegistrationState::new(
                actual_tun_name,
                [10, 0, 0],
                snapshot,
                public_ip,
            ));

            // HTTP server thread
            let state_http = Arc::clone(&state);
            let shutdown_http = Arc::clone(&shutdown_flag);
            let bind = bind_addr.clone();
            std::thread::Builder::new()
                .name("http-api".into())
                .spawn(move || {
                    boringtun::device::http_api::run_http_server(state_http, &bind, shutdown_http);
                })
                .expect("Failed to spawn HTTP API thread");

            // Reaper thread
            let state_reaper = Arc::clone(&state);
            let shutdown_reaper = Arc::clone(&shutdown_flag);
            std::thread::Builder::new()
                .name("peer-reaper".into())
                .spawn(move || {
                    boringtun::device::http_api::run_reaper(state_reaper, shutdown_reaper);
                })
                .expect("Failed to spawn reaper thread");

            tracing::info!("Registration API on {}", bind_addr);

            // WebSocket proxy thread
            let ws_bind = std::env::var("BT_WS_BIND")
                .unwrap_or_else(|_| matches.value_of("ws-bind").unwrap_or("").to_string());
            if !ws_bind.is_empty() {
                let wg_port: u16 = std::env::var("BT_WG_PORT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(51820);
                let shutdown_ws = Arc::clone(&shutdown_flag);
                let ws_bind_log = ws_bind.clone();
                std::thread::Builder::new()
                    .name("ws-proxy".into())
                    .spawn(move || {
                        if let Err(e) = std::panic::catch_unwind(|| {
                            boringtun::device::ws_proxy::run_ws_proxy(
                                &ws_bind,
                                wg_port,
                                shutdown_ws,
                            );
                        }) {
                            tracing::error!("WS proxy thread panicked: {:?}", e);
                        }
                    })
                    .expect("Failed to spawn WS proxy thread");

                tracing::info!("WebSocket proxy on {}", ws_bind_log);
            }
        }

        // Client-side WS bridge (connects to remote server's WS proxy)
        let ws_connect = std::env::var("BT_WS_CONNECT")
            .unwrap_or_else(|_| matches.value_of("ws-connect").unwrap_or("").to_string());
        if !ws_connect.is_empty() {
            let ws_local_port: u16 = std::env::var("BT_WS_LOCAL_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| matches.value_of_t("ws-local-port").unwrap_or(51821));

            let shutdown_bridge = Arc::clone(&shutdown_flag);
            let ws_url = ws_connect.clone();
            std::thread::Builder::new()
                .name("ws-bridge".into())
                .spawn(move || {
                    boringtun::device::ws_bridge::run_ws_bridge(
                        ws_local_port,
                        &ws_url,
                        shutdown_bridge,
                    );
                })
                .expect("Failed to spawn WS bridge thread");

            tracing::info!("WS bridge: UDP :{} ↔ {}", ws_local_port, ws_connect);
        }
    }

    device_handle.wait();
}
