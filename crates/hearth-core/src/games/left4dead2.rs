//! Left 4 Dead 2: Valve's Source-engine dedicated server (SteamCMD app 222860, anonymous).
//!
//! On Windows `srcds.exe` draws its own console and doesn't read piped input, so Hearth talks to
//! it over RCON (commands, `status` for players, `quit` to stop) and shows its output by
//! following the `-condebug` log (`left4dead2/console.log`).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use super::{
    ClientImport, FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome, LaunchSpec, LineEvent,
    Progress, RemotePlayer,
};
use crate::content::ContentFolder;
use crate::error::{Error, IoContext, Result};
use crate::instance::Instance;
use crate::net::Protocol;
use crate::players::{PlayerAction, PlayerAdmin, PlayerCaps, PlayerEntry, Role, SeenPlayer};
use crate::steamcmd;

pub const APP_ID: u32 = 222860;
const GAME: &str = "left4dead2";
const HEARTH_FILE: &str = "hearth-l4d2.json";
const ADDONS: &str = "left4dead2/addons";
/// First line of server.cfg. ASCII only: Source misreads other characters.
const HEADER: &str = "// Written by Hearth. Change these in Hearth's Settings tab.";

/// Official campaigns: first map and name.
pub const CAMPAIGNS: &[(&str, &str)] = &[
    ("c1m1_hotel", "Dead Center"),
    ("c2m1_highway", "Dark Carnival"),
    ("c3m1_plankcountry", "Swamp Fever"),
    ("c4m1_milltown_a", "Hard Rain"),
    ("c5m1_waterfront", "The Parish"),
    ("c6m1_riverbank", "The Passing"),
    ("c7m1_docks", "The Sacrifice"),
    ("c8m1_apartment", "No Mercy"),
    ("c9m1_alleys", "Crash Course"),
    ("c10m1_caves", "Death Toll"),
    ("c11m1_greenhouse", "Dead Air"),
    ("c12m1_hilltop", "Blood Harvest"),
    ("c13m1_alpinecreek", "Cold Stream"),
    ("c14m1_junkyard", "The Last Stand"),
];
pub const GAME_MODES: &[&str] = &["coop", "realism", "versus", "survival", "scavenge"];

pub struct Left4Dead2;

fn executable(dir: &Path) -> PathBuf {
    dir.join(if cfg!(windows) { "srcds.exe" } else { "srcds_run" })
}

fn cfg_dir(dir: &Path) -> PathBuf {
    dir.join(GAME).join("cfg")
}

fn server_cfg(dir: &Path) -> PathBuf {
    cfg_dir(dir).join("server.cfg")
}

fn banned_cfg(dir: &Path) -> PathBuf {
    cfg_dir(dir).join("banned_user.cfg")
}

// ---- server.cfg ----------------------------------------------------------------------------------

/// A Source `.cfg`: one `name value` per line (value optionally quoted), `//` comments, and
/// other commands such as `exec`. Unknown lines are kept as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceCfg {
    lines: Vec<String>,
}

fn split_line(line: &str) -> Option<(String, String)> {
    let code = match line.find("//") {
        // `//` outside quotes starts a comment.
        Some(i) if line[..i].matches('"').count().is_multiple_of(2) => &line[..i],
        _ => line,
    };
    let code = code.trim();
    let (k, v) = code.split_once(char::is_whitespace).unwrap_or((code, ""));
    if k.is_empty() {
        return None;
    }
    let v = v.trim();
    let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(v);
    Some((k.to_owned(), v.to_owned()))
}

impl SourceCfg {
    pub fn parse(text: &str) -> Self {
        Self {
            lines: text.lines().map(str::to_owned).collect(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Ok(Self::parse(&std::fs::read_to_string(path).at(path)?))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).at(d)?;
        }
        std::fs::write(path, self.lines.join("\n") + "\n").at(path)
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.lines
            .iter()
            .rev()
            .filter_map(|l| split_line(l))
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    /// `name value` settings (not `exec` and friends).
    pub fn entries(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = vec![];
        for (k, v) in self.lines.iter().filter_map(|l| split_line(l)) {
            if v.is_empty() || matches!(k.as_str(), "exec" | "banid" | "removeid" | "writeid") {
                continue;
            }
            out.retain(|(x, _)| !x.eq_ignore_ascii_case(&k));
            out.push((k, v));
        }
        out
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key} \"{}\"", value.replace('"', "'"));
        match self
            .lines
            .iter()
            .rposition(|l| split_line(l).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key)))
        {
            Some(i) => self.lines[i] = line,
            None => self.lines.push(line),
        }
    }

    pub fn has_line(&self, line: &str) -> bool {
        self.lines.iter().any(|l| l.trim().eq_ignore_ascii_case(line))
    }

    pub fn push_line(&mut self, line: &str) {
        self.lines.push(line.to_owned());
    }
}

fn random_password() -> String {
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(24)
        .map(char::from)
        .collect()
}

/// Writes Hearth's defaults on a new server; on every install makes sure RCON is set up and
/// players can join with `connect` (not only through a lobby).
pub(crate) fn prepare(dir: &Path, inst: &Instance, fresh: bool) -> Result<()> {
    let path = server_cfg(dir);
    let mut cfg = SourceCfg::load(&path)?;
    if fresh || cfg.entries().is_empty() {
        cfg = SourceCfg::default();
        cfg.push_line(HEADER);
        cfg.set("hostname", &inst.name);
        cfg.set("sv_password", "");
        cfg.set("z_difficulty", "Normal");
        cfg.set("sv_lan", "0");
    }
    // Source's cfg parser chokes on non-ASCII text, even in comments (0.11.0 wrote an arrow).
    cfg.lines
        .retain(|l| !(l.trim_start().starts_with("//") && l.contains("Written by Hearth") && !l.is_ascii()));
    if !cfg.lines.iter().any(|l| l == HEADER) {
        cfg.lines.insert(0, HEADER.to_owned());
    }
    if cfg.get("rcon_password").is_none_or(|p| p.is_empty()) {
        cfg.set("rcon_password", &random_password());
    }
    cfg.set("sv_allow_lobby_connect_only", "0");
    if !cfg.has_line("exec banned_user.cfg") {
        cfg.push_line("exec banned_user.cfg");
    }
    cfg.save(&path)?;
    if !banned_cfg(dir).exists() {
        std::fs::write(banned_cfg(dir), "").at(banned_cfg(dir))?;
    }
    std::fs::create_dir_all(dir.join(ADDONS)).at(dir.join(ADDONS))?;
    let hearth = dir.join(HEARTH_FILE);
    if fresh || !hearth.exists() {
        save_options(dir, "coop", "c1m1_hotel")?;
    }
    Ok(())
}

/// Game mode and starting map (launch options, not cvars in server.cfg).
fn options(dir: &Path) -> (String, String) {
    let v: Value = std::fs::read_to_string(dir.join(HEARTH_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    let get = |k: &str, d: &str| v.get(k).and_then(Value::as_str).unwrap_or(d).to_owned();
    (get("gamemode", "coop"), get("map", "c1m1_hotel"))
}

fn save_options(dir: &Path, gamemode: &str, map: &str) -> Result<()> {
    let path = dir.join(HEARTH_FILE);
    let v = serde_json::json!({ "gamemode": gamemode, "map": map });
    std::fs::write(&path, serde_json::to_string_pretty(&v).expect("json")).at(&path)
}

fn rcon_password(dir: &Path) -> Result<String> {
    SourceCfg::load(&server_cfg(dir))?
        .get("rcon_password")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Error::Conflict("The server has no remote console password. Try Settings → Update.".into()))
}

/// The game port is also the RCON (TCP) port. Hearth records it next to the options.
fn port_of(dir: &Path) -> Option<u16> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(HEARTH_FILE)).ok()?).ok()?;
    v.get("port")?.as_u64().and_then(|p| u16::try_from(p).ok())
}

fn set_port(dir: &Path, port: u16) -> Result<()> {
    let path = dir.join(HEARTH_FILE);
    let mut v: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    v["port"] = port.into();
    std::fs::write(&path, serde_json::to_string_pretty(&v).expect("json")).at(&path)
}

async fn rcon(dir: &Path, command: &str) -> Result<String> {
    let port = port_of(dir).ok_or_else(|| Error::Conflict("Start the server first.".into()))?;
    crate::rcon::run(port, &rcon_password(dir)?, command).await
}

// ---- status --------------------------------------------------------------------------------------

/// Players from `status`: `#  2 1 "Bacon" STEAM_1:0:12345 00:12 50 0 active 30000 1.2.3.4:27005`.
/// Bots (`BOT`) are skipped.
pub(crate) fn parse_status(text: &str) -> Vec<RemotePlayer> {
    text.lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let open = l.find('"')?;
            let close = l.rfind('"')?;
            if close <= open {
                return None;
            }
            let name = l[open + 1..close].to_owned();
            let id = l[close + 1..].split_whitespace().next()?.to_owned();
            if id.eq_ignore_ascii_case("BOT") || name.is_empty() {
                return None;
            }
            Some(RemotePlayer {
                name,
                id: id.starts_with("STEAM_").then_some(id),
            })
        })
        .collect()
}

// ---- add-ons -------------------------------------------------------------------------------------

/// Enabled add-ons from the game's `addonlist.txt` (`"workshop\123.vpk"  "1"`).
pub(crate) fn enabled_addons(addonlist: &str) -> Vec<(String, bool)> {
    addonlist
        .lines()
        .filter_map(|l| {
            let parts: Vec<&str> = l.split('"').collect();
            // ["", key, ws, value, ""]
            if parts.len() < 5 {
                return None;
            }
            let key = parts[1].trim();
            key.to_ascii_lowercase()
                .ends_with(".vpk")
                .then(|| (key.replace('\\', "/"), parts[3].trim() == "1"))
        })
        .collect()
}

/// Copies the add-ons enabled in the player's game (Workshop subscriptions and manual ones).
pub fn copy_client_addons(client_addons: Option<&Path>, server_dir: &Path) -> Result<ClientImport> {
    let client = client_addons.filter(|d| d.is_dir()).ok_or_else(|| {
        Error::InvalidInput("Couldn't find Left 4 Dead 2 on this PC. Install it from Steam, then try again.".into())
    })?;
    let listed = std::fs::read_to_string(client.join("..").join("addonlist.txt"))
        .or_else(|_| std::fs::read_to_string(client.join("addonlist.txt")))
        .map(|t| enabled_addons(&t))
        .unwrap_or_default();
    let mut wanted: Vec<String> = listed.iter().filter(|(_, on)| *on).map(|(k, _)| k.clone()).collect();
    // Add-ons in the folder but not in the list are on by default.
    for e in std::fs::read_dir(client).at(client)?.flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        if f.to_ascii_lowercase().ends_with(".vpk") && !listed.iter().any(|(k, _)| k.eq_ignore_ascii_case(&f)) {
            wanted.push(f);
        }
    }
    let dest = server_dir.join(ADDONS);
    std::fs::create_dir_all(&dest).at(&dest)?;
    let mut out = ClientImport::default();
    for rel in wanted {
        let src = client.join(&rel);
        let name = Path::new(&rel)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(rel.clone());
        if !src.is_file() {
            out.missing.push(name);
            continue;
        }
        crate::content::check_file_name(&name)?;
        std::fs::copy(&src, dest.join(&name)).at(&src)?;
        out.copied.push(name);
    }
    Ok(out)
}

// ---- provider ------------------------------------------------------------------------------------

#[async_trait]
impl GameProvider for Left4Dead2 {
    fn info(&self) -> GameInfo {
        GameInfo {
            id: "left4dead2",
            name: "Left 4 Dead 2",
            description: "Co-op zombie survival for 4 (8 in Versus). Friends join from the developer console with connect and your address.",
            flavors: vec![FlavorInfo {
                id: "dedicated",
                name: "Dedicated server",
                description: "Valve's dedicated server, downloaded through SteamCMD.",
            }],
            default_port: 27015,
            protocol: Protocol::Udp,
            configurable_memory: false,
            memory_limit: true,
            eula: None,
            playit_tunnel_type: None,
            command_hint: "Type a console command, e.g. status or changelevel c2m1_highway",
            setup_note: Some(
                "The first install downloads several GB through Steam. Players join from the developer console: turn on Options → Keyboard/Mouse → Allow Developer Console, press ~ and type connect with your address.",
            ),
        }
    }

    async fn versions(&self, _ctx: &GameContext, _flavor: &str) -> Result<Vec<GameVersion>> {
        Ok(vec![GameVersion {
            id: "latest".into(),
            label: "Latest (Steam)".into(),
            stable: true,
        }])
    }

    async fn install(
        &self,
        ctx: &GameContext,
        inst: &Instance,
        dir: &Path,
        fresh: bool,
        progress: Progress<'_>,
    ) -> Result<InstallOutcome> {
        ctx.steam.app_update(APP_ID, dir, progress).await?;
        prepare(dir, inst, fresh)?;
        set_port(dir, inst.port)?;
        Ok(InstallOutcome {
            installed_build: steamcmd::installed_build(dir, APP_ID).map(|b| format!("build {b}")),
            java_major: None,
            jvm_args: None,
            launch: None,
        })
    }

    fn launch(&self, _ctx: &GameContext, inst: &Instance, dir: &Path) -> Result<LaunchSpec> {
        let exe = executable(dir);
        if !exe.exists() {
            return Err(Error::InvalidInput(
                "The Left 4 Dead 2 server files are missing. Try reinstalling (Settings → Update).".into(),
            ));
        }
        prepare(dir, inst, false)?;
        set_port(dir, inst.port)?;
        let (mode, map) = options(dir);
        Ok(LaunchSpec {
            program: exe,
            args: vec![
                "-console".into(),
                "-game".into(),
                GAME.into(),
                "-port".into(),
                inst.port.to_string(),
                "-usercon".into(),
                "-condebug".into(),
                // `map <name> <mode>`: mp_gamemode can't be set from the command line.
                "+map".into(),
                map,
                mode,
            ],
            cwd: dir.to_path_buf(),
            env: vec![],
            console_encoding: Default::default(),
            memory_limit_mb: None,
            log_file: Some(dir.join(GAME).join("console.log")),
            own_console: true,
        })
    }

    fn parse_line(&self, _line: &str) -> Option<LineEvent> {
        // Readiness and players come from RCON `status` (see `poll`).
        None
    }

    fn stop_command(&self) -> &'static str {
        ""
    }

    async fn poll(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path) -> Option<Result<Vec<RemotePlayer>>> {
        Some(rcon(dir, "status").await.map(|s| parse_status(&s)))
    }

    async fn remote_stop(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path) -> bool {
        // The connection drops as the server exits, so the reply doesn't matter.
        match rcon(dir, "quit").await {
            Ok(_) => true,
            Err(Error::Conflict(m)) if m.contains("reach") => true,
            Err(_) => false,
        }
    }

    async fn console_input(
        &self,
        _ctx: &GameContext,
        _inst: &Instance,
        dir: &Path,
        line: &str,
    ) -> Option<Result<Option<String>>> {
        Some(rcon(dir, line).await.map(Some))
    }

    fn properties_file(&self) -> Option<&'static str> {
        None
    }

    fn ports_used(&self, port: u16) -> Vec<(u16, Protocol)> {
        // Game traffic is UDP; RCON listens on TCP on the same port.
        vec![(port, Protocol::Udp), (port, Protocol::Tcp)]
    }

    fn port_setting_key(&self) -> &'static str {
        "port"
    }

    fn apply_port(&self, dir: &Path, port: u16) -> Result<()> {
        set_port(dir, port)
    }

    fn read_settings(&self, dir: &Path) -> Result<Vec<(String, String)>> {
        let mut out: Vec<(String, String)> = SourceCfg::load(&server_cfg(dir))?
            .entries()
            .into_iter()
            .filter(|(k, _)| k != "rcon_password")
            .collect();
        let (mode, map) = options(dir);
        out.push(("gamemode".into(), mode));
        out.push(("map".into(), map));
        Ok(out)
    }

    fn write_settings(&self, dir: &Path, changes: &[(String, String)]) -> Result<()> {
        let path = server_cfg(dir);
        let mut cfg = SourceCfg::load(&path)?;
        let (mut mode, mut map) = options(dir);
        for (k, v) in changes {
            match k.as_str() {
                "rcon_password" => {
                    return Err(Error::InvalidInput(
                        "Hearth manages the remote console password; it can't be changed here.".into(),
                    ))
                }
                "gamemode" => {
                    if !GAME_MODES.contains(&v.as_str()) {
                        return Err(Error::InvalidInput(format!("Unknown game mode: {v}")));
                    }
                    mode = v.clone();
                }
                "map" => {
                    let ok = !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if !ok {
                        return Err(Error::InvalidInput(format!("\"{v}\" isn't a map name.")));
                    }
                    map = v.clone();
                }
                _ => cfg.set(k, v),
            }
        }
        cfg.save(&path)?;
        let port = port_of(dir);
        save_options(dir, &mode, &map)?;
        if let Some(p) = port {
            set_port(dir, p)?;
        }
        Ok(())
    }

    fn players(&self) -> Option<&dyn PlayerAdmin> {
        Some(&L4d2Players)
    }

    fn content_folder(&self, _inst: &Instance) -> Option<ContentFolder> {
        Some(ContentFolder {
            label: "Add-ons",
            folder: ADDONS,
            extension: "vpk",
            import_label: Some("Copy my add-ons from Left 4 Dead 2"),
        })
    }

    fn import_client_content(&self, _inst: &Instance, dir: &Path) -> Option<Result<ClientImport>> {
        let client = crate::steamlib::app_dir("Left 4 Dead 2").map(|d| d.join(GAME).join("addons"));
        Some(copy_client_addons(client.as_deref(), dir))
    }
}

// ---- players -------------------------------------------------------------------------------------

struct L4d2Players;

/// Steam ids banned in `banned_user.cfg` (`banid 0 STEAM_1:0:123`).
fn read_bans(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(banned_cfg(dir))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut p = l.split_whitespace();
            (p.next()? == "banid").then_some(())?;
            p.next()?;
            p.next().map(|id| id.trim_matches('"').to_owned())
        })
        .collect()
}

fn write_bans(dir: &Path, ids: &[String]) -> Result<()> {
    let text: String = ids.iter().map(|id| format!("banid 0 {id}\n")).collect();
    std::fs::write(banned_cfg(dir), text).at(banned_cfg(dir))
}

fn steam_id(name: &str, seen: &[SeenPlayer]) -> Option<String> {
    if name.starts_with("STEAM_") {
        return Some(name.to_owned());
    }
    seen.iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .and_then(|s| s.id.clone())
        .filter(|id| id.starts_with("STEAM_"))
}

fn need_id(name: &str) -> Error {
    Error::Conflict(format!(
        "Hearth hasn't seen {name}'s Steam ID yet. It's recorded while they're on the server."
    ))
}

fn check_id(id: &str) -> Result<()> {
    if id.starts_with("STEAM_") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!("\"{id}\" isn't a Steam ID.")))
    }
}

#[async_trait]
impl PlayerAdmin for L4d2Players {
    fn caps(&self) -> PlayerCaps {
        PlayerCaps {
            roles: vec![],
            can_ban: true,
            ban_reason: false,
            game_modes: vec![],
            has_whitelist: false,
            whitelist_label: "",
            role_needs_id: false,
        }
    }

    fn list(&self, dir: &Path, online: &[String], seen: &[SeenPlayer]) -> Result<Vec<PlayerEntry>> {
        let bans = read_bans(dir);
        let mut out: Vec<PlayerEntry> = vec![];
        let mut add = |name: String, id: Option<String>| {
            if out.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                return;
            }
            out.push(PlayerEntry {
                online: online.iter().any(|o| o.eq_ignore_ascii_case(&name)),
                role: Role::Player,
                whitelisted: false,
                banned: id.as_ref().is_some_and(|i| bans.contains(i)),
                ban_reason: None,
                id,
                name,
            });
        };
        for n in online {
            add(n.clone(), steam_id(n, seen));
        }
        for s in seen {
            add(s.name.clone(), s.id.clone());
        }
        for b in &bans {
            if !seen.iter().any(|s| s.id.as_deref() == Some(b.as_str())) {
                add(b.clone(), Some(b.clone()));
            }
        }
        out.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    fn whitelist_enabled(&self, _dir: &Path) -> bool {
        false
    }

    fn commands(&self, _name: &str, _action: &PlayerAction) -> Result<Vec<String>> {
        Err(Error::InvalidInput(
            "Left 4 Dead 2 is managed through its remote console.".into(),
        ))
    }

    async fn apply_live(
        &self,
        dir: &Path,
        name: &str,
        seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Option<Result<()>> {
        let id = steam_id(name, seen);
        let commands: Result<Vec<String>> = match action {
            PlayerAction::Kick { .. } => Ok(vec![match &id {
                Some(id) => format!("kickid {id}"),
                None => format!("kick \"{}\"", name.replace('"', "")),
            }]),
            PlayerAction::Ban { .. } => id
                .ok_or_else(|| need_id(name))
                .and_then(|id| check_id(&id).map(|_| id))
                .map(|id| vec![format!("banid 0 {id} kick"), "writeid".into()]),
            PlayerAction::Unban => id
                .ok_or_else(|| need_id(name))
                .and_then(|id| check_id(&id).map(|_| id))
                .map(|id| vec![format!("removeid {id}"), "writeid".into()]),
            _ => Err(Error::InvalidInput(
                "Left 4 Dead 2 servers don't have roles, whitelists or game modes.".into(),
            )),
        };
        Some(
            async {
                for c in commands? {
                    rcon(dir, &c).await?;
                }
                Ok(())
            }
            .await,
        )
    }

    async fn apply_offline(
        &self,
        _ctx: &GameContext,
        dir: &Path,
        name: &str,
        seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Result<()> {
        let id = steam_id(name, seen).ok_or_else(|| need_id(name))?;
        check_id(&id)?;
        let mut bans = read_bans(dir);
        match action {
            PlayerAction::Ban { .. } => {
                if !bans.contains(&id) {
                    bans.push(id);
                }
            }
            PlayerAction::Unban => bans.retain(|b| *b != id),
            _ => {
                return Err(Error::Conflict(
                    "Start the server first: the player has to be online for that.".into(),
                ))
            }
        }
        write_bans(dir, &bans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst() -> Instance {
        Instance {
            id: "l4d-0001".into(),
            name: "Zombie Night".into(),
            game: "left4dead2".into(),
            flavor: "dedicated".into(),
            version: "latest".into(),
            installed_build: None,
            port: 27015,
            memory_mb: None,
            java_major: None,
            jvm_args: vec![],
            install_state: crate::instance::InstallState::Installed,
            created_at: chrono::Utc::now(),
            playit_tunnel_id: None,
            launch: None,
            pack: None,
            voice: None,
        }
    }

    #[test]
    fn parses_status() {
        let text = "hostname: Zombie Night\nversion : 2.2.4.3 9256 secure\nudp/ip  : 0.0.0.0:27015\nmap     : c1m1_hotel\nplayers : 2 humans, 2 bots (4 max)\n# userid name uniqueid connected ping loss state rate adr\n#  2 1 \"Bacon \"Dude\"\" STEAM_1:0:12345 00:12 50 0 active 30000 1.2.3.4:27005\n#  3 2 \"Friend\" STEAM_1:1:999 01:02 70 0 active 30000 5.6.7.8:27005\n#  4 \"Coach\" BOT active\n#end\n";
        let p = parse_status(text);
        assert_eq!(
            p,
            vec![
                RemotePlayer {
                    name: "Bacon \"Dude\"".into(),
                    id: Some("STEAM_1:0:12345".into())
                },
                RemotePlayer {
                    name: "Friend".into(),
                    id: Some("STEAM_1:1:999".into())
                },
            ]
        );
        assert!(parse_status("players : 0 humans\n# userid name uniqueid\n#end").is_empty());
    }

    #[test]
    fn server_cfg_round_trip() {
        let mut cfg = SourceCfg::parse(
            "// comment\nhostname \"Old Name\" // trailing\nsv_password \"\"\nexec banned_user.cfg\nz_difficulty Hard\n",
        );
        assert_eq!(cfg.get("hostname").as_deref(), Some("Old Name"));
        assert_eq!(cfg.get("z_difficulty").as_deref(), Some("Hard"));
        cfg.set("hostname", "New \"Name\"");
        assert!(cfg.lines.iter().all(|l| l.is_ascii()));
        cfg.set("sv_region", "3");
        let text = cfg.lines.join("\n");
        assert!(text.contains("hostname \"New 'Name'\""));
        assert!(text.contains("exec banned_user.cfg"));
        assert!(text.ends_with("sv_region \"3\""));
        let entries = cfg.entries();
        assert!(entries.contains(&("z_difficulty".into(), "Hard".into())));
        assert!(!entries.iter().any(|(k, _)| k == "exec" || k == "sv_password"));
    }

    #[test]
    fn prepares_rcon_and_settings() {
        let t = tempfile::tempdir().unwrap();
        prepare(t.path(), &inst(), true).unwrap();
        set_port(t.path(), 27015).unwrap();
        let cfg = SourceCfg::load(&server_cfg(t.path())).unwrap();
        assert_eq!(cfg.get("hostname").as_deref(), Some("Zombie Night"));
        assert_eq!(cfg.get("sv_allow_lobby_connect_only").as_deref(), Some("0"));
        assert_eq!(rcon_password(t.path()).unwrap().len(), 24);
        assert!(cfg.has_line("exec banned_user.cfg"));
        assert_eq!(port_of(t.path()), Some(27015));

        // A 0.11.0 header with a non-ASCII arrow is replaced on the next start.
        let mut old = SourceCfg::load(&server_cfg(t.path())).unwrap();
        old.lines[0] = "// Written by Hearth. Settings → Game settings edits this file.".into();
        old.save(&server_cfg(t.path())).unwrap();
        prepare(t.path(), &inst(), false).unwrap();
        let text = std::fs::read_to_string(server_cfg(t.path())).unwrap();
        assert!(text.is_ascii(), "{text}");
        assert_eq!(text.lines().next(), Some(HEADER));

        // The password survives a reinstall and never shows in Settings.
        let pw = rcon_password(t.path()).unwrap();
        prepare(t.path(), &inst(), false).unwrap();
        assert_eq!(rcon_password(t.path()).unwrap(), pw);
        let s = Left4Dead2.read_settings(t.path()).unwrap();
        assert!(!s.iter().any(|(k, _)| k == "rcon_password"));
        assert!(s.contains(&("gamemode".into(), "coop".into())));
        assert!(s.contains(&("map".into(), "c1m1_hotel".into())));

        Left4Dead2
            .write_settings(
                t.path(),
                &[
                    ("gamemode".into(), "versus".into()),
                    ("map".into(), "c8m1_apartment".into()),
                    ("z_difficulty".into(), "Impossible".into()),
                ],
            )
            .unwrap();
        assert_eq!(options(t.path()), ("versus".into(), "c8m1_apartment".into()));
        assert_eq!(port_of(t.path()), Some(27015), "port kept");
        assert!(Left4Dead2
            .write_settings(t.path(), &[("gamemode".into(), "deathmatch".into())])
            .is_err());
        assert!(Left4Dead2
            .write_settings(t.path(), &[("map".into(), "c1; quit".into())])
            .is_err());
        assert!(Left4Dead2
            .write_settings(t.path(), &[("rcon_password".into(), "x".into())])
            .is_err());
    }

    #[test]
    fn offline_bans_by_steam_id() {
        let t = tempfile::tempdir().unwrap();
        prepare(t.path(), &inst(), true).unwrap();
        let ctx = GameContext::new(crate::paths::AppPaths::new(t.path().join("app")), crate::http::client());
        let seen = vec![SeenPlayer {
            name: "Griefer".into(),
            id: Some("STEAM_1:0:42".into()),
            last_seen: chrono::Utc::now(),
        }];
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let act = |n: &str, a: PlayerAction| rt.block_on(L4d2Players.apply_offline(&ctx, t.path(), n, &seen, &a));
        act("Griefer", PlayerAction::Ban { reason: None }).unwrap();
        assert_eq!(read_bans(t.path()), vec!["STEAM_1:0:42"]);
        let list = L4d2Players.list(t.path(), &[], &seen).unwrap();
        assert!(list[0].banned && list[0].name == "Griefer");
        act("Griefer", PlayerAction::Unban).unwrap();
        assert!(read_bans(t.path()).is_empty());
        let err = act("Stranger", PlayerAction::Ban { reason: None })
            .unwrap_err()
            .to_string();
        assert!(err.contains("Steam ID"), "{err}");
    }

    #[test]
    fn reads_addonlist_and_copies_enabled_addons() {
        let t = tempfile::tempdir().unwrap();
        let addons = t.path().join("client/left4dead2/addons");
        std::fs::create_dir_all(addons.join("workshop")).unwrap();
        std::fs::write(addons.join("workshop/111.vpk"), b"vpk").unwrap();
        std::fs::write(addons.join("workshop/222.vpk"), b"vpk").unwrap();
        std::fs::write(addons.join("mycampaign.vpk"), b"vpk").unwrap();
        std::fs::write(
            t.path().join("client/left4dead2/addonlist.txt"),
            "\"AddonList\"\n{\n\t\"workshop\\111.vpk\"\t\t\"1\"\n\t\"workshop\\222.vpk\"\t\t\"0\"\n\t\"workshop\\333.vpk\"\t\t\"1\"\n}\n",
        )
        .unwrap();
        let server = t.path().join("server");
        let r = copy_client_addons(Some(&addons), &server).unwrap();
        assert_eq!(r.copied, vec!["111.vpk", "mycampaign.vpk"]);
        assert_eq!(r.missing, vec!["333.vpk"]);
        assert!(server.join(ADDONS).join("111.vpk").exists());
        assert!(!server.join(ADDONS).join("222.vpk").exists(), "disabled in the game");
    }
}
