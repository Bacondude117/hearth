mod commands;

use std::sync::Arc;

use hearth_core::events::{Event, EventSink};
use hearth_core::manager::ServerManager;
use tauri::{Emitter, Manager, WindowEvent};

/// Delivers core events to the web view on a single `hearth` channel.
struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: Event) {
        if let Err(e) = self.0.emit("hearth", &event) {
            tracing::warn!("failed to emit event: {e}");
        }
    }
}

pub struct AppState {
    pub manager: Arc<ServerManager>,
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,hearth_core=debug".into()),
        )
        .init();

    tauri::Builder::default()
        // A second Hearth would fight the first over the same servers; focus the existing one.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().app_data_dir()?;
            std::fs::create_dir_all(&root)?;
            tracing::info!("data directory: {}", root.display());
            let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app.handle().clone()));
            let manager = ServerManager::new(root, sink);
            // Bring shared servers back online: start the playit agent if an account is linked.
            if manager.playit.is_linked() {
                let playit = manager.playit.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = playit.start().await {
                        tracing::warn!("could not start playit agent: {e}");
                    }
                });
            }
            app.manage(AppState { manager });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle().clone();
                let running = app.state::<AppState>().manager.running_count();
                api.prevent_close();
                if running > 0 {
                    // Let the UI ask whether to stop the servers.
                    let _ = app.emit("close-requested", running);
                } else {
                    tauri::async_runtime::spawn(commands::shutdown_and_exit(app));
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::games,
            commands::versions,
            commands::suggest_port,
            commands::list_servers,
            commands::get_server,
            commands::create_server,
            commands::reinstall_server,
            commands::delete_server,
            commands::update_server,
            commands::start_server,
            commands::stop_server,
            commands::restart_server,
            commands::kill_server,
            commands::send_command,
            commands::console_lines,
            commands::get_properties,
            commands::set_properties,
            commands::pick_pack_file,
            commands::inspect_pack,
            commands::import_pack,
            commands::list_content,
            commands::add_content,
            commands::set_content_enabled,
            commands::delete_content,
            commands::list_players,
            commands::player_action,
            commands::open_server_folder,
            commands::open_link,
            commands::share_info,
            commands::share_server,
            commands::unshare_server,
            commands::playit_status,
            commands::playit_link,
            commands::playit_cancel_link,
            commands::playit_unlink,
            commands::playit_start,
            commands::playit_stop,
            commands::playit_console,
            commands::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Hearth");
}
