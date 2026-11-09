//! Thin wrappers from UI calls to `ServerManager`. Errors become user-readable strings.

use std::collections::HashMap;

use hearth_core::events::LogLine;
use hearth_core::games::{GameInfo, GameVersion};
use hearth_core::instance::Instance;
use hearth_core::content::ContentEntry;
use hearth_core::manager::{CreateServer, ImportPack, ServerSummary};
use hearth_core::packs::PackInfo;
use tauri_plugin_dialog::DialogExt;
use hearth_core::playit::{PlayitStatus, TunnelInfo};
use hearth_core::players::{PlayerAction, PlayersView};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::AppState;

type R<T> = Result<T, String>;

fn e(err: impl std::fmt::Display) -> String {
    err.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    data_dir: String,
    total_memory_mb: Option<u64>,
    playit_supported: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.manager.paths().root().display().to_string(),
        total_memory_mb: hearth_core::system::total_memory_mb(),
        playit_supported: hearth_core::playit::Playit::supported(),
    }
}

#[tauri::command]
pub fn games(state: State<'_, AppState>) -> Vec<GameInfo> {
    state.manager.games()
}

#[tauri::command]
pub async fn versions(state: State<'_, AppState>, game: String, flavor: String) -> R<Vec<GameVersion>> {
    state.manager.versions(&game, &flavor).await.map_err(e)
}

#[tauri::command]
pub fn suggest_port(state: State<'_, AppState>, game: String) -> R<u16> {
    state.manager.suggest_port(&game).map_err(e)
}

#[tauri::command]
pub fn list_servers(state: State<'_, AppState>) -> R<Vec<ServerSummary>> {
    state.manager.list().map_err(e)
}

#[tauri::command]
pub fn get_server(state: State<'_, AppState>, id: String) -> R<ServerSummary> {
    state.manager.get(&id).map_err(e)
}

#[tauri::command]
pub async fn create_server(state: State<'_, AppState>, req: CreateServer) -> R<ServerSummary> {
    let inst = state.manager.create(req).map_err(e)?;
    state.manager.get(&inst.id).map_err(e)
}

#[tauri::command]
pub async fn reinstall_server(state: State<'_, AppState>, id: String, version: Option<String>) -> R<()> {
    state.manager.reinstall(&id, version).map_err(e)
}

#[tauri::command]
pub async fn delete_server(state: State<'_, AppState>, id: String, remove_tunnel: bool) -> R<()> {
    state.manager.delete(&id, remove_tunnel).await.map_err(e)
}

#[tauri::command]
pub fn update_server(state: State<'_, AppState>, id: String, name: Option<String>, memory_mb: Option<u32>) -> R<Instance> {
    state.manager.update_instance(&id, name, memory_mb).map_err(e)
}

#[tauri::command]
pub async fn start_server(state: State<'_, AppState>, id: String) -> R<()> {
    state.manager.start(&id).await.map_err(e)
}

#[tauri::command]
pub async fn stop_server(state: State<'_, AppState>, id: String) -> R<()> {
    state.manager.stop(&id).await.map_err(e)
}

#[tauri::command]
pub async fn restart_server(state: State<'_, AppState>, id: String) -> R<()> {
    state.manager.restart(&id).await.map_err(e)
}

#[tauri::command]
pub fn kill_server(state: State<'_, AppState>, id: String) -> R<()> {
    state.manager.kill(&id).map_err(e)
}

#[tauri::command]
pub async fn send_command(state: State<'_, AppState>, id: String, line: String) -> R<()> {
    state.manager.command(&id, &line).await.map_err(e)
}

#[tauri::command]
pub fn console_lines(state: State<'_, AppState>, id: String) -> Vec<LogLine> {
    state.manager.console_lines(&id)
}

#[tauri::command]
pub fn get_properties(state: State<'_, AppState>, id: String) -> R<Vec<(String, String)>> {
    state.manager.properties(&id).map_err(e)
}

#[tauri::command]
pub fn set_properties(state: State<'_, AppState>, id: String, changes: HashMap<String, String>) -> R<()> {
    state.manager.set_properties(&id, changes).map_err(e)
}

/// Native "open file" dialog for a modpack. `None` if the user cancelled.
#[tauri::command]
pub async fn pick_pack_file(app: AppHandle) -> R<Option<String>> {
    let picked = app
        .dialog()
        .file()
        .set_title("Choose a modpack")
        .add_filter("Modpacks (.mrpack, server pack .zip)", &["mrpack", "zip"])
        .blocking_pick_file();
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()))
}

#[tauri::command]
pub async fn inspect_pack(state: State<'_, AppState>, path: String) -> R<PackInfo> {
    let manager = state.manager.clone();
    // Opening big zips is blocking work.
    tauri::async_runtime::spawn_blocking(move || manager.inspect_pack(std::path::Path::new(&path)))
        .await
        .map_err(e)?
        .map_err(e)
}

#[tauri::command]
pub async fn import_pack(state: State<'_, AppState>, req: ImportPack) -> R<ServerSummary> {
    let inst = state.manager.import_pack(req).map_err(e)?;
    state.manager.get(&inst.id).map_err(e)
}

#[tauri::command]
pub fn list_content(state: State<'_, AppState>, id: String) -> R<Vec<ContentEntry>> {
    state.manager.list_content(&id).map_err(e)
}

/// Opens a file picker for mod/plugin jars and copies the chosen files in.
#[tauri::command]
pub async fn add_content(app: AppHandle, state: State<'_, AppState>, id: String) -> R<Vec<String>> {
    let picked = app
        .dialog()
        .file()
        .set_title("Add mods or plugins")
        .add_filter("Java archives (.jar)", &["jar"])
        .blocking_pick_files();
    let Some(files) = picked else { return Ok(vec![]) };
    let paths: Vec<std::path::PathBuf> = files.into_iter().filter_map(|f| f.into_path().ok()).collect();
    state.manager.add_content(&id, &paths).map_err(e)
}

#[tauri::command]
pub fn set_content_enabled(state: State<'_, AppState>, id: String, file: String, enabled: bool) -> R<()> {
    state.manager.set_content_enabled(&id, &file, enabled).map_err(e)
}

#[tauri::command]
pub fn delete_content(state: State<'_, AppState>, id: String, file: String) -> R<()> {
    state.manager.delete_content(&id, &file).map_err(e)
}

#[tauri::command]
pub fn list_players(state: State<'_, AppState>, id: String) -> R<PlayersView> {
    state.manager.players(&id).map_err(e)
}

#[tauri::command]
pub async fn player_action(state: State<'_, AppState>, id: String, name: String, action: PlayerAction) -> R<()> {
    state.manager.player_action(&id, &name, action).await.map_err(e)
}

#[tauri::command]
pub fn open_server_folder(app: AppHandle, state: State<'_, AppState>, id: String) -> R<()> {
    let dir = state.manager.server_dir(&id).map_err(e)?;
    std::fs::create_dir_all(&dir).map_err(e)?;
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(e)
}

/// Hosts the UI may send the user to. Everything else is refused.
const LINK_HOSTS: &[&str] = &[
    "playit.gg",
    "aka.ms",
    "minecraft.net",
    "www.minecraft.net",
    "go.microsoft.com",
    "papermc.io",
    "hangar.papermc.io",
    "github.com",
];

#[tauri::command]
pub fn open_link(app: AppHandle, url: String) -> R<()> {
    let ok = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_some_and(|host| LINK_HOSTS.contains(&host) || host.ends_with(".playit.gg"));
    if !ok {
        return Err(format!("Refusing to open {url}"));
    }
    app.opener().open_url(url, None::<&str>).map_err(e)
}

#[tauri::command]
pub async fn share_info(state: State<'_, AppState>, id: String) -> R<Option<TunnelInfo>> {
    state.manager.share_info(&id).await.map_err(e)
}

#[tauri::command]
pub async fn share_server(state: State<'_, AppState>, id: String) -> R<()> {
    state.manager.share(&id).await.map_err(e)
}

#[tauri::command]
pub async fn unshare_server(state: State<'_, AppState>, id: String) -> R<()> {
    state.manager.unshare(&id).await.map_err(e)
}

#[tauri::command]
pub async fn playit_status(state: State<'_, AppState>) -> R<PlayitStatus> {
    Ok(state.manager.playit.status().await)
}

/// Starts linking and opens the playit.gg claim page in the browser.
#[tauri::command]
pub async fn playit_link(app: AppHandle, state: State<'_, AppState>) -> R<String> {
    let url = state.manager.playit.start_claim().map_err(e)?;
    let _ = app.opener().open_url(url.clone(), None::<&str>);
    Ok(url)
}

#[tauri::command]
pub fn playit_cancel_link(state: State<'_, AppState>) {
    state.manager.playit.cancel_claim();
}

#[tauri::command]
pub async fn playit_unlink(state: State<'_, AppState>) -> R<()> {
    state.manager.playit.unlink().await.map_err(e)
}

#[tauri::command]
pub async fn playit_start(state: State<'_, AppState>) -> R<()> {
    state.manager.playit.start().await.map_err(e)
}

#[tauri::command]
pub async fn playit_stop(state: State<'_, AppState>) -> R<()> {
    state.manager.playit.stop().await;
    Ok(())
}

#[tauri::command]
pub fn playit_console(state: State<'_, AppState>) -> Vec<LogLine> {
    state.manager.playit.console().snapshot()
}

/// Stops every server and the playit agent, then exits.
#[tauri::command]
pub async fn quit_app(app: AppHandle) -> R<()> {
    shutdown_and_exit(app).await;
    Ok(())
}

pub async fn shutdown_and_exit(app: AppHandle) {
    let manager = app.state::<AppState>().manager.clone();
    manager.shutdown().await;
    app.exit(0);
}
