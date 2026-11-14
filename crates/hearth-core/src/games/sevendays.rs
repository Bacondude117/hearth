//! 7 Days to Die: The Fun Pimps' dedicated server (SteamCMD app 294420, anonymous).
//!
//! It's a Unity program: it logs to a file (`-logfile`), not a console, and is administered over
//! its Telnet console (`lp` to list players, `kick`, `ban add`, `admin add`, `shutdown`).
//! Hearth keeps its own config file (`hearth-serverconfig.xml`, which Steam updates leave alone)
//! with saves in the server folder, Telnet on a random password, and the Windows terminal
//! window turned off.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{
    FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome, LaunchSpec, LineEvent, Progress,
    RemotePlayer,
};
use crate::error::{Error, IoContext, Result};
use crate::instance::Instance;
use crate::net::Protocol;
use crate::players::{PlayerAction, PlayerAdmin, PlayerCaps, PlayerEntry, Role, SeenPlayer};
use crate::steamcmd;

pub const APP_ID: u32 = 294420;
pub const CONFIG: &str = "hearth-serverconfig.xml";
const LOG: &str = "logs/hearth-server.log";

pub struct SevenDays;

fn executable(dir: &Path) -> PathBuf {
    dir.join(if cfg!(windows) {
        "7DaysToDieServer.exe"
    } else {
        "7DaysToDieServer.x86_64"
    })
}

fn user_data(dir: &Path) -> PathBuf {
    dir.join("UserData")
}

/// Telnet sits 1000 above the game port (below it if that would overflow).
pub fn telnet_port(game_port: u16) -> u16 {
    game_port.checked_add(1000).unwrap_or(game_port - 1000)
}

// ---- serverconfig.xml ----------------------------------------------------------------------------

/// `<property name="X" value="Y" />` lines. Everything else (comments, layout) is kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerConfig {
    text: String,
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Value of `attr="..."` inside one tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!("{name}=\"");
    let start = tag.find(&pat)? + pat.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Byte ranges of each `<property .../>` tag outside comments.
fn property_tags(text: &str) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut i = 0;
    while let Some(off) = text[i..].find('<') {
        let start = i + off;
        if text[start..].starts_with("<!--") {
            i = text[start..].find("-->").map(|e| start + e + 3).unwrap_or(text.len());
            continue;
        }
        let Some(len) = text[start..].find('>') else { break };
        let end = start + len + 1;
        if text[start..end].starts_with("<property") {
            out.push((start, end));
        }
        i = end;
    }
    out
}

impl ServerConfig {
    pub fn parse(text: &str) -> Self {
        Self { text: text.to_owned() }
    }

    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self::parse(&std::fs::read_to_string(path).at(path)?))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, &self.text).at(path)
    }

    pub fn entries(&self) -> Vec<(String, String)> {
        property_tags(&self.text)
            .into_iter()
            .filter_map(|(s, e)| {
                let tag = &self.text[s..e];
                Some((attr(tag, "name")?.to_owned(), xml_unescape(attr(tag, "value")?)))
            })
            .collect()
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.entries().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let new_tag = format!("<property name=\"{key}\" value=\"{}\" />", xml_escape(value));
        let found = property_tags(&self.text)
            .into_iter()
            .find(|&(s, e)| attr(&self.text[s..e], "name") == Some(key));
        match found {
            Some((s, e)) => self.text.replace_range(s..e, &new_tag),
            None => {
                let at = self.text.rfind("</ServerSettings>").unwrap_or(self.text.len());
                self.text.insert_str(at, &format!("\t{new_tag}\n"));
            }
        }
    }
}

/// Settings Hearth manages; never shown or edited in Settings.
const MANAGED: &[&str] = &[
    "TelnetEnabled",
    "TelnetPort",
    "TelnetPassword",
    "TelnetFailedLoginLimit",
    "TerminalWindowEnabled",
    "UserDataFolder",
    "SaveGameFolder",
    "AdminFileName",
    "WebDashboardEnabled",
];

fn random_password() -> String {
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(24)
        .map(char::from)
        .collect()
}

/// Writes Hearth's config from the game's own `serverconfig.xml` on a new server, then (always)
/// the settings Hearth relies on.
pub(crate) fn prepare(dir: &Path, inst: &Instance, fresh: bool) -> Result<()> {
    let path = dir.join(CONFIG);
    let mut cfg = if fresh || !path.exists() {
        let base = dir.join("serverconfig.xml");
        let mut c = if base.exists() {
            ServerConfig::load(&base)?
        } else {
            ServerConfig::parse("<?xml version=\"1.0\"?>\n<ServerSettings>\n</ServerSettings>\n")
        };
        c.set("ServerName", &inst.name);
        c.set("ServerDescription", "Hosted with Hearth");
        c.set("ServerPassword", "");
        c.set("GameName", &inst.name);
        // Friends join by address; not listed publicly unless the host chooses to.
        c.set("ServerVisibility", "0");
        c
    } else {
        ServerConfig::load(&path)?
    };
    cfg.set("ServerPort", &inst.port.to_string());
    cfg.set("TelnetEnabled", "true");
    cfg.set("TelnetPort", &telnet_port(inst.port).to_string());
    if cfg.get("TelnetPassword").is_none_or(|p| p.is_empty()) {
        cfg.set("TelnetPassword", &random_password());
    }
    cfg.set("TerminalWindowEnabled", "false");
    cfg.set("WebDashboardEnabled", "false");
    cfg.set("UserDataFolder", &user_data(dir).display().to_string());
    cfg.save(&path)?;
    std::fs::create_dir_all(dir.join("logs")).at(dir.join("logs"))?;
    Ok(())
}

// ---- telnet --------------------------------------------------------------------------------------

/// A log line streamed over Telnet (`2026-11-14T12:00:00 123.456 INF ...`), as opposed to a
/// command's reply.
fn is_log_line(l: &str) -> bool {
    let b = l.as_bytes();
    b.len() > 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && (l.contains(" INF ") || l.contains(" WRN ") || l.contains(" ERR "))
}

pub struct Telnet {
    stream: TcpStream,
    buf: Vec<u8>,
}

fn unreachable_err(e: impl std::fmt::Display) -> Error {
    Error::Conflict(format!("Couldn't reach the server's admin console: {e}"))
}

impl Telnet {
    /// Connects to the server on this PC and logs in.
    pub async fn connect(port: u16, password: &str) -> Result<Self> {
        let stream = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(("127.0.0.1", port)))
            .await
            .map_err(|_| unreachable_err("timed out"))?
            .map_err(unreachable_err)?;
        let mut t = Self { stream, buf: vec![] };
        t.read_until(|text| text.contains("password:"), Duration::from_secs(5))
            .await?;
        t.send(password).await?;
        let reply = t
            .read_until(
                |text| text.contains("Logon successful") || text.contains("Password incorrect"),
                Duration::from_secs(5),
            )
            .await?;
        if !reply.contains("Logon successful") {
            return Err(Error::Conflict(
                "The server refused Hearth's admin password. Restart the server to apply it.".into(),
            ));
        }
        Ok(t)
    }

    async fn send(&mut self, line: &str) -> Result<()> {
        self.stream
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .map_err(unreachable_err)
    }

    /// Reads until `done` matches the text received so far, or `wait` passes. Returns the text.
    async fn read_until(&mut self, done: impl Fn(&str) -> bool, wait: Duration) -> Result<String> {
        self.buf.clear();
        let deadline = tokio::time::Instant::now() + wait;
        let mut chunk = [0u8; 4096];
        loop {
            let text = String::from_utf8_lossy(&self.buf).into_owned();
            if done(&text) {
                return Ok(text);
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Ok(text);
            }
            match tokio::time::timeout(left, self.stream.read(&mut chunk)).await {
                Ok(Ok(0)) => return Err(unreachable_err("connection closed")),
                Ok(Ok(n)) => self.buf.extend_from_slice(&chunk[..n]),
                Ok(Err(e)) => return Err(unreachable_err(e)),
                Err(_) => return Ok(text),
            }
        }
    }

    /// Runs a command and returns its reply (streamed log lines filtered out). Replies have no
    /// end marker, so this waits for `done` or a short quiet period.
    pub async fn exec(&mut self, command: &str, done: impl Fn(&str) -> bool) -> Result<String> {
        self.send(command).await?;
        let text = self.read_until(done, Duration::from_millis(1500)).await?;
        Ok(text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty() && !is_log_line(l))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn telnet_login(dir: &Path) -> Result<(u16, String)> {
    let cfg = ServerConfig::load(&dir.join(CONFIG))?;
    let port = cfg
        .get("TelnetPort")
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| Error::Conflict("The server's admin console isn't set up. Try Settings → Update.".into()))?;
    let pw = cfg
        .get("TelnetPassword")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Error::Conflict("The server's admin console isn't set up. Try Settings → Update.".into()))?;
    Ok((port, pw))
}

async fn telnet(dir: &Path, command: &str, done: impl Fn(&str) -> bool) -> Result<String> {
    let (port, pw) = telnet_login(dir)?;
    let mut t = Telnet::connect(port, &pw).await?;
    t.exec(command, done).await
}

// ---- players & readiness -------------------------------------------------------------------------

/// `lp`: `0. id=171, Bacon Dude, pos=(...), ..., pltfmid=Steam_7656..., crossid=EOS_..., ip=..., ping=20`
/// (older builds: `steamid=7656...`).
pub(crate) fn parse_players(text: &str) -> Vec<RemotePlayer> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (num, rest) = l.split_once(". id=")?;
            if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let (_, rest) = rest.split_once(", ")?;
            let (name, rest) = rest.split_once(", pos=")?;
            let field = |k: &str| {
                rest.split(", ")
                    .find_map(|f| f.strip_prefix(k))
                    .map(|v| v.trim().to_owned())
            };
            let id = field("pltfmid=").or_else(|| field("steamid=").map(|s| format!("Steam_{s}")));
            Some(RemotePlayer {
                name: name.to_owned(),
                id: id.filter(|i| !i.is_empty()),
            })
        })
        .collect()
}

/// Servers whose log has shown "StartGame done" since they were started. The world (and on
/// first start, world generation) is only ready then, although Telnet answers earlier.
static STARTED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn started(dir: &Path) -> bool {
    STARTED.lock().unwrap().as_ref().is_some_and(|s| s.contains(dir))
}

fn set_started(dir: &Path, on: bool) {
    let mut g = STARTED.lock().unwrap();
    let set = g.get_or_insert_with(HashSet::new);
    if on {
        set.insert(dir.to_path_buf());
    } else {
        set.remove(dir);
    }
}

pub(crate) fn parse_7d_line(line: &str) -> Option<LineEvent> {
    if line.contains(" StartGame done") {
        return Some(LineEvent::Ready);
    }
    let player_name = |l: &str| {
        let (_, rest) = l.split_once("PlayerName='")?;
        // The name ends at the next `', Field=` (or the last quote on the line).
        let name = match rest.split_once("', ") {
            Some((n, _)) => n,
            None => rest.rsplit_once('\'')?.0,
        };
        Some(name.to_owned())
    };
    if line.contains("PlayerSpawnedInWorld") && line.contains("JoinMultiplayer") {
        return player_name(line).map(LineEvent::PlayerJoined);
    }
    if line.contains("Player disconnected:") {
        return player_name(line).map(LineEvent::PlayerLeft);
    }
    None
}

// ---- provider ------------------------------------------------------------------------------------

#[async_trait]
impl GameProvider for SevenDays {
    fn info(&self) -> GameInfo {
        GameInfo {
            id: "sevendays",
            name: "7 Days to Die",
            description: "Survive the zombie hordes together. Friends join from Join a Game → Connect to IP.",
            flavors: vec![FlavorInfo {
                id: "dedicated",
                name: "Dedicated server",
                description: "The Fun Pimps' dedicated server, downloaded through SteamCMD.",
            }],
            default_port: 26900,
            protocol: Protocol::Udp,
            configurable_memory: false,
            memory_limit: true,
            eula: None,
            playit_tunnel_type: None,
            command_hint: "Type a console command, e.g. say \"hello\", lp or saveworld",
            setup_note: Some(
                "The first install downloads over 10 GB through Steam. The first start also generates the world, which can take several minutes; the server shows Online when it's ready. 7 Days to Die needs at least 8 GB of memory.",
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
                "The 7 Days to Die server files are missing. Try reinstalling (Settings → Update).".into(),
            ));
        }
        prepare(dir, inst, false)?;
        set_started(dir, false);
        let log = dir.join(LOG);
        Ok(LaunchSpec {
            program: exe,
            args: vec![
                "-quit".into(),
                "-batchmode".into(),
                "-nographics".into(),
                "-dedicated".into(),
                format!("-configfile={}", dir.join(CONFIG).display()),
                "-logfile".into(),
                log.display().to_string(),
            ],
            cwd: dir.to_path_buf(),
            env: vec![],
            console_encoding: Default::default(),
            memory_limit_mb: None,
            log_file: Some(log),
            own_console: false,
        })
    }

    fn parse_line(&self, line: &str) -> Option<LineEvent> {
        parse_7d_line(line)
    }

    fn stop_command(&self) -> &'static str {
        ""
    }

    async fn poll(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path) -> Option<Result<Vec<RemotePlayer>>> {
        if !started(dir) {
            let log = std::fs::read_to_string(dir.join(LOG)).unwrap_or_default();
            if !log.contains(" StartGame done") {
                return Some(Err(Error::Conflict("The world is still loading.".into())));
            }
            set_started(dir, true);
        }
        Some(
            telnet(dir, "lp", |t| t.contains("in the game"))
                .await
                .map(|t| parse_players(&t)),
        )
    }

    async fn remote_stop(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path) -> bool {
        // Saves the world, then exits. The connection drops as it does, so only a failed
        // login counts as "didn't answer".
        let Ok((port, pw)) = telnet_login(dir) else {
            return false;
        };
        let Ok(mut t) = Telnet::connect(port, &pw).await else {
            return false;
        };
        t.send("shutdown").await.is_ok()
    }

    async fn console_input(
        &self,
        _ctx: &GameContext,
        _inst: &Instance,
        dir: &Path,
        line: &str,
    ) -> Option<Result<Option<String>>> {
        Some(telnet(dir, line, |_| false).await.map(Some))
    }

    fn properties_file(&self) -> Option<&'static str> {
        Some(CONFIG)
    }

    fn ports_used(&self, port: u16) -> Vec<(u16, Protocol)> {
        vec![
            (port, Protocol::Udp),
            (port, Protocol::Tcp),
            (port.saturating_add(1), Protocol::Udp),
            (port.saturating_add(2), Protocol::Udp),
            (telnet_port(port), Protocol::Tcp),
        ]
    }

    fn port_setting_key(&self) -> &'static str {
        "ServerPort"
    }

    fn apply_port(&self, dir: &Path, port: u16) -> Result<()> {
        let path = dir.join(CONFIG);
        let mut cfg = ServerConfig::load(&path)?;
        cfg.set("ServerPort", &port.to_string());
        cfg.set("TelnetPort", &telnet_port(port).to_string());
        cfg.save(&path)
    }

    fn read_settings(&self, dir: &Path) -> Result<Vec<(String, String)>> {
        Ok(ServerConfig::load(&dir.join(CONFIG))?
            .entries()
            .into_iter()
            .filter(|(k, _)| !MANAGED.contains(&k.as_str()))
            .collect())
    }

    fn write_settings(&self, dir: &Path, changes: &[(String, String)]) -> Result<()> {
        let path = dir.join(CONFIG);
        let mut cfg = ServerConfig::load(&path)?;
        for (k, v) in changes {
            if MANAGED.contains(&k.as_str()) {
                return Err(Error::InvalidInput(format!(
                    "Hearth manages {k}; it can't be changed here."
                )));
            }
            cfg.set(k, v);
        }
        cfg.save(&path)
    }

    fn players(&self) -> Option<&dyn PlayerAdmin> {
        Some(&SevenDaysPlayers)
    }
}

// ---- players -------------------------------------------------------------------------------------

struct SevenDaysPlayers;

/// Admins and bans from `UserData/Saves/serveradmin.xml` (written by the server).
/// (platform id, name)
type Admin = (String, String);
/// (platform id, name, reason)
type Ban = (String, String, Option<String>);

fn read_admin_file(dir: &Path) -> (Vec<Admin>, Vec<Ban>) {
    let path = user_data(dir).join("Saves").join("serveradmin.xml");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut admins = vec![];
    let mut bans = vec![];
    for tag in text.split('<').map(|t| t.split('>').next().unwrap_or("")) {
        let id = || {
            let platform = attr(tag, "platform")?;
            let user = attr(tag, "userid")?;
            Some(format!("{platform}_{user}"))
        };
        let name = attr(tag, "name").map(xml_unescape).unwrap_or_default();
        if tag.starts_with("user ") && attr(tag, "permission_level") == Some("0") {
            if let Some(id) = id() {
                admins.push((id, name));
            }
        } else if tag.starts_with("blacklisted ") {
            if let Some(id) = id() {
                let reason = attr(tag, "reason").map(xml_unescape).filter(|r| !r.is_empty());
                bans.push((id, name, reason));
            }
        }
    }
    (admins, bans)
}

fn platform_id(name: &str, seen: &[SeenPlayer]) -> Option<String> {
    seen.iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .and_then(|s| s.id.clone())
}

/// A name or id as one console argument.
fn arg(s: &str) -> String {
    let s = s.replace('"', "'");
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s
    }
}

pub(crate) fn live_commands(name: &str, id: Option<&str>, action: &PlayerAction) -> Result<Vec<String>> {
    let who = arg(id.unwrap_or(name));
    let reason = |r: &Option<String>, d: &str| arg(&crate::players::clean_reason(r).unwrap_or_else(|| d.to_owned()));
    Ok(match action {
        PlayerAction::Kick { reason: r } => vec![format!("kick {who} {}", reason(r, "Kicked by the host."))],
        PlayerAction::Ban { reason: r } => vec![format!(
            "ban add {who} 10 years {} {}",
            reason(r, "Banned by the host."),
            arg(name)
        )],
        PlayerAction::Unban => {
            let id = id.ok_or_else(|| need_id(name))?;
            vec![format!("ban remove {}", arg(id))]
        }
        PlayerAction::SetRole { role: Role::Operator } => vec![format!("admin add {who} 0 {}", arg(name))],
        PlayerAction::SetRole { role: Role::Player } => vec![format!("admin remove {who}")],
        _ => {
            return Err(Error::InvalidInput(
                "7 Days to Die players are either players or admins.".into(),
            ))
        }
    })
}

fn need_id(name: &str) -> Error {
    Error::Conflict(format!(
        "Hearth hasn't seen {name}'s account yet. It's recorded while they're on the server."
    ))
}

#[async_trait]
impl PlayerAdmin for SevenDaysPlayers {
    fn caps(&self) -> PlayerCaps {
        PlayerCaps {
            roles: vec![Role::Player, Role::Operator],
            can_ban: true,
            ban_reason: true,
            game_modes: vec![],
            has_whitelist: false,
            whitelist_label: "",
            role_needs_id: false,
        }
    }

    fn list(&self, dir: &Path, online: &[String], seen: &[SeenPlayer]) -> Result<Vec<PlayerEntry>> {
        let (admins, bans) = read_admin_file(dir);
        let mut out: Vec<PlayerEntry> = vec![];
        let mut add = |name: String, id: Option<String>| {
            if name.is_empty() || out.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                return;
            }
            let ban = id.as_ref().and_then(|i| bans.iter().find(|(b, _, _)| b == i));
            out.push(PlayerEntry {
                online: online.iter().any(|o| o.eq_ignore_ascii_case(&name)),
                role: if id.as_ref().is_some_and(|i| admins.iter().any(|(a, _)| a == i)) {
                    Role::Operator
                } else {
                    Role::Player
                },
                whitelisted: false,
                banned: ban.is_some(),
                ban_reason: ban.and_then(|(_, _, r)| r.clone()),
                id,
                name,
            });
        };
        for n in online {
            add(n.clone(), platform_id(n, seen));
        }
        for s in seen {
            add(s.name.clone(), s.id.clone());
        }
        for (id, name) in &admins {
            add(
                if name.is_empty() { id.clone() } else { name.clone() },
                Some(id.clone()),
            );
        }
        for (id, name, _) in &bans {
            add(
                if name.is_empty() { id.clone() } else { name.clone() },
                Some(id.clone()),
            );
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
            "7 Days to Die is managed through its admin console.".into(),
        ))
    }

    async fn apply_live(
        &self,
        dir: &Path,
        name: &str,
        seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Option<Result<()>> {
        let id = platform_id(name, seen);
        Some(
            async {
                for c in live_commands(name, id.as_deref(), action)? {
                    telnet(dir, &c, |_| false).await?;
                }
                Ok(())
            }
            .await,
        )
    }

    async fn apply_offline(
        &self,
        _ctx: &GameContext,
        _dir: &Path,
        _name: &str,
        _seen: &[SeenPlayer],
        _action: &PlayerAction,
    ) -> Result<()> {
        Err(Error::Conflict(
            "Start the server first: 7 Days to Die changes admins and bans through its console.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn inst() -> Instance {
        Instance {
            id: "7d-0001".into(),
            name: "Horde Night".into(),
            game: "sevendays".into(),
            flavor: "dedicated".into(),
            version: "latest".into(),
            installed_build: None,
            port: 26900,
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

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<ServerSettings>
	<!-- GENERAL SERVER SETTINGS -->
	<property name="ServerName"			value="My Game Host"/>	<!-- Whatever you want the name of the server to be. -->
	<property name="ServerPort"			value="26900"/>
	<!-- <property name="ServerLoginConfirmationText" value="" /> -->
	<property name="GameDifficulty"		value="1"/>
	<property name="TelnetEnabled"		value="true"/>
	<property name="TelnetPassword"		value=""/>
</ServerSettings>
"#;

    #[test]
    fn edits_serverconfig_and_keeps_the_rest() {
        let mut c = ServerConfig::parse(SAMPLE);
        assert_eq!(c.get("ServerName").as_deref(), Some("My Game Host"));
        assert_eq!(c.get("ServerLoginConfirmationText"), None, "commented out");
        c.set("ServerName", "Tom & \"Jerry\"");
        c.set("WorldGenSize", "8192");
        assert_eq!(c.get("ServerName").as_deref(), Some("Tom & \"Jerry\""));
        assert!(c.text.contains("value=\"Tom &amp; &quot;Jerry&quot;\""));
        assert!(c.text.contains("<!-- Whatever you want"), "comments kept");
        assert!(c
            .text
            .contains("<property name=\"WorldGenSize\" value=\"8192\" />\n</ServerSettings>"));
    }

    #[test]
    fn prepares_hearths_config() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("serverconfig.xml"), SAMPLE).unwrap();
        prepare(t.path(), &inst(), true).unwrap();
        let c = ServerConfig::load(&t.path().join(CONFIG)).unwrap();
        assert_eq!(c.get("ServerName").as_deref(), Some("Horde Night"));
        assert_eq!(c.get("GameDifficulty").as_deref(), Some("1"), "game defaults kept");
        assert_eq!(c.get("TelnetPort").as_deref(), Some("27900"));
        assert_eq!(c.get("TerminalWindowEnabled").as_deref(), Some("false"));
        assert!(c.get("UserDataFolder").unwrap().ends_with("UserData"));
        let pw = c.get("TelnetPassword").unwrap();
        assert_eq!(pw.len(), 24);

        // Reinstall keeps the password and the host's settings.
        SevenDays
            .write_settings(t.path(), &[("ServerName".into(), "Renamed".into())])
            .unwrap();
        prepare(t.path(), &inst(), false).unwrap();
        let c = ServerConfig::load(&t.path().join(CONFIG)).unwrap();
        assert_eq!(c.get("TelnetPassword").unwrap(), pw);
        assert_eq!(c.get("ServerName").as_deref(), Some("Renamed"));

        let s = SevenDays.read_settings(t.path()).unwrap();
        assert!(!s.iter().any(|(k, _)| MANAGED.contains(&k.as_str())), "{s:?}");
        assert!(SevenDays
            .write_settings(t.path(), &[("TelnetPassword".into(), "x".into())])
            .is_err());
        SevenDays.apply_port(t.path(), 26910).unwrap();
        let c = ServerConfig::load(&t.path().join(CONFIG)).unwrap();
        assert_eq!(c.get("TelnetPort").as_deref(), Some("27910"));
    }

    #[test]
    fn parses_player_list_and_log() {
        let lp = "0. id=171, Bacon Dude, pos=(-123.5, 61.1, 345.2), rot=(-11.3, 22.5, 0.0), remote=True, health=100, deaths=0, zombies=3, players=0, score=3, level=1, pltfmid=Steam_76561198000000001, crossid=EOS_0002, ip=1.2.3.4, ping=20\n1. id=172, Old, pos=(1.0, 2.0, 3.0), rot=(0, 0, 0), remote=True, health=100, deaths=0, zombies=0, players=0, score=0, level=1, steamid=76561198000000002, ip=5.6.7.8, ping=40\nTotal of 2 in the game\n";
        assert_eq!(
            parse_players(lp),
            vec![
                RemotePlayer {
                    name: "Bacon Dude".into(),
                    id: Some("Steam_76561198000000001".into())
                },
                RemotePlayer {
                    name: "Old".into(),
                    id: Some("Steam_76561198000000002".into())
                },
            ]
        );
        assert!(parse_players("Total of 0 in the game").is_empty());

        use LineEvent::*;
        assert_eq!(
            parse_7d_line("2026-11-14T12:00:00 51.234 INF StartGame done"),
            Some(Ready)
        );
        assert_eq!(
            parse_7d_line("2026-11-14T12:01:00 90.1 INF PlayerSpawnedInWorld (reason: JoinMultiplayer, position: 1, 2, 3): EntityID=171, PltfmId='Steam_765', CrossId='EOS_1', OwnerID='Steam_765', PlayerName='Bacon Dude', ClientNumber='1'"),
            Some(PlayerJoined("Bacon Dude".into()))
        );
        assert_eq!(
            parse_7d_line("2026-11-14T12:05:00 300.0 INF Player disconnected: EntityID=171, PltfmId='Steam_765', CrossId='EOS_1', OwnerID='Steam_765', PlayerName='Bacon Dude', ClientNumber='1'"),
            Some(PlayerLeft("Bacon Dude".into()))
        );
        assert_eq!(
            parse_7d_line("2026-11-14T12:05:00 300.0 INF PlayerSpawnedInWorld (reason: Teleport, ...): PlayerName='X'"),
            None
        );
    }

    #[test]
    fn admin_commands() {
        let id = Some("Steam_765");
        let c = |a: PlayerAction| live_commands("Bacon Dude", id, &a).unwrap();
        assert_eq!(
            c(PlayerAction::Kick {
                reason: Some("Go to bed".into())
            }),
            vec!["kick Steam_765 \"Go to bed\""]
        );
        assert_eq!(
            c(PlayerAction::Ban { reason: None }),
            vec!["ban add Steam_765 10 years \"Banned by the host.\" \"Bacon Dude\""]
        );
        assert_eq!(c(PlayerAction::Unban), vec!["ban remove Steam_765"]);
        assert_eq!(
            c(PlayerAction::SetRole { role: Role::Operator }),
            vec!["admin add Steam_765 0 \"Bacon Dude\""]
        );
        assert!(live_commands("X", None, &PlayerAction::Unban).is_err());
        assert_eq!(
            live_commands("NoId", None, &PlayerAction::Kick { reason: None }).unwrap(),
            vec!["kick NoId \"Kicked by the host.\""]
        );
    }

    #[test]
    fn reads_admins_and_bans() {
        let t = tempfile::tempdir().unwrap();
        let saves = user_data(t.path()).join("Saves");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(
            saves.join("serveradmin.xml"),
            r#"<adminTools><users><user platform="Steam" userid="111" name="Boss" permission_level="0" /><user platform="Steam" userid="333" name="Mod" permission_level="1" /></users><blacklist><blacklisted platform="Steam" userid="222" name="Griefer" unbandate="2036-01-01" reason="Burned the base" /></blacklist></adminTools>"#,
        )
        .unwrap();
        let list = SevenDaysPlayers.list(t.path(), &[], &[]).unwrap();
        let get = |n: &str| list.iter().find(|p| p.name == n).unwrap().clone();
        assert_eq!(get("Boss").role, Role::Operator);
        assert!(get("Griefer").banned);
        assert_eq!(get("Griefer").ban_reason.as_deref(), Some("Burned the base"));
        assert!(!list.iter().any(|p| p.name == "Mod"), "only full admins count");
    }

    /// Minimal 7DTD-style Telnet: password prompt, log lines streamed in between, `lp` reply.
    #[tokio::test]
    async fn telnet_logs_in_and_filters_log_lines() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"*** Connected with 7DTD server.\r\n\r\nPlease enter password:\r\n")
                .await
                .unwrap();
            let mut buf = [0u8; 256];
            let n = s.read(&mut buf).await.unwrap();
            let ok = &buf[..n] == b"secret\r\n";
            s.write_all(if ok {
                b"Logon successful.\r\n\r\n" as &[u8]
            } else {
                b"Password incorrect, please enter password:\r\n"
            })
            .await
            .unwrap();
            if !ok {
                return;
            }
            let _ = s.read(&mut buf).await.unwrap();
            s.write_all(b"2026-11-14T12:00:00 51.2 INF Executing command 'lp' by Telnet from 127.0.0.1:5555\r\n0. id=171, Bacon, pos=(1, 2, 3), rot=(0, 0, 0), remote=True, pltfmid=Steam_1, ip=1.2.3.4, ping=5\r\nTotal of 1 in the game\r\n").await.unwrap();
        });
        let mut t = Telnet::connect(port, "secret").await.unwrap();
        let out = t.exec("lp", |x| x.contains("in the game")).await.unwrap();
        assert!(!out.contains("INF Executing"), "{out}");
        assert_eq!(parse_players(&out).len(), 1);
    }
}
