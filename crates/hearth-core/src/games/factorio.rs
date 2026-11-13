//! Factorio. On Windows the dedicated server is the game itself (`factorio.exe --start-server`;
//! Wube only ships a separate headless build for Linux), so Hearth runs the player's installed
//! copy (Steam or factorio.com) with its own `config.ini`: saves, mods and settings live in the
//! server folder and never touch the player's game.
//!
//! The map is created at install with `--create`. The server reads console commands and chat
//! from stdin (`/promote`, `/ban name reason`, `/quit`, ...) and prints `[JOIN]`/`[LEAVE]` lines.

use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::{
    ClientImport, FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome, LaunchSpec, LineEvent,
    Progress,
};
use crate::content::{ContentFolder, DISABLED_SUFFIX};
use crate::error::{Error, IoContext, Result};
use crate::instance::Instance;
use crate::net::Protocol;
use crate::players::{PlayerAction, PlayerAdmin, PlayerCaps, PlayerEntry, Role, SeenPlayer};

pub const SETTINGS: &str = "server-settings.json";
const ADMINS: &str = "server-adminlist.json";
const BANS: &str = "server-banlist.json";
const WHITELIST: &str = "server-whitelist.json";
/// Hearth's own options (the whitelist switch is a launch flag, not a server setting).
const HEARTH_FILE: &str = "hearth-factorio.json";
const EXE_FILE: &str = ".hearth-factorio-exe";
const MODS: &str = "mods";
const SAVE: &str = "world.zip";
/// Settings key for the whitelist switch (stored in `HEARTH_FILE`).
pub const WHITELIST_KEY: &str = "hearth_whitelist";
/// Mods that ship inside the game's `data` folder (base game and expansions).
const BUILTIN: &[&str] = &["base", "core", "space-age", "quality", "elevated-rails"];

pub struct Factorio;

// ---- finding the game ----------------------------------------------------------------------------

fn exe_in(root: &Path) -> PathBuf {
    let bin = root.join("bin").join("x64");
    bin.join(if cfg!(windows) { "factorio.exe" } else { "factorio" })
}

/// The player's Factorio: Steam first, then the factorio.com installer's default folder.
pub fn find_install() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = crate::steamlib::app_dir("Factorio").into_iter().collect();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = std::env::var_os(var) {
                roots.push(PathBuf::from(p).join("Factorio"));
            }
        }
    }
    roots.into_iter().map(|r| exe_in(&r)).find(|e| e.is_file())
}

/// `<root>/bin/x64/factorio.exe` -> `<root>/data`
fn data_dir(exe: &Path) -> Option<PathBuf> {
    Some(exe.parent()?.parent()?.parent()?.join("data"))
}

/// The game version, from `data/base/info.json`.
pub fn installed_version(exe: &Path) -> Option<String> {
    let info = data_dir(exe)?.join("base").join("info.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(info).ok()?).ok()?;
    v.get("version")?.as_str().map(str::to_owned)
}

fn not_found() -> Error {
    Error::InvalidInput(
        "Hearth couldn't find Factorio on this PC. Factorio's server is part of the game on Windows, so install it (Steam or factorio.com) and try again.".into(),
    )
}

/// The game this server runs: the one recorded at install, or wherever Factorio is now.
fn server_exe(dir: &Path) -> Result<PathBuf> {
    if let Ok(p) = std::fs::read_to_string(dir.join(EXE_FILE)) {
        let p = PathBuf::from(p.trim());
        if p.is_file() {
            return Ok(p);
        }
    }
    find_install().ok_or_else(not_found)
}

/// `config/config.ini`: game data from the install, everything written into the server folder.
fn write_config(dir: &Path, exe: &Path) -> Result<PathBuf> {
    let data = data_dir(exe).ok_or_else(not_found)?;
    let cfg_dir = dir.join("config");
    std::fs::create_dir_all(&cfg_dir).at(&cfg_dir)?;
    let path = cfg_dir.join("config.ini");
    let text = format!(
        "; Written by Hearth\n[path]\nread-data={}\nwrite-data={}\n",
        data.display(),
        dir.display()
    );
    std::fs::write(&path, text).at(&path)?;
    Ok(path)
}

// ---- server-settings.json ------------------------------------------------------------------------

fn load_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text = std::fs::read_to_string(path).at(path)?;
    serde_json::from_str(&text).map_err(|e| Error::InvalidInput(format!("{} isn't valid JSON: {e}", path.display())))
}

fn save_json(path: &Path, v: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(v).expect("json serializes")).at(path)
}

/// Settings for a new server: Factorio's example file (if the install has it) with Hearth's
/// defaults. Never public: listing on the official server browser needs a factorio.com login.
pub(crate) fn initial_settings(example: Option<&str>, name: &str) -> Value {
    let mut v: Value = example
        .and_then(|t| serde_json::from_str(t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let o = v.as_object_mut().expect("object");
    o.insert("name".into(), name.into());
    o.insert("description".into(), "Hosted with Hearth".into());
    o.insert("tags".into(), Value::Array(vec![]));
    o.insert("visibility".into(), serde_json::json!({ "public": false, "lan": true }));
    for k in ["username", "password", "token", "game_password"] {
        o.insert(k.into(), "".into());
    }
    o.entry("max_players").or_insert(0.into());
    o.entry("require_user_verification").or_insert(true.into());
    o.entry("allow_commands").or_insert("admins-only".into());
    o.entry("autosave_interval").or_insert(10.into());
    o.entry("autosave_slots").or_insert(5.into());
    o.entry("afk_autokick_interval").or_insert(0.into());
    o.entry("auto_pause").or_insert(true.into());
    o.entry("only_admins_can_pause_the_game").or_insert(true.into());
    o.entry("autosave_only_on_server").or_insert(true.into());
    v
}

/// Top-level settings as strings, minus the factorio.com login fields.
pub(crate) fn settings_entries(v: &Value) -> Vec<(String, String)> {
    let Some(o) = v.as_object() else { return vec![] };
    o.iter()
        .filter(|(k, _)| !matches!(k.as_str(), "username" | "password" | "token"))
        .filter_map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some((k.clone(), s))
        })
        .collect()
}

/// Applies a change, keeping the setting's JSON type (number, true/false or text).
pub(crate) fn set_setting(v: &mut Value, key: &str, value: &str) -> Result<()> {
    if matches!(key, "username" | "password" | "token") {
        return Err(Error::InvalidInput("Hearth doesn't store factorio.com logins.".into()));
    }
    let o = v
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("server-settings.json isn't a JSON object.".into()))?;
    let bad = || Error::InvalidInput(format!("\"{value}\" isn't a valid value for {key}."));
    let new = match o.get(key) {
        Some(Value::Bool(_)) => Value::Bool(match value {
            "true" => true,
            "false" => false,
            _ => return Err(bad()),
        }),
        Some(Value::Number(_)) => {
            let n: i64 = value.trim().parse().map_err(|_| bad())?;
            if n < 0 {
                return Err(bad());
            }
            n.into()
        }
        _ => Value::String(value.to_owned()),
    };
    o.insert(key.to_owned(), new);
    Ok(())
}

fn whitelist_on(dir: &Path) -> bool {
    load_json(&dir.join(HEARTH_FILE))
        .ok()
        .and_then(|v| v.get("use_whitelist")?.as_bool())
        .unwrap_or(false)
}

// ---- mods ----------------------------------------------------------------------------------------

/// `name`, `title` and `version` from a mod zip's `info.json` (`<name>_<version>/info.json`).
pub fn read_mod_info(zip_path: &Path) -> Option<(String, String, String)> {
    let f = std::fs::File::open(zip_path).ok()?;
    let mut zip = zip::ZipArchive::new(f).ok()?;
    let idx = (0..zip.len()).find(|&i| {
        zip.by_index(i)
            .map(|e| {
                let n = e.name().trim_start_matches('/');
                n.ends_with("info.json") && n.matches('/').count() <= 1
            })
            .unwrap_or(false)
    })?;
    let mut text = String::new();
    zip.by_index(idx).ok()?.take(1 << 20).read_to_string(&mut text).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let name = v.get("name")?.as_str()?.to_owned();
    let title = v.get("title").and_then(Value::as_str).unwrap_or(&name).to_owned();
    let version = v.get("version")?.as_str()?.to_owned();
    Some((name, title, version))
}

/// `mymod_1.2.3.zip` -> `mymod` (when the zip can't be read).
fn name_from_file(file: &str) -> String {
    let base = file.strip_suffix(DISABLED_SUFFIX).unwrap_or(file);
    let base = base.strip_suffix(".zip").unwrap_or(base);
    base.rsplit_once('_').map(|(n, _)| n).unwrap_or(base).to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModListEntry {
    name: String,
    enabled: bool,
}

fn read_mod_list(path: &Path) -> Vec<ModListEntry> {
    load_json(path)
        .ok()
        .and_then(|v| v.get("mods").cloned())
        .and_then(|m| m.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            Some(ModListEntry {
                name: e.get("name")?.as_str()?.to_owned(),
                enabled: e.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            })
        })
        .collect()
}

fn write_mod_list_file(path: &Path, entries: &[ModListEntry]) -> Result<()> {
    let mods: Vec<Value> = entries
        .iter()
        .map(|e| serde_json::json!({ "name": e.name, "enabled": e.enabled }))
        .collect();
    save_json(path, &serde_json::json!({ "mods": mods }))
}

/// Rewrites `mods/mod-list.json` from the server's mods folder: zips are enabled, `.disabled`
/// ones off. Built-in entries (base game, expansions) keep their state.
pub fn write_mod_list(server_dir: &Path) -> Result<()> {
    let mods = server_dir.join(MODS);
    std::fs::create_dir_all(&mods).at(&mods)?;
    let list_path = mods.join("mod-list.json");
    let old = read_mod_list(&list_path);
    let mut out: Vec<ModListEntry> = old
        .iter()
        .filter(|e| BUILTIN.contains(&e.name.as_str()))
        .cloned()
        .collect();
    if !out.iter().any(|e| e.name == "base") {
        out.insert(
            0,
            ModListEntry {
                name: "base".into(),
                enabled: true,
            },
        );
    }
    let mut files: Vec<(String, bool)> = vec![];
    for e in std::fs::read_dir(&mods).at(&mods)?.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        let enabled = file.ends_with(".zip");
        if !enabled && !file.ends_with(&format!(".zip{DISABLED_SUFFIX}")) {
            continue;
        }
        let name = read_mod_info(&e.path())
            .map(|(n, _, _)| n)
            .unwrap_or_else(|| name_from_file(&file));
        files.push((name, enabled));
    }
    files.sort_by_key(|(n, _)| n.to_lowercase());
    for (name, enabled) in files {
        match out.iter_mut().find(|e| e.name == name) {
            Some(e) => e.enabled |= enabled,
            None => out.push(ModListEntry { name, enabled }),
        }
    }
    write_mod_list_file(&list_path, &out)
}

fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// The player's own Factorio data folder (`%APPDATA%\Factorio` / `~/.factorio`).
pub fn client_data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Factorio"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".factorio"))
    }
}

/// Copies the mods enabled in the player's game into the server, and matches its expansion
/// settings (Space Age, Quality, ...) so the player can join.
pub fn copy_client_mods(client_mods: Option<&Path>, server_dir: &Path) -> Result<ClientImport> {
    let client = client_mods
        .filter(|d| d.join("mod-list.json").exists())
        .ok_or_else(|| {
            Error::InvalidInput("Couldn't find your Factorio mods. Start Factorio once, then try again.".into())
        })?;
    let wanted = read_mod_list(&client.join("mod-list.json"));

    // Newest zip per mod name in the player's mods folder.
    let mut zips: Vec<(String, String, PathBuf)> = vec![];
    for e in std::fs::read_dir(client).at(client)?.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            if let Some((name, _, version)) = read_mod_info(&p) {
                match zips.iter_mut().find(|(n, _, _)| *n == name) {
                    Some(z) if version_key(&version) > version_key(&z.1) => *z = (name, version, p),
                    Some(_) => {}
                    None => zips.push((name, version, p)),
                }
            }
        }
    }

    let dest = server_dir.join(MODS);
    std::fs::create_dir_all(&dest).at(&dest)?;
    let mut out = ClientImport::default();
    let mut builtins: Vec<ModListEntry> = vec![];
    for m in &wanted {
        if BUILTIN.contains(&m.name.as_str()) {
            builtins.push(m.clone());
            continue;
        }
        if !m.enabled {
            continue;
        }
        let Some((name, version, src)) = zips.iter().find(|(n, _, _)| *n == m.name) else {
            out.missing.push(m.name.clone());
            continue;
        };
        // Replace any other version of this mod on the server.
        for e in std::fs::read_dir(&dest).at(&dest)?.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if f.ends_with(".zip") || f.ends_with(&format!(".zip{DISABLED_SUFFIX}")) {
                let n = read_mod_info(&e.path())
                    .map(|(n, _, _)| n)
                    .unwrap_or_else(|| name_from_file(&f));
                if n == *name {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let file = src.file_name().expect("file").to_owned();
        std::fs::copy(src, dest.join(&file)).at(src)?;
        out.copied.push(format!("{name} v{version}"));
    }
    // Expansions on/off exactly as in the player's game.
    let list_path = dest.join("mod-list.json");
    let mut list = read_mod_list(&list_path);
    list.retain(|e| !BUILTIN.contains(&e.name.as_str()));
    builtins.extend(list);
    write_mod_list_file(&list_path, &builtins)?;
    write_mod_list(server_dir)?;
    Ok(out)
}

// ---- console -------------------------------------------------------------------------------------

/// `2026-11-12 13:00:00 [JOIN] Bacon joined the game`, `[LEAVE] Bacon left the game`, and the
/// state change to `InGame` once the map is running.
pub(crate) fn parse_factorio_line(line: &str) -> Option<LineEvent> {
    if line.contains("changing state from(CreatingGame) to(InGame)") {
        return Some(LineEvent::Ready);
    }
    if let Some((_, rest)) = line.split_once("[JOIN] ") {
        return rest
            .strip_suffix(" joined the game")
            .map(|n| LineEvent::PlayerJoined(n.trim().to_owned()));
    }
    if let Some((_, rest)) = line.split_once("[LEAVE] ") {
        let name = rest.split(" left the game").next()?.trim();
        return (!name.is_empty() && rest.contains(" left the game")).then(|| LineEvent::PlayerLeft(name.to_owned()));
    }
    None
}

// ---- provider ------------------------------------------------------------------------------------

#[async_trait]
impl GameProvider for Factorio {
    fn info(&self) -> GameInfo {
        GameInfo {
            id: "factorio",
            name: "Factorio",
            description: "Build and automate factories together. Friends join from Multiplayer → Connect to address.",
            flavors: vec![FlavorInfo {
                id: "installed",
                name: "Your Factorio",
                description: "Runs the server with the Factorio installed on this PC (Steam or factorio.com).",
            }],
            default_port: 34197,
            protocol: Protocol::Udp,
            configurable_memory: false,
            memory_limit: true,
            eula: None,
            playit_tunnel_type: None,
            command_hint: "Type a command or chat, e.g. /players or hello",
            setup_note: Some(
                "Factorio's server is part of the game on Windows, so Hearth uses the copy installed on this PC, with its own map, saves and mods. Players need the same Factorio version (and expansion, if it's turned on).",
            ),
        }
    }

    async fn versions(&self, _ctx: &GameContext, _flavor: &str) -> Result<Vec<GameVersion>> {
        let exe = find_install().ok_or_else(not_found)?;
        let label = match installed_version(&exe) {
            Some(v) => format!("Installed ({v})"),
            None => "Installed".into(),
        };
        Ok(vec![GameVersion {
            id: "installed".into(),
            label,
            stable: true,
        }])
    }

    async fn install(
        &self,
        _ctx: &GameContext,
        inst: &Instance,
        dir: &Path,
        fresh: bool,
        progress: Progress<'_>,
    ) -> Result<InstallOutcome> {
        progress("Finding Factorio", None);
        let exe = find_install().ok_or_else(not_found)?;
        std::fs::create_dir_all(dir).at(dir)?;
        std::fs::write(dir.join(EXE_FILE), exe.display().to_string()).at(dir)?;
        let config = write_config(dir, &exe)?;

        let settings = dir.join(SETTINGS);
        if fresh || !settings.exists() {
            let example =
                data_dir(&exe).and_then(|d| std::fs::read_to_string(d.join("server-settings.example.json")).ok());
            save_json(&settings, &initial_settings(example.as_deref(), &inst.name))?;
        }
        for list in [ADMINS, BANS, WHITELIST] {
            if !dir.join(list).exists() {
                std::fs::write(dir.join(list), "[]").at(dir)?;
            }
        }
        write_mod_list(dir)?;

        let saves = dir.join("saves");
        let has_save = std::fs::read_dir(&saves)
            .map(|d| d.flatten().any(|e| e.path().extension().is_some_and(|x| x == "zip")))
            .unwrap_or(false);
        if !has_save {
            progress("Creating the map", None);
            std::fs::create_dir_all(&saves).at(&saves)?;
            create_map(&exe, &config, &saves.join(SAVE)).await?;
        }
        Ok(InstallOutcome {
            installed_build: installed_version(&exe),
            java_major: None,
            jvm_args: None,
            launch: None,
        })
    }

    fn launch(&self, _ctx: &GameContext, inst: &Instance, dir: &Path) -> Result<LaunchSpec> {
        let exe = server_exe(dir)?;
        let config = write_config(dir, &exe)?;
        write_mod_list(dir)?;
        let p = |name: &str| dir.join(name).display().to_string();
        Ok(LaunchSpec {
            program: exe,
            args: vec![
                "--config".into(),
                config.display().to_string(),
                "--start-server-load-latest".into(),
                "--server-settings".into(),
                p(SETTINGS),
                "--port".into(),
                inst.port.to_string(),
                "--mod-directory".into(),
                p(MODS),
                "--server-adminlist".into(),
                p(ADMINS),
                "--server-banlist".into(),
                p(BANS),
                "--server-whitelist".into(),
                p(WHITELIST),
                "--use-server-whitelist".into(),
                whitelist_on(dir).to_string(),
            ],
            cwd: dir.to_path_buf(),
            env: vec![],
            console_encoding: Default::default(),
            memory_limit_mb: None,
            log_file: None,
            own_console: false,
        })
    }

    fn parse_line(&self, line: &str) -> Option<LineEvent> {
        parse_factorio_line(line)
    }

    fn stop_command(&self) -> &'static str {
        // Save, then shut down.
        "/server-save\n/quit"
    }

    fn properties_file(&self) -> Option<&'static str> {
        Some(SETTINGS)
    }

    fn port_setting_key(&self) -> &'static str {
        "port"
    }

    fn apply_port(&self, _dir: &Path, _port: u16) -> Result<()> {
        Ok(()) // passed with --port at launch
    }

    fn read_settings(&self, dir: &Path) -> Result<Vec<(String, String)>> {
        let mut out = settings_entries(&load_json(&dir.join(SETTINGS))?);
        out.push((WHITELIST_KEY.into(), whitelist_on(dir).to_string()));
        Ok(out)
    }

    fn write_settings(&self, dir: &Path, changes: &[(String, String)]) -> Result<()> {
        let path = dir.join(SETTINGS);
        let mut v = load_json(&path)?;
        for (k, val) in changes {
            if k == WHITELIST_KEY {
                save_json(
                    &dir.join(HEARTH_FILE),
                    &serde_json::json!({ "use_whitelist": val == "true" }),
                )?;
            } else {
                set_setting(&mut v, k, val)?;
            }
        }
        save_json(&path, &v)
    }

    fn players(&self) -> Option<&dyn PlayerAdmin> {
        Some(&FactorioPlayers)
    }

    fn content_folder(&self, _inst: &Instance) -> Option<ContentFolder> {
        Some(ContentFolder {
            label: "Mods",
            folder: MODS,
            extension: "zip",
            import_label: Some("Copy my enabled mods from Factorio"),
        })
    }

    fn describe_content(&self, file: &Path) -> Option<(String, Option<String>)> {
        let (_, title, version) = read_mod_info(file)?;
        Some((title, Some(version)))
    }

    fn import_client_content(&self, _inst: &Instance, dir: &Path) -> Option<Result<ClientImport>> {
        let client = client_data_dir().map(|d| d.join(MODS));
        Some(copy_client_mods(client.as_deref(), dir))
    }
}

/// Runs `factorio --create` to make the first map (default map settings).
async fn create_map(exe: &Path, config: &Path, save: &Path) -> Result<()> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--config")
        .arg(config)
        .arg("--create")
        .arg(save)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(crate::system::CHILD_PROCESS_FLAGS);
    let child = cmd
        .spawn()
        .map_err(|e| Error::InvalidInput(format!("Couldn't start Factorio to create the map: {e}")))?;
    let out = tokio::time::timeout(std::time::Duration::from_secs(600), child.wait_with_output())
        .await
        .map_err(|_| Error::InvalidInput("Creating the Factorio map took too long.".into()))?
        .map_err(|e| Error::InvalidInput(format!("Factorio stopped while creating the map: {e}")))?;
    if out.status.success() && save.exists() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let detail = text
        .lines()
        .rev()
        .find(|l| l.contains("Error") || l.contains("error"))
        .unwrap_or("no details")
        .trim()
        .to_owned();
    Err(Error::InvalidInput(format!(
        "Factorio couldn't create the map: {detail}"
    )))
}

// ---- players -------------------------------------------------------------------------------------

struct FactorioPlayers;

fn check_name(n: &str) -> Result<()> {
    let ok = !n.is_empty()
        && n.chars().count() <= 60
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "\"{n}\" isn't a valid Factorio player name."
        )))
    }
}

fn read_names(path: &Path) -> Vec<String> {
    load_json(path)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

fn write_names(path: &Path, names: &[String]) -> Result<()> {
    save_json(path, &serde_json::json!(names))
}

/// Ban list entries are names or `{ "username": ..., "reason": ... }`.
fn read_bans(path: &Path) -> Vec<(String, Option<String>)> {
    load_json(path)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| match v {
            Value::String(s) => Some((s.clone(), None)),
            Value::Object(o) => Some((
                o.get("username")?.as_str()?.to_owned(),
                o.get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .filter(|r| !r.is_empty()),
            )),
            _ => None,
        })
        .collect()
}

fn write_bans(path: &Path, bans: &[(String, Option<String>)]) -> Result<()> {
    let list: Vec<Value> = bans
        .iter()
        .map(|(n, r)| serde_json::json!({ "username": n, "reason": r.clone().unwrap_or_default() }))
        .collect();
    save_json(path, &Value::Array(list))
}

fn toggle(list: &mut Vec<String>, name: &str, on: bool) {
    list.retain(|n| !n.eq_ignore_ascii_case(name));
    if on {
        list.push(name.to_owned());
    }
}

fn reason_or(r: &Option<String>, default: &str) -> String {
    crate::players::clean_reason(r).unwrap_or_else(|| default.to_owned())
}

#[async_trait]
impl PlayerAdmin for FactorioPlayers {
    fn caps(&self) -> PlayerCaps {
        PlayerCaps {
            roles: vec![Role::Player, Role::Operator],
            can_ban: true,
            ban_reason: true,
            game_modes: vec![],
            has_whitelist: true,
            whitelist_label: "Whitelist",
            role_needs_id: false,
        }
    }

    fn list(&self, dir: &Path, online: &[String], seen: &[SeenPlayer]) -> Result<Vec<PlayerEntry>> {
        let admins = read_names(&dir.join(ADMINS));
        let white = read_names(&dir.join(WHITELIST));
        let bans = read_bans(&dir.join(BANS));
        let mut names: Vec<String> = vec![];
        let all = online
            .iter()
            .chain(seen.iter().map(|s| &s.name))
            .chain(admins.iter())
            .chain(white.iter())
            .chain(bans.iter().map(|(n, _)| n));
        for n in all {
            if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                names.push(n.clone());
            }
        }
        let has = |list: &[String], n: &str| list.iter().any(|x| x.eq_ignore_ascii_case(n));
        let mut out: Vec<PlayerEntry> = names
            .into_iter()
            .map(|name| {
                let ban = bans.iter().find(|(b, _)| b.eq_ignore_ascii_case(&name));
                PlayerEntry {
                    online: has(online, &name),
                    role: if has(&admins, &name) {
                        Role::Operator
                    } else {
                        Role::Player
                    },
                    whitelisted: has(&white, &name),
                    banned: ban.is_some(),
                    ban_reason: ban.and_then(|(_, r)| r.clone()),
                    id: None,
                    name,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    fn whitelist_enabled(&self, dir: &Path) -> bool {
        whitelist_on(dir)
    }

    fn commands(&self, name: &str, action: &PlayerAction) -> Result<Vec<String>> {
        check_name(name)?;
        Ok(vec![match action {
            PlayerAction::SetRole { role: Role::Operator } => format!("/promote {name}"),
            PlayerAction::SetRole { role: Role::Player } => format!("/demote {name}"),
            PlayerAction::SetRole { .. } => {
                return Err(Error::InvalidInput(
                    "Factorio players are either players or admins.".into(),
                ))
            }
            PlayerAction::Whitelist { on: true } => format!("/whitelist add {name}"),
            PlayerAction::Whitelist { on: false } => format!("/whitelist remove {name}"),
            PlayerAction::Ban { reason } => format!("/ban {name} {}", reason_or(reason, "Banned by the host.")),
            PlayerAction::Unban => format!("/unban {name}"),
            PlayerAction::Kick { reason } => format!("/kick {name} {}", reason_or(reason, "Kicked by the host.")),
            PlayerAction::GameMode { .. } => {
                return Err(Error::InvalidInput("Factorio doesn't have game modes.".into()))
            }
        }])
    }

    fn live_needs_file_write(&self, action: &PlayerAction) -> bool {
        // Keep the lists on disk right even if the server only saves them when it exits.
        !matches!(action, PlayerAction::Kick { .. } | PlayerAction::GameMode { .. })
    }

    async fn apply_live(
        &self,
        _dir: &Path,
        _name: &str,
        _seen: &[SeenPlayer],
        _action: &PlayerAction,
    ) -> Option<Result<()>> {
        None
    }

    async fn apply_offline(
        &self,
        _ctx: &GameContext,
        dir: &Path,
        name: &str,
        _seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Result<()> {
        check_name(name)?;
        match action {
            PlayerAction::SetRole { role } => {
                let path = dir.join(ADMINS);
                let mut list = read_names(&path);
                toggle(&mut list, name, *role == Role::Operator);
                write_names(&path, &list)
            }
            PlayerAction::Whitelist { on } => {
                let path = dir.join(WHITELIST);
                let mut list = read_names(&path);
                toggle(&mut list, name, *on);
                write_names(&path, &list)
            }
            PlayerAction::Ban { reason } => {
                let path = dir.join(BANS);
                let mut bans = read_bans(&path);
                bans.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
                bans.push((name.to_owned(), crate::players::clean_reason(reason)));
                write_bans(&path, &bans)
            }
            PlayerAction::Unban => {
                let path = dir.join(BANS);
                let mut bans = read_bans(&path);
                bans.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
                write_bans(&path, &bans)
            }
            PlayerAction::Kick { .. } | PlayerAction::GameMode { .. } => Err(Error::Conflict(
                "Start the server first: the player has to be online for that.".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mod_zip(path: &Path, name: &str, title: &str, version: &str) {
        use std::io::Write;
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file(format!("{name}_{version}/info.json"), opts).unwrap();
        write!(
            z,
            r#"{{"name":"{name}","title":"{title}","version":"{version}","factorio_version":"2.0"}}"#
        )
        .unwrap();
        z.start_file(format!("{name}_{version}/control.lua"), opts).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn parses_console_lines() {
        use LineEvent::*;
        let p = parse_factorio_line;
        assert_eq!(
            p("   2.345 Info ServerMultiplayerManager.cpp:807: updateTick(18446744073709551615) changing state from(CreatingGame) to(InGame)"),
            Some(Ready)
        );
        assert_eq!(
            p("2026-11-12 13:00:00 [JOIN] Bacon_Dude117 joined the game"),
            Some(PlayerJoined("Bacon_Dude117".into()))
        );
        assert_eq!(
            p("2026-11-12 13:05:00 [LEAVE] Bacon_Dude117 left the game"),
            Some(PlayerLeft("Bacon_Dude117".into()))
        );
        assert_eq!(
            p("2026-11-12 13:01:00 [CHAT] Bacon_Dude117: [JOIN] x joined the game?"),
            None
        );
        assert_eq!(p("   1.000 Info Hello"), None);
    }

    #[test]
    fn settings_keep_their_types_and_hide_logins() {
        let example = r#"{"name":"Name","max_players":0,"visibility":{"public":true,"lan":true},"username":"u","token":"t","auto_pause":true,"allow_commands":"admins-only"}"#;
        let mut v = initial_settings(Some(example), "Factory");
        assert_eq!(v["name"], "Factory");
        assert_eq!(v["visibility"]["public"], false, "public listing needs a login");
        assert_eq!(v["token"], "");
        let entries = settings_entries(&v);
        assert!(entries.contains(&("max_players".into(), "0".into())));
        assert!(entries.contains(&("auto_pause".into(), "true".into())));
        assert!(!entries.iter().any(|(k, _)| k == "token" || k == "username"));

        set_setting(&mut v, "max_players", "8").unwrap();
        set_setting(&mut v, "auto_pause", "false").unwrap();
        set_setting(&mut v, "game_password", "hunter2").unwrap();
        assert_eq!(v["max_players"], 8);
        assert_eq!(v["auto_pause"], false);
        assert_eq!(v["game_password"], "hunter2");
        assert!(set_setting(&mut v, "max_players", "lots").is_err());
        assert!(set_setting(&mut v, "auto_pause", "maybe").is_err());
        assert!(set_setting(&mut v, "token", "x").is_err());
        // Without the example file, Hearth's defaults still make a working config.
        let bare = initial_settings(None, "F");
        assert_eq!(bare["require_user_verification"], true);
    }

    #[test]
    fn settings_round_trip_with_whitelist_switch() {
        let t = tempfile::tempdir().unwrap();
        save_json(&t.path().join(SETTINGS), &initial_settings(None, "F")).unwrap();
        Factorio
            .write_settings(
                t.path(),
                &[("name".into(), "New".into()), (WHITELIST_KEY.into(), "true".into())],
            )
            .unwrap();
        let s = Factorio.read_settings(t.path()).unwrap();
        assert!(s.contains(&("name".into(), "New".into())));
        assert!(s.contains(&(WHITELIST_KEY.into(), "true".into())));
        assert!(FactorioPlayers.whitelist_enabled(t.path()));
    }

    #[test]
    fn mod_list_follows_the_mods_folder() {
        let t = tempfile::tempdir().unwrap();
        let mods = t.path().join(MODS);
        std::fs::create_dir_all(&mods).unwrap();
        write_mod_zip(
            &mods.join("even-distribution_2.0.2.zip"),
            "even-distribution",
            "Even Distribution",
            "2.0.2",
        );
        write_mod_zip(&mods.join("helmod_2.0.1.zip.disabled"), "helmod", "Helmod", "2.0.1");
        std::fs::write(
            mods.join("mod-list.json"),
            r#"{"mods":[{"name":"base","enabled":true},{"name":"space-age","enabled":false},{"name":"gone","enabled":true}]}"#,
        )
        .unwrap();
        write_mod_list(t.path()).unwrap();
        let list = read_mod_list(&mods.join("mod-list.json"));
        let get = |n: &str| list.iter().find(|e| e.name == n).map(|e| e.enabled);
        assert_eq!(get("base"), Some(true));
        assert_eq!(get("space-age"), Some(false), "expansion state kept");
        assert_eq!(get("even-distribution"), Some(true));
        assert_eq!(get("helmod"), Some(false));
        assert_eq!(get("gone"), None, "mods no longer on the server are dropped");
        assert_eq!(
            Factorio.describe_content(&mods.join("even-distribution_2.0.2.zip")),
            Some(("Even Distribution".into(), Some("2.0.2".into())))
        );
    }

    #[test]
    fn copies_the_players_enabled_mods_and_expansions() {
        let t = tempfile::tempdir().unwrap();
        let client = t.path().join("client/mods");
        let server = t.path().join("server");
        std::fs::create_dir_all(&client).unwrap();
        std::fs::create_dir_all(server.join(MODS)).unwrap();
        write_mod_zip(&client.join("helmod_2.0.0.zip"), "helmod", "Helmod", "2.0.0");
        write_mod_zip(&client.join("helmod_2.0.1.zip"), "helmod", "Helmod", "2.0.1");
        write_mod_zip(
            &client.join("squeak-through_1.0.zip"),
            "squeak-through",
            "Squeak",
            "1.0",
        );
        // An old copy on the server is replaced.
        write_mod_zip(&server.join("mods/helmod_1.9.0.zip"), "helmod", "Helmod", "1.9.0");
        std::fs::write(
            client.join("mod-list.json"),
            r#"{"mods":[{"name":"base","enabled":true},{"name":"space-age","enabled":true},{"name":"quality","enabled":true},{"name":"helmod","enabled":true},{"name":"squeak-through","enabled":false},{"name":"missing-mod","enabled":true}]}"#,
        )
        .unwrap();
        let r = copy_client_mods(Some(&client), &server).unwrap();
        assert_eq!(r.copied, vec!["helmod v2.0.1"]);
        assert_eq!(r.missing, vec!["missing-mod"]);
        assert!(server.join("mods/helmod_2.0.1.zip").exists());
        assert!(!server.join("mods/helmod_1.9.0.zip").exists());
        assert!(
            !server.join("mods/squeak-through_1.0.zip").exists(),
            "disabled in the game"
        );
        let list = read_mod_list(&server.join("mods/mod-list.json"));
        let get = |n: &str| list.iter().find(|e| e.name == n).map(|e| e.enabled);
        assert_eq!(get("space-age"), Some(true));
        assert_eq!(get("quality"), Some(true));
        assert_eq!(get("helmod"), Some(true));

        assert!(copy_client_mods(None, &server)
            .unwrap_err()
            .to_string()
            .contains("Start Factorio once"));
    }

    #[test]
    fn player_lists_and_commands() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let ctx = GameContext::new(crate::paths::AppPaths::new(d.join("app")), crate::http::client());
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let act = |name: &str, a: PlayerAction| rt.block_on(FactorioPlayers.apply_offline(&ctx, d, name, &[], &a));
        act("Bacon", PlayerAction::SetRole { role: Role::Operator }).unwrap();
        act("Friend", PlayerAction::Whitelist { on: true }).unwrap();
        act(
            "Griefer",
            PlayerAction::Ban {
                reason: Some("Blew up the base".into()),
            },
        )
        .unwrap();
        let list = FactorioPlayers.list(d, &["Bacon".into()], &[]).unwrap();
        let get = |n: &str| list.iter().find(|p| p.name == n).unwrap().clone();
        assert!(get("Bacon").online && get("Bacon").role == Role::Operator);
        assert!(get("Friend").whitelisted);
        assert_eq!(get("Griefer").ban_reason.as_deref(), Some("Blew up the base"));
        act("Griefer", PlayerAction::Unban).unwrap();
        act("Bacon", PlayerAction::SetRole { role: Role::Player }).unwrap();
        let list = FactorioPlayers.list(d, &[], &[]).unwrap();
        assert!(list.iter().all(|p| !p.banned && p.role == Role::Player));
        assert!(act("Bacon", PlayerAction::Kick { reason: None }).is_err());

        let cmd = |a: PlayerAction| FactorioPlayers.commands("Bacon", &a).unwrap();
        assert_eq!(
            cmd(PlayerAction::SetRole { role: Role::Operator }),
            vec!["/promote Bacon"]
        );
        assert_eq!(
            cmd(PlayerAction::Ban {
                reason: Some("Griefing".into())
            }),
            vec!["/ban Bacon Griefing"]
        );
        assert_eq!(
            cmd(PlayerAction::Kick { reason: None }),
            vec!["/kick Bacon Kicked by the host."]
        );
        assert_eq!(
            cmd(PlayerAction::Whitelist { on: false }),
            vec!["/whitelist remove Bacon"]
        );
        assert!(FactorioPlayers.commands("Bad Name", &PlayerAction::Unban).is_err());
    }
}
