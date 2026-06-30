mod config;
mod printer;
mod relay;

use tauri::{
    tray::TrayIconBuilder,
    menu::{MenuBuilder, MenuItemBuilder},
    Listener,
};
use tauri_plugin_autostart::MacosLauncher;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, serde::Serialize)]
struct RelayStatus {
    connected: bool,
    venue_id: Option<String>,
    print_count: u64,
}

struct AppState {
    status: Mutex<RelayStatus>,
    config: Mutex<Option<config::RelayConfig>>,
}

#[tauri::command]
async fn get_status(state: tauri::State<'_, Arc<AppState>>) -> Result<RelayStatus, String> {
    Ok(state.status.lock().await.clone())
}

#[tauri::command]
async fn setup_from_token(token: String, state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    let cfg = config::fetch_config_from_token(&token).await.map_err(|e| e.to_string())?;
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    *state.config.lock().await = Some(cfg.clone());
    Ok(format!("Configured for venue {}", cfg.venue_id))
}

pub fn run() {
    let app_state = Arc::new(AppState {
        status: Mutex::new(RelayStatus {
            connected: false,
            venue_id: None,
            print_count: 0,
        }),
        config: Mutex::new(None),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().level(log::LevelFilter::Info).build())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .manage(app_state.clone())
        .invoke_handler(tauri::generate_handler![get_status, setup_from_token])
        .setup(move |app| {
            // Load saved config
            let state = app_state.clone();
            let _handle = app.handle().clone();

            // Build tray menu
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let status_item = MenuItemBuilder::with_id("status", "Status: Starting...").enabled(false).build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&status_item)
                .separator()
                .item(&quit)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .on_menu_event(move |app, event| {
                    if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .tooltip("Synalux Print Relay")
                .build(app)?;

            // Handle deep link setup
            let state_dl = state.clone();
            app.listen("deep-link://new-url", move |event| {
                let url = event.payload().to_string();
                if let Some(token) = url.strip_prefix("synalux-relay://setup?token=") {
                    let token = token.trim_matches('"').to_string();
                    let s = state_dl.clone();
                    tokio::spawn(async move {
                        match config::fetch_config_from_token(&token).await {
                            Ok(cfg) => {
                                let _ = config::save_config(&cfg);
                                *s.config.lock().await = Some(cfg);
                                log::info!("Setup complete via deep link");
                            }
                            Err(e) => log::error!("Deep link setup failed: {}", e),
                        }
                    });
                }
            });

            // Start relay loop
            let state_relay = state.clone();
            tauri::async_runtime::spawn(async move {
                // Try loading saved config
                if let Ok(cfg) = config::load_config() {
                    *state_relay.config.lock().await = Some(cfg);
                }

                loop {
                    let cfg = state_relay.config.lock().await.clone();
                    if let Some(cfg) = cfg {
                        log::info!("Connecting relay for venue {}", cfg.venue_id);
                        {
                            let mut s = state_relay.status.lock().await;
                            s.venue_id = Some(cfg.venue_id.clone());
                            s.connected = true;
                        }
                        relay::run_relay(&cfg, state_relay.clone()).await;
                        {
                            let mut s = state_relay.status.lock().await;
                            s.connected = false;
                        }
                        log::warn!("Relay disconnected, reconnecting in 5s...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    } else {
                        log::info!("No config found, waiting for setup...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
