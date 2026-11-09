//! The game catalog. Each supported game is a [`GameProvider`]; adding a game means adding
//! one provider and registering it in [`Registry::builtin`].

pub mod bedrock_players;
pub mod java_players;
pub mod minecraft_bedrock;
pub mod minecraft_java;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::instance::Instance;
use crate::java::JavaRuntimes;
use crate::net::Protocol;
use crate::paths::AppPaths;
use crate::players::PlayerAdmin;

/// Shared services a provider may use.
pub struct GameContext {
    pub client: reqwest::Client,
    pub paths: AppPaths,
    pub java: Arc<JavaRuntimes>,
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
    /// Terms the user must accept before installing.
    pub eula: Option<Eula>,
    /// Name of the playit tunnel type for this game, if playit has one.
    pub playit_tunnel_type: Option<&'static str>,
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
}

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineEvent {
    Ready,
    PlayerJoined(String),
    PlayerLeft(String),
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

    /// Every port the server binds for a given main port (Bedrock also binds an IPv6 port).
    fn ports_used(&self, port: u16) -> Vec<u16> {
        vec![port]
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
    let digits = |s: &str| -> u64 { s.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0) };
    let letters = |s: &str| -> String { s.chars().filter(|c| !c.is_ascii_digit()).collect() };
    letters(a).cmp(&letters(b)).then(digits(a).cmp(&digits(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        let mut v = vec!["1.21.11", "26.3-rc-3", "1.8.9", "26.3", "1.21.9", "26.1.2", "1.21.11-pre5", "26.3-rc-10"];
        v.sort_by(|a, b| compare_versions(b, a));
        assert_eq!(v, vec!["26.3", "26.3-rc-10", "26.3-rc-3", "26.1.2", "1.21.11", "1.21.11-pre5", "1.21.9", "1.8.9"]);
    }

    #[test]
    fn registry_has_builtin_games() {
        let r = Registry::builtin();
        assert!(r.get("minecraft-java").is_ok());
        assert!(r.get("minecraft-bedrock").is_ok());
        assert!(r.get("nope").is_err());
    }
}
