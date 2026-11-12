//! The game catalog. Each supported game is a [`GameProvider`]; adding a game means adding
//! one provider and registering it in [`Registry::builtin`].

pub mod bedrock_players;
pub mod java_players;
pub mod mc_content;
pub mod mc_loaders;
pub mod mc_packs;
pub mod mc_voicechat;
pub mod minecraft_bedrock;
pub mod minecraft_java;
pub mod palworld;
pub mod palworld_api;
pub mod palworld_settings;
pub mod terraria;
pub mod tmodloader;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::content::ContentFolder;
use crate::error::{Error, Result};
pub use crate::games::tmodloader::ClientImport;
use crate::instance::Instance;
use crate::instance::JavaLaunch;
use crate::java::JavaRuntimes;
use crate::net::Protocol;
use crate::packs::PackInfo;
use crate::paths::AppPaths;
use crate::players::PlayerAdmin;

/// Shared services a provider may use.
pub struct GameContext {
    pub client: reqwest::Client,
    pub paths: AppPaths,
    pub java: Arc<JavaRuntimes>,
    pub dotnet: Arc<crate::dotnet::DotnetRuntimes>,
    pub steam: Arc<crate::steamcmd::SteamCmd>,
}

impl GameContext {
    pub fn new(paths: AppPaths, client: reqwest::Client) -> Self {
        Self {
            java: Arc::new(JavaRuntimes::new(paths.clone(), client.clone())),
            dotnet: Arc::new(crate::dotnet::DotnetRuntimes::new(paths.clone(), client.clone())),
            steam: Arc::new(crate::steamcmd::SteamCmd::new(paths.clone(), client.clone())),
            client,
            paths,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub flavors: Vec<FlavorInfo>,
    pub default_port: u16,
    pub protocol: Protocol,
    /// Whether the user picks a RAM amount (JVM games).
    pub configurable_memory: bool,
    /// Whether the user can cap the server's memory (games with no setting of their own;
    /// Hearth enforces it). Off by default.
    pub memory_limit: bool,
    /// Terms the user must accept before installing.
    pub eula: Option<Eula>,
    /// Name of the playit tunnel type for this game, if playit has one (otherwise a plain
    /// TCP/UDP tunnel is created).
    pub playit_tunnel_type: Option<&'static str>,
    /// Placeholder for the console input ("Type a command…" or "Broadcast a message…").
    pub command_hint: &'static str,
    /// Shown under the version picker when creating a server (hardware advice, etc.).
    pub setup_note: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlavorInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Eula {
    pub text: &'static str,
    pub links: Vec<(&'static str, &'static str)>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GameVersion {
    pub id: String,
    pub label: String,
    pub stable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct InstallOutcome {
    pub installed_build: Option<String>,
    pub java_major: Option<u32>,
    pub jvm_args: Option<Vec<String>>,
    pub launch: Option<JavaLaunch>,
}

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// How console commands are encoded on the server's stdin.
    pub console_encoding: ConsoleEncoding,
    /// Hard cap on the server's memory (games without their own setting; Windows only).
    pub memory_limit_mb: Option<u32>,
}

/// Text encoding a server expects on its console input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConsoleEncoding {
    #[default]
    Utf8,
    /// UTF-16 little-endian: Terraria on Windows sets `Console.InputEncoding = Encoding.Unicode`.
    Utf16Le,
}

impl ConsoleEncoding {
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Utf16Le => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineEvent {
    Ready,
    PlayerJoined(String),
    PlayerLeft(String),
    /// The server hit a fatal error (it may still exit with code 0 afterwards).
    Crashed,
}

/// Install progress callback: stage label + optional fraction.
pub type Progress<'a> = &'a (dyn Fn(&str, Option<f64>) + Send + Sync);

#[async_trait]
pub trait GameProvider: Send + Sync {
    fn info(&self) -> GameInfo;

    async fn versions(&self, ctx: &GameContext, flavor: &str) -> Result<Vec<GameVersion>>;

    /// Installs into `server_dir`. Also used for updates: `fresh == false` means the folder
    /// already holds a server whose worlds and settings must survive.
    async fn install(
        &self,
        ctx: &GameContext,
        inst: &Instance,
        server_dir: &Path,
        fresh: bool,
        progress: Progress<'_>,
    ) -> Result<InstallOutcome>;

    fn launch(&self, ctx: &GameContext, inst: &Instance, server_dir: &Path) -> Result<LaunchSpec>;

    fn parse_line(&self, line: &str) -> Option<LineEvent>;

    /// Console command that shuts the server down cleanly.
    fn stop_command(&self) -> &'static str {
        "stop"
    }

    /// Config file holding user-editable settings, relative to the server dir.
    fn properties_file(&self) -> Option<&'static str> {
        Some("server.properties")
    }

    /// Every port the server binds for a given main port, with its protocol (Bedrock also binds
    /// an IPv6 port; Palworld a TCP admin port).
    fn ports_used(&self, port: u16) -> Vec<(u16, Protocol)> {
        vec![(port, self.info().protocol)]
    }

    /// The settings key that holds the main port.
    fn port_setting_key(&self) -> &'static str {
        "server-port"
    }

    /// User-editable settings, in file order.
    fn read_settings(&self, server_dir: &Path) -> Result<Vec<(String, String)>> {
        match self.properties_file() {
            Some(f) => Ok(crate::properties::Properties::load(&server_dir.join(f))?.entries()),
            None => Ok(vec![]),
        }
    }

    /// Writes changed settings (the port key is handled separately via `apply_port`).
    fn write_settings(&self, server_dir: &Path, changes: &[(String, String)]) -> Result<()> {
        let Some(f) = self.properties_file() else { return Ok(()) };
        let path = server_dir.join(f);
        let mut props = crate::properties::Properties::load(&path)?;
        for (k, v) in changes {
            props.set(k, v.clone());
        }
        props.save(&path)
    }

    /// For servers without a console: poll their admin API. `Some(Ok(players))` also means the
    /// server is ready. `None` means this game reports through its log instead.
    async fn poll(
        &self,
        _ctx: &GameContext,
        _inst: &Instance,
        _server_dir: &Path,
    ) -> Option<Result<Vec<RemotePlayer>>> {
        None
    }

    /// Asks the server to save and shut down through its admin API. Returns true if it accepted,
    /// in which case Hearth only waits for the process to exit.
    async fn remote_stop(&self, _ctx: &GameContext, _inst: &Instance, _server_dir: &Path) -> bool {
        false
    }

    /// Handles a line typed in the console box when the game has no stdin console.
    async fn console_input(
        &self,
        _ctx: &GameContext,
        _inst: &Instance,
        _server_dir: &Path,
        _line: &str,
    ) -> Option<Result<()>> {
        None
    }

    /// Writes the port into the game's config.
    fn apply_port(&self, server_dir: &Path, port: u16) -> Result<()>;

    /// Player management (roles, whitelist, bans), if this game supports it.
    fn players(&self) -> Option<&dyn PlayerAdmin> {
        None
    }

    /// A player's name and account id when a log line reveals them (recorded so files keyed
    /// by id can be edited later).
    fn parse_identity(&self, _line: &str) -> Option<(String, Option<String>)> {
        None
    }

    /// `Some` when `file` is a modpack this game can import.
    async fn inspect_pack(&self, _ctx: &GameContext, _file: &Path) -> Option<Result<PackInfo>> {
        None
    }

    /// The add-on folder (mods/plugins) for this server, if it has one.
    fn content_folder(&self, _inst: &Instance) -> Option<ContentFolder> {
        None
    }

    /// The game's own server-icon file (name in the server folder, pixel size), if players see
    /// one (Minecraft Java's `server-icon.png`).
    fn server_icon_file(&self) -> Option<(&'static str, u32)> {
        None
    }

    /// Display name and version of an add-on file.
    fn describe_content(&self, _file: &Path) -> Option<(String, Option<String>)> {
        None
    }

    /// Copies the add-ons enabled in the player's own game on this PC into the server
    /// (offered when the content folder has an `import_label`).
    fn import_client_content(&self, _inst: &Instance, _server_dir: &Path) -> Option<Result<ClientImport>> {
        None
    }
}

/// A player reported by a game's admin API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePlayer {
    pub name: String,
    pub id: Option<String>,
}

pub struct Registry {
    providers: Vec<Arc<dyn GameProvider>>,
}

impl Registry {
    pub fn builtin() -> Self {
        Self {
            providers: vec![
                Arc::new(minecraft_java::MinecraftJava),
                Arc::new(minecraft_bedrock::MinecraftBedrock),
                Arc::new(palworld::Palworld),
                Arc::new(terraria::Terraria),
            ],
        }
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn GameProvider>> {
        self.providers
            .iter()
            .find(|p| p.info().id == id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput(format!("Unknown game: {id}")))
    }

    pub fn infos(&self) -> Vec<GameInfo> {
        self.providers.iter().map(|p| p.info()).collect()
    }
}

/// Orders version strings like `1.21.11`, `26.3`, `26.3-rc-3` naturally.
/// A pre-release sorts below its release (`26.3-rc-3 < 26.3`).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (a_main, a_pre) = split_pre(a);
    let (b_main, b_pre) = split_pre(b);
    let nums = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (an, bn) = (nums(a_main), nums(b_main));
    let len = an.len().max(bn.len());
    for i in 0..len {
        let (x, y) = (an.get(i).copied().unwrap_or(0), bn.get(i).copied().unwrap_or(0));
        match x.cmp(&y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => natural_cmp(x, y),
    }
}

fn split_pre(v: &str) -> (&str, Option<&str>) {
    match v.find('-') {
        Some(i) => (&v[..i], Some(&v[i + 1..])),
        None => (v, None),
    }
}

/// Compares strings treating digit runs as numbers (`rc10 > rc9`).
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let digits = |s: &str| -> u64 {
        s.chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    };
    let letters = |s: &str| -> String { s.chars().filter(|c| !c.is_ascii_digit()).collect() };
    letters(a).cmp(&letters(b)).then(digits(a).cmp(&digits(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        let mut v = vec![
            "1.21.11",
            "26.3-rc-3",
            "1.8.9",
            "26.3",
            "1.21.9",
            "26.1.2",
            "1.21.11-pre5",
            "26.3-rc-10",
        ];
        v.sort_by(|a, b| compare_versions(b, a));
        assert_eq!(
            v,
            vec![
                "26.3",
                "26.3-rc-10",
                "26.3-rc-3",
                "26.1.2",
                "1.21.11",
                "1.21.11-pre5",
                "1.21.9",
                "1.8.9"
            ]
        );
    }

    #[test]
    fn registry_has_builtin_games() {
        let r = Registry::builtin();
        assert!(r.get("minecraft-java").is_ok());
        assert!(r.get("minecraft-bedrock").is_ok());
        assert!(r.get("nope").is_err());
    }
}
