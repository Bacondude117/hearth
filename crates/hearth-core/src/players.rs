//! Player management shared by all games: the types the UI sees, the actions it can take,
//! and a small per-server record of everyone who has joined.
//!
//! Game-specific behavior (which files hold ops/whitelists, which console commands to send)
//! lives in each provider's [`PlayerAdmin`] implementation.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::games::GameContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Java: a normal player. Bedrock uses Visitor/Member instead.
    Player,
    Visitor,
    Member,
    /// Full permissions.
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerEntry {
    pub name: String,
    /// Java UUID or Bedrock XUID, when known.
    pub id: Option<String>,
    pub online: bool,
    pub role: Role,
    pub whitelisted: bool,
    pub banned: bool,
    pub ban_reason: Option<String>,
}

/// What a game supports, so the UI only shows controls that work.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerCaps {
    pub roles: Vec<Role>,
    pub can_ban: bool,
    pub game_modes: Vec<&'static str>,
    /// Whether the game has a whitelist at all.
    pub has_whitelist: bool,
    /// "Whitelist" (Java) or "Allow list" (Bedrock).
    pub whitelist_label: &'static str,
    /// Roles can only be changed for players whose id is known (Bedrock needs an XUID).
    pub role_needs_id: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayersView {
    pub caps: PlayerCaps,
    /// Whether the whitelist is enforced (server setting).
    pub whitelist_enabled: bool,
    pub running: bool,
    pub players: Vec<PlayerEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PlayerAction {
    SetRole { role: Role },
    Whitelist { on: bool },
    Ban { reason: Option<String> },
    Unban,
    Kick { reason: Option<String> },
    GameMode { mode: String },
}

impl PlayerAction {
    /// Actions that only make sense against a live server.
    pub fn needs_running(&self) -> bool {
        matches!(self, Self::Kick { .. } | Self::GameMode { .. })
    }
}

/// Per-game player administration.
#[async_trait]
pub trait PlayerAdmin: Send + Sync {
    fn caps(&self) -> PlayerCaps;

    /// Builds the player list from the server's own files plus Hearth's record of who joined.
    fn list(&self, server_dir: &Path, online: &[String], seen: &[SeenPlayer]) -> Result<Vec<PlayerEntry>>;

    fn whitelist_enabled(&self, server_dir: &Path) -> bool;

    /// Console commands that perform `action` on a running server.
    fn commands(&self, name: &str, action: &PlayerAction) -> Result<Vec<String>>;

    /// For games managed through an admin API instead of console commands: perform `action`
    /// on the running server. `None` means "use `commands`".
    async fn apply_live(
        &self,
        _server_dir: &Path,
        _name: &str,
        _seen: &[SeenPlayer],
        _action: &PlayerAction,
    ) -> Option<Result<()>> {
        None
    }

    /// Applies `action` by editing files while the server is stopped.
    async fn apply_offline(
        &self,
        ctx: &GameContext,
        server_dir: &Path,
        name: &str,
        seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Result<()>;

    /// Whether `commands` alone is enough while running, or files must also be written
    /// and the server told to reload them (Bedrock permissions).
    fn live_needs_file_write(&self, _action: &PlayerAction) -> bool {
        false
    }
}

// ---- record of players who joined -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenPlayer {
    pub name: String,
    #[serde(default)]
    pub id: Option<String>,
    pub last_seen: DateTime<Utc>,
}

/// `instances/<id>/players.json`: everyone Hearth has seen join, with their id when the log
/// reveals it. Bedrock needs this because its files are keyed by XUID, not name.
pub struct PlayerBook {
    path: PathBuf,
    lock: Mutex<()>,
}

impl PlayerBook {
    pub fn new(path: PathBuf) -> Self {
        Self { path, lock: Mutex::new(()) }
    }

    pub fn load(&self) -> Vec<SeenPlayer> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Adds or refreshes a player. A known id is never replaced by `None`.
    pub fn record(&self, name: &str, id: Option<&str>) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut all = self.load();
        match all.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name)) {
            Some(p) => {
                p.name = name.to_owned();
                if let Some(id) = id {
                    p.id = Some(id.to_owned());
                }
                p.last_seen = Utc::now();
            }
            None => all.push(SeenPlayer { name: name.to_owned(), id: id.map(str::to_owned), last_seen: Utc::now() }),
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).at(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&all).expect("serializes")).at(&tmp)?;
        std::fs::rename(&tmp, &self.path).at(&self.path)
    }
}

// ---- helpers shared by implementations --------------------------------------------------------

/// Reads a JSON array file, treating a missing or empty file as empty.
pub(crate) fn read_json_list<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(vec![]),
        Ok(s) => serde_json::from_str(&s).map_err(|e| Error::BadResponse {
            source_name: path.display().to_string(),
            detail: e.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(Error::Io { path: path.to_path_buf(), source: e }),
    }
}

pub(crate) fn write_json_list<T: Serialize>(path: &Path, items: &[T]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(items).expect("serializes")).at(&tmp)?;
    std::fs::rename(&tmp, path).at(path)
}

/// Single-line text safe to put in a console command (reasons).
pub(crate) fn clean_reason(r: &Option<String>) -> Option<String> {
    r.as_deref()
        .map(|s| s.replace(['\n', '\r', '"'], " ").trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_records_and_keeps_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let book = PlayerBook::new(tmp.path().join("players.json"));
        assert!(book.load().is_empty());
        book.record("Steve", Some("123")).unwrap();
        book.record("steve", None).unwrap();
        book.record("Alex", None).unwrap();
        let all = book.load();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "steve");
        assert_eq!(all[0].id.as_deref(), Some("123"), "id survives a later record without one");
    }

    #[test]
    fn actions_deserialize_from_ui_shape() {
        let a: PlayerAction = serde_json::from_str(r#"{"type":"setRole","role":"operator"}"#).unwrap();
        assert_eq!(a, PlayerAction::SetRole { role: Role::Operator });
        let a: PlayerAction = serde_json::from_str(r#"{"type":"ban","reason":null}"#).unwrap();
        assert_eq!(a, PlayerAction::Ban { reason: None });
        let a: PlayerAction = serde_json::from_str(r#"{"type":"gameMode","mode":"creative"}"#).unwrap();
        assert!(a.needs_running());
    }

    #[test]
    fn reasons_are_single_line() {
        assert_eq!(clean_reason(&Some("griefing\nop Bob".into())).as_deref(), Some("griefing op Bob"));
        assert_eq!(clean_reason(&Some("   ".into())), None);
    }
}
