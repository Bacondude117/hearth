//! Player administration for the Bedrock Dedicated Server.
//!
//! `permissions.json` maps XUIDs to visitor/member/operator; `allowlist.json` lists allowed
//! names (XUID optional). BDS has no ban list. XUIDs come from Hearth's record of joins
//! (`Player connected: Name, xuid: 2535...`).

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::GameContext;
use crate::error::{Error, Result};
use crate::players::{
    clean_reason, read_json_list, write_json_list, PlayerAction, PlayerAdmin, PlayerCaps, PlayerEntry, Role,
    SeenPlayer,
};
use crate::properties::Properties;

pub const GAME_MODES: &[&str] = &["survival", "creative", "adventure", "spectator"];
const PERMISSIONS: &str = "permissions.json";
const ALLOWLIST: &str = "allowlist.json";

pub struct BedrockPlayers;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Permission {
    permission: String,
    xuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Allowed {
    #[serde(default, rename = "ignoresPlayerLimit")]
    ignores_player_limit: bool,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    xuid: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

/// Gamertags: letters, numbers and single spaces, up to 16 characters (newer tags can carry
/// a numeric suffix, still within these characters). Keeps quoted commands injection-free.
pub fn valid_name(n: &str) -> bool {
    (1..=20).contains(&n.chars().count())
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
        && !n.starts_with(' ')
        && !n.ends_with(' ')
}

fn check_name(n: &str) -> Result<()> {
    if valid_name(n) {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!("\"{n}\" isn't a valid Xbox gamertag.")))
    }
}

fn role_str(r: Role) -> Option<&'static str> {
    match r {
        Role::Visitor => Some("visitor"),
        Role::Member => Some("member"),
        Role::Operator => Some("operator"),
        Role::Player => None,
    }
}

fn parse_role(s: &str) -> Role {
    match s {
        "visitor" => Role::Visitor,
        "operator" => Role::Operator,
        _ => Role::Member,
    }
}

fn default_role(dir: &Path) -> Role {
    let p = Properties::load(&dir.join("server.properties")).unwrap_or_default();
    parse_role(p.get("default-player-permission-level").unwrap_or("member"))
}

fn xuid_for(name: &str, seen: &[SeenPlayer], allow: &[Allowed]) -> Option<String> {
    seen.iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .and_then(|s| s.id.clone())
        .or_else(|| allow.iter().find(|a| a.name.eq_ignore_ascii_case(name)).and_then(|a| a.xuid.clone()))
}

fn write_role(dir: &Path, name: &str, seen: &[SeenPlayer], role: Role) -> Result<()> {
    let level = role_str(role).ok_or_else(|| Error::InvalidInput("Pick Visitor, Member or Operator.".into()))?;
    let allow: Vec<Allowed> = read_json_list(&dir.join(ALLOWLIST))?;
    let xuid = xuid_for(name, seen, &allow).ok_or_else(|| {
        Error::InvalidInput(format!("{name} has to join the server once before their role can be changed."))
    })?;
    let path = dir.join(PERMISSIONS);
    let mut perms: Vec<Permission> = read_json_list(&path)?;
    perms.retain(|p| p.xuid != xuid);
    perms.push(Permission { permission: level.into(), xuid });
    write_json_list(&path, &perms)
}

#[async_trait]
impl PlayerAdmin for BedrockPlayers {
    fn caps(&self) -> PlayerCaps {
        PlayerCaps {
            roles: vec![Role::Visitor, Role::Member, Role::Operator],
            can_ban: false,
            game_modes: GAME_MODES.to_vec(),
            has_whitelist: true,
            whitelist_label: "Allow list",
            role_needs_id: true,
        }
    }

    fn list(&self, dir: &Path, online: &[String], seen: &[SeenPlayer]) -> Result<Vec<PlayerEntry>> {
        let perms: Vec<Permission> = read_json_list(&dir.join(PERMISSIONS))?;
        let allow: Vec<Allowed> = read_json_list(&dir.join(ALLOWLIST))?;
        let default = default_role(dir);

        let mut names: Vec<String> = vec![];
        for n in online.iter().chain(seen.iter().map(|s| &s.name)).chain(allow.iter().map(|a| &a.name)) {
            if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                names.push(n.clone());
            }
        }
        let mut out: Vec<PlayerEntry> = names
            .into_iter()
            .map(|name| {
                let id = xuid_for(&name, seen, &allow);
                let role = id
                    .as_ref()
                    .and_then(|x| perms.iter().find(|p| &p.xuid == x))
                    .map(|p| parse_role(&p.permission))
                    .unwrap_or(default);
                PlayerEntry {
                    online: online.iter().any(|o| o.eq_ignore_ascii_case(&name)),
                    whitelisted: allow.iter().any(|a| a.name.eq_ignore_ascii_case(&name)),
                    banned: false,
                    ban_reason: None,
                    role,
                    id,
                    name,
                }
            })
            .collect();
        out.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    fn whitelist_enabled(&self, dir: &Path) -> bool {
        let p = Properties::load(&dir.join("server.properties")).unwrap_or_default();
        p.get("allow-list").or_else(|| p.get("white-list")) == Some("true")
    }

    fn commands(&self, name: &str, action: &PlayerAction) -> Result<Vec<String>> {
        check_name(name)?;
        let q = format!("\"{name}\"");
        Ok(match action {
            // The new role is written to permissions.json first (see live_needs_file_write).
            PlayerAction::SetRole { .. } => vec!["permission reload".into()],
            PlayerAction::Whitelist { on: true } => vec![format!("allowlist add {q}")],
            PlayerAction::Whitelist { on: false } => vec![format!("allowlist remove {q}")],
            PlayerAction::Ban { .. } | PlayerAction::Unban => {
                return Err(Error::InvalidInput("Bedrock servers don't support bans. Use the allow list instead.".into()))
            }
            PlayerAction::Kick { reason } => vec![match clean_reason(reason) {
                Some(r) => format!("kick {q} {r}"),
                None => format!("kick {q}"),
            }],
            PlayerAction::GameMode { mode } => {
                if !GAME_MODES.contains(&mode.as_str()) {
                    return Err(Error::InvalidInput(format!("Unknown game mode: {mode}")));
                }
                vec![format!("gamemode {mode} {q}")]
            }
        })
    }

    fn live_needs_file_write(&self, action: &PlayerAction) -> bool {
        matches!(action, PlayerAction::SetRole { .. })
    }

    async fn apply_offline(
        &self,
        _ctx: &GameContext,
        dir: &Path,
        name: &str,
        seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Result<()> {
        check_name(name)?;
        match action {
            PlayerAction::SetRole { role } => write_role(dir, name, seen, *role),
            PlayerAction::Whitelist { on } => {
                let path = dir.join(ALLOWLIST);
                let mut allow: Vec<Allowed> = read_json_list(&path)?;
                allow.retain(|a| !a.name.eq_ignore_ascii_case(name));
                if *on {
                    let xuid = seen.iter().find(|s| s.name.eq_ignore_ascii_case(name)).and_then(|s| s.id.clone());
                    allow.push(Allowed { ignores_player_limit: false, name: name.to_owned(), xuid, extra: BTreeMap::new() });
                }
                write_json_list(&path, &allow)
            }
            PlayerAction::Ban { .. } | PlayerAction::Unban => {
                Err(Error::InvalidInput("Bedrock servers don't support bans. Use the allow list instead.".into()))
            }
            PlayerAction::Kick { .. } | PlayerAction::GameMode { .. } => {
                Err(Error::Conflict("The player has to be online for that.".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &Path) -> GameContext {
        let paths = crate::paths::AppPaths::new(dir);
        let client = crate::http::client();
        GameContext::new(paths, client)
    }

    fn seen(name: &str, xuid: &str) -> SeenPlayer {
        SeenPlayer { name: name.into(), id: Some(xuid.into()), last_seen: chrono::Utc::now() }
    }

    #[test]
    fn lists_roles_from_permissions_by_xuid() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join(PERMISSIONS), r#"[{"permission":"operator","xuid":"111"}]"#).unwrap();
        std::fs::write(dir.join(ALLOWLIST), r#"[{"ignoresPlayerLimit":false,"name":"Kid Brother"}]"#).unwrap();
        std::fs::write(dir.join("server.properties"), "allow-list=true\ndefault-player-permission-level=member\n").unwrap();
        let s = vec![seen("Cool Guy 22", "111"), seen("Mom", "222")];
        let list = BedrockPlayers.list(dir, &["Mom".into()], &s).unwrap();
        let get = |n: &str| list.iter().find(|p| p.name == n).unwrap();
        assert_eq!(list[0].name, "Mom", "online first");
        assert_eq!(get("Cool Guy 22").role, Role::Operator);
        assert_eq!(get("Mom").role, Role::Member);
        assert!(get("Kid Brother").whitelisted);
        assert_eq!(get("Kid Brother").id, None);
        assert!(BedrockPlayers.whitelist_enabled(dir));
    }

    #[test]
    fn commands_quote_names_with_spaces() {
        let c = |a: PlayerAction| BedrockPlayers.commands("Cool Guy 22", &a).unwrap().join(";");
        assert_eq!(c(PlayerAction::Whitelist { on: true }), "allowlist add \"Cool Guy 22\"");
        assert_eq!(c(PlayerAction::Kick { reason: Some("bye".into()) }), "kick \"Cool Guy 22\" bye");
        assert_eq!(c(PlayerAction::GameMode { mode: "creative".into() }), "gamemode creative \"Cool Guy 22\"");
        assert_eq!(c(PlayerAction::SetRole { role: Role::Operator }), "permission reload");
        assert!(BedrockPlayers.commands("Cool Guy", &PlayerAction::Unban).is_err());
        assert!(BedrockPlayers.commands("Bob\" op \"Eve", &PlayerAction::Unban).is_err());
    }

    #[tokio::test]
    async fn offline_role_and_allowlist_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let s = vec![seen("Cool Guy 22", "111")];
        let ctx = ctx(dir);
        BedrockPlayers.apply_offline(&ctx, dir, "Cool Guy 22", &s, &PlayerAction::SetRole { role: Role::Operator }).await.unwrap();
        BedrockPlayers.apply_offline(&ctx, dir, "Cool Guy 22", &s, &PlayerAction::SetRole { role: Role::Visitor }).await.unwrap();
        let perms: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(PERMISSIONS)).unwrap()).unwrap();
        assert_eq!(perms, serde_json::json!([{"permission":"visitor","xuid":"111"}]));

        BedrockPlayers.apply_offline(&ctx, dir, "Cool Guy 22", &s, &PlayerAction::Whitelist { on: true }).await.unwrap();
        BedrockPlayers.apply_offline(&ctx, dir, "Stranger", &s, &PlayerAction::Whitelist { on: true }).await.unwrap();
        BedrockPlayers.apply_offline(&ctx, dir, "stranger", &s, &PlayerAction::Whitelist { on: false }).await.unwrap();
        let allow: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(ALLOWLIST)).unwrap()).unwrap();
        assert_eq!(allow, serde_json::json!([{"ignoresPlayerLimit":false,"name":"Cool Guy 22","xuid":"111"}]));

        let err = BedrockPlayers
            .apply_offline(&ctx, dir, "Never Joined", &s, &PlayerAction::SetRole { role: Role::Operator })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("join the server once"), "{err}");
    }
}
