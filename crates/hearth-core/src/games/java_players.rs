//! Player administration for Minecraft: Java Edition (Vanilla and Paper share the formats).
//!
//! Files in the server folder: `ops.json`, `whitelist.json`, `banned-players.json`, and
//! `usercache.json` (name <-> UUID for everyone who joined).

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use super::GameContext;
use crate::error::{Error, Result};
use crate::http;
use crate::players::{
    clean_reason, read_json_list, write_json_list, PlayerAction, PlayerAdmin, PlayerCaps, PlayerEntry, Role, SeenPlayer,
};
use crate::properties::Properties;

pub const GAME_MODES: &[&str] = &["survival", "creative", "adventure", "spectator"];
const MOJANG_PROFILE_API: &str = "https://api.mojang.com/users/profiles/minecraft";

pub struct JavaPlayers;

/// Unknown fields are kept so we never drop data the server or plugins wrote.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    name: String,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

const OPS: &str = "ops.json";
const WHITELIST: &str = "whitelist.json";
const BANS: &str = "banned-players.json";
const USERCACHE: &str = "usercache.json";

/// Java account names: 3-16 of `[A-Za-z0-9_]` (shorter legacy names exist). Floodgate prefixes
/// Bedrock players with `.`. Validating also keeps console commands injection-free.
pub fn valid_name(n: &str) -> bool {
    let core = n.strip_prefix('.').unwrap_or(n);
    (1..=16).contains(&core.len()) && core.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn check_name(n: &str) -> Result<()> {
    if valid_name(n) {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "\"{n}\" isn't a valid Minecraft name (letters, numbers and _ only, up to 16 characters)."
        )))
    }
}

/// UUID the server uses for `name` when `online-mode=false` (Java's `UUID.nameUUIDFromBytes`).
pub fn offline_uuid(name: &str) -> String {
    let mut b: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    b[6] = (b[6] & 0x0f) | 0x30; // version 3
    b[8] = (b[8] & 0x3f) | 0x80; // IETF variant
    uuid::Uuid::from_bytes(b).hyphenated().to_string()
}

/// `0123...` (32 hex chars, as Mojang's API returns) -> hyphenated form.
fn hyphenate(id: &str) -> Option<String> {
    uuid::Uuid::parse_str(id).ok().map(|u| u.hyphenated().to_string())
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn props(dir: &Path) -> Properties {
    Properties::load(&dir.join("server.properties")).unwrap_or_default()
}

/// Finds the UUID the server already knows for `name`, from any of its files.
fn known_uuid(dir: &Path, name: &str) -> Option<(String, String)> {
    for f in [USERCACHE, OPS, WHITELIST, BANS] {
        let list: Vec<Entry> = read_json_list(&dir.join(f)).unwrap_or_default();
        if let Some(e) = list.into_iter().find(|e| same(&e.name, name) && !e.uuid.is_empty()) {
            return Some((e.uuid, e.name));
        }
    }
    None
}

async fn resolve(ctx: &GameContext, dir: &Path, name: &str) -> Result<(String, String)> {
    if let Some(found) = known_uuid(dir, name) {
        return Ok(found);
    }
    if props(dir).get("online-mode") == Some("false") {
        return Ok((offline_uuid(name), name.to_owned()));
    }
    if name.starts_with('.') {
        return Err(Error::InvalidInput(format!(
            "{name} has to join the server once before you can change this while it's stopped."
        )));
    }
    #[derive(Deserialize)]
    struct Profile {
        id: String,
        name: String,
    }
    let resp = ctx.client.get(format!("{MOJANG_PROFILE_API}/{name}")).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND || resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Err(Error::InvalidInput(format!(
            "There's no Minecraft account named \"{name}\"."
        )));
    }
    let p: Profile = resp.error_for_status()?.json().await?;
    let id = hyphenate(&p.id).ok_or_else(|| Error::BadResponse {
        source_name: http::host_of(MOJANG_PROFILE_API),
        detail: format!("invalid id {}", p.id),
    })?;
    Ok((id, p.name))
}

fn upsert(path: &Path, uuid: &str, name: &str, extra: BTreeMap<String, serde_json::Value>) -> Result<()> {
    let mut list: Vec<Entry> = read_json_list(path)?;
    list.retain(|e| !same(&e.name, name) && !(e.uuid == uuid && !uuid.is_empty()));
    list.push(Entry {
        uuid: uuid.to_owned(),
        name: name.to_owned(),
        extra,
    });
    write_json_list(path, &list)
}

fn remove(path: &Path, name: &str) -> Result<()> {
    let mut list: Vec<Entry> = read_json_list(path)?;
    let before = list.len();
    list.retain(|e| !same(&e.name, name));
    if list.len() != before {
        write_json_list(path, &list)?;
    }
    Ok(())
}

fn obj(pairs: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[async_trait]
impl PlayerAdmin for JavaPlayers {
    fn caps(&self) -> PlayerCaps {
        PlayerCaps {
            roles: vec![Role::Player, Role::Operator],
            can_ban: true,
            ban_reason: true,
            game_modes: GAME_MODES.to_vec(),
            has_whitelist: true,
            whitelist_label: "Whitelist",
            role_needs_id: false,
        }
    }

    fn list(&self, dir: &Path, online: &[String], seen: &[SeenPlayer]) -> Result<Vec<PlayerEntry>> {
        let ops: Vec<Entry> = read_json_list(&dir.join(OPS))?;
        let wl: Vec<Entry> = read_json_list(&dir.join(WHITELIST))?;
        let bans: Vec<Entry> = read_json_list(&dir.join(BANS))?;
        let cache: Vec<Entry> = read_json_list(&dir.join(USERCACHE)).unwrap_or_default();

        // name (as first seen) -> uuid
        let mut people: Vec<(String, Option<String>)> = vec![];
        let mut add = |name: &str, uuid: Option<&str>| {
            if name.is_empty() {
                return;
            }
            match people.iter_mut().find(|(n, _)| same(n, name)) {
                Some((_, id)) => {
                    if id.is_none() {
                        *id = uuid.filter(|u| !u.is_empty()).map(str::to_owned);
                    }
                }
                None => people.push((name.to_owned(), uuid.filter(|u| !u.is_empty()).map(str::to_owned))),
            }
        };
        for n in online {
            add(n, None);
        }
        for e in ops.iter().chain(&wl).chain(&bans).chain(&cache) {
            add(&e.name, Some(&e.uuid));
        }
        for s in seen {
            add(&s.name, s.id.as_deref());
        }

        let mut out: Vec<PlayerEntry> = people
            .into_iter()
            .map(|(name, id)| {
                let ban = bans.iter().find(|e| same(&e.name, &name));
                PlayerEntry {
                    online: online.iter().any(|o| same(o, &name)),
                    role: if ops.iter().any(|e| same(&e.name, &name)) {
                        Role::Operator
                    } else {
                        Role::Player
                    },
                    whitelisted: wl.iter().any(|e| same(&e.name, &name)),
                    banned: ban.is_some(),
                    ban_reason: ban
                        .and_then(|b| b.extra.get("reason"))
                        .and_then(|r| r.as_str())
                        .map(str::to_owned),
                    name,
                    id,
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
        props(dir).get("white-list") == Some("true")
    }

    fn commands(&self, name: &str, action: &PlayerAction) -> Result<Vec<String>> {
        check_name(name)?;
        let with_reason = |cmd: &str, r: &Option<String>| match clean_reason(r) {
            Some(r) => format!("{cmd} {name} {r}"),
            None => format!("{cmd} {name}"),
        };
        Ok(vec![match action {
            PlayerAction::SetRole { role: Role::Operator } => format!("op {name}"),
            PlayerAction::SetRole { role: Role::Player } => format!("deop {name}"),
            PlayerAction::SetRole { role } => {
                return Err(Error::InvalidInput(format!("{role:?} isn't a Java Edition role.")))
            }
            PlayerAction::Whitelist { on: true } => format!("whitelist add {name}"),
            PlayerAction::Whitelist { on: false } => format!("whitelist remove {name}"),
            PlayerAction::Ban { reason } => with_reason("ban", reason),
            PlayerAction::Unban => format!("pardon {name}"),
            PlayerAction::Kick { reason } => with_reason("kick", reason),
            PlayerAction::GameMode { mode } => {
                if !GAME_MODES.contains(&mode.as_str()) {
                    return Err(Error::InvalidInput(format!("Unknown game mode: {mode}")));
                }
                format!("gamemode {mode} {name}")
            }
        }])
    }

    async fn apply_offline(
        &self,
        ctx: &GameContext,
        dir: &Path,
        name: &str,
        _seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Result<()> {
        check_name(name)?;
        match action {
            PlayerAction::SetRole { role: Role::Operator } => {
                let (uuid, name) = resolve(ctx, dir, name).await?;
                // Operators always get full permissions (level 4).
                upsert(
                    &dir.join(OPS),
                    &uuid,
                    &name,
                    obj(&[("level", 4.into()), ("bypassesPlayerLimit", false.into())]),
                )
            }
            PlayerAction::SetRole { role: Role::Player } => remove(&dir.join(OPS), name),
            PlayerAction::SetRole { role } => Err(Error::InvalidInput(format!("{role:?} isn't a Java Edition role."))),
            PlayerAction::Whitelist { on: true } => {
                let (uuid, name) = resolve(ctx, dir, name).await?;
                upsert(&dir.join(WHITELIST), &uuid, &name, BTreeMap::new())
            }
            PlayerAction::Whitelist { on: false } => remove(&dir.join(WHITELIST), name),
            PlayerAction::Ban { reason } => {
                let (uuid, name) = resolve(ctx, dir, name).await?;
                let created = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S %z").to_string();
                let reason = clean_reason(reason).unwrap_or_else(|| "Banned by an operator.".into());
                upsert(
                    &dir.join(BANS),
                    &uuid,
                    &name,
                    obj(&[
                        ("created", created.into()),
                        ("source", "Hearth".into()),
                        ("expires", "forever".into()),
                        ("reason", reason.into()),
                    ]),
                )
            }
            PlayerAction::Unban => remove(&dir.join(BANS), name),
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

    /// Files as a real 26.3 server writes them.
    fn sample_server(dir: &Path) {
        std::fs::write(
            dir.join(USERCACHE),
            r#"[{"name":"Bacon_Dude117","uuid":"33f7864d-a02c-4e21-ae28-3ecf6f0fd767","expiresOn":"2026-12-09 22:52:49 -0600"},
               {"name":"Alex","uuid":"6ab43178-89fd-4905-97f6-0f67d9d76fd9","expiresOn":"2026-12-09 22:52:49 -0600"}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(OPS),
            r#"[{"uuid":"33f7864d-a02c-4e21-ae28-3ecf6f0fd767","name":"Bacon_Dude117","level":4,"bypassesPlayerLimit":false}]"#,
        )
        .unwrap();
        std::fs::write(dir.join(WHITELIST), "[]").unwrap();
        std::fs::write(
            dir.join(BANS),
            r#"[{"uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","name":"Griefer","created":"2026-11-09 03:00:00 +0000","source":"Server","expires":"forever","reason":"tnt"}]"#,
        )
        .unwrap();
        std::fs::write(dir.join("server.properties"), "online-mode=true\nwhite-list=false\n").unwrap();
    }

    #[test]
    fn lists_everyone_with_their_state() {
        let tmp = tempfile::tempdir().unwrap();
        sample_server(tmp.path());
        let seen = vec![SeenPlayer {
            name: "Newbie".into(),
            id: None,
            last_seen: chrono::Utc::now(),
        }];
        let list = JavaPlayers.list(tmp.path(), &["Alex".into()], &seen).unwrap();
        let names: Vec<_> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Alex", "Bacon_Dude117", "Griefer", "Newbie"],
            "online first, then by name"
        );
        let alex = &list[0];
        assert!(alex.online);
        assert_eq!(alex.role, Role::Player);
        let bacon = &list[1];
        assert_eq!(bacon.role, Role::Operator);
        assert_eq!(bacon.id.as_deref(), Some("33f7864d-a02c-4e21-ae28-3ecf6f0fd767"));
        let griefer = &list[2];
        assert!(griefer.banned);
        assert_eq!(griefer.ban_reason.as_deref(), Some("tnt"));
        assert!(!JavaPlayers.whitelist_enabled(tmp.path()));
    }

    #[test]
    fn live_commands() {
        let c = |a: PlayerAction| JavaPlayers.commands("Alex", &a).unwrap().join(";");
        assert_eq!(c(PlayerAction::SetRole { role: Role::Operator }), "op Alex");
        assert_eq!(c(PlayerAction::SetRole { role: Role::Player }), "deop Alex");
        assert_eq!(c(PlayerAction::Whitelist { on: true }), "whitelist add Alex");
        assert_eq!(c(PlayerAction::Whitelist { on: false }), "whitelist remove Alex");
        assert_eq!(
            c(PlayerAction::Ban {
                reason: Some("x-ray\nop me".into())
            }),
            "ban Alex x-ray op me"
        );
        assert_eq!(c(PlayerAction::Unban), "pardon Alex");
        assert_eq!(c(PlayerAction::Kick { reason: None }), "kick Alex");
        assert_eq!(
            c(PlayerAction::GameMode {
                mode: "creative".into()
            }),
            "gamemode creative Alex"
        );
        assert!(JavaPlayers
            .commands("Alex", &PlayerAction::GameMode { mode: "god".into() })
            .is_err());
    }

    #[test]
    fn rejects_names_that_could_inject_commands() {
        for bad in ["Alex; op Bob", "Al ex", "", "averyveryverylongname", "a\nop b"] {
            assert!(JavaPlayers.commands(bad, &PlayerAction::Unban).is_err(), "{bad:?}");
        }
        assert!(JavaPlayers.commands(".BedrockGuy", &PlayerAction::Unban).is_ok());
    }

    #[tokio::test]
    async fn offline_edits_use_known_uuids_and_keep_other_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        sample_server(dir);
        let ctx = ctx(dir);

        JavaPlayers
            .apply_offline(&ctx, dir, "alex", &[], &PlayerAction::SetRole { role: Role::Operator })
            .await
            .unwrap();
        JavaPlayers
            .apply_offline(&ctx, dir, "Alex", &[], &PlayerAction::Whitelist { on: true })
            .await
            .unwrap();
        JavaPlayers
            .apply_offline(
                &ctx,
                dir,
                "Bacon_Dude117",
                &[],
                &PlayerAction::SetRole { role: Role::Player },
            )
            .await
            .unwrap();
        JavaPlayers
            .apply_offline(&ctx, dir, "Griefer", &[], &PlayerAction::Unban)
            .await
            .unwrap();
        JavaPlayers
            .apply_offline(
                &ctx,
                dir,
                "Alex",
                &[],
                &PlayerAction::Ban {
                    reason: Some("testing".into()),
                },
            )
            .await
            .unwrap();

        let ops: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(OPS)).unwrap()).unwrap();
        assert_eq!(ops.as_array().unwrap().len(), 1);
        assert_eq!(
            ops[0]["name"], "Alex",
            "canonical name from usercache, not the typed casing"
        );
        assert_eq!(ops[0]["uuid"], "6ab43178-89fd-4905-97f6-0f67d9d76fd9");
        assert_eq!(ops[0]["level"], 4);

        let wl: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(WHITELIST)).unwrap()).unwrap();
        assert_eq!(wl[0]["uuid"], "6ab43178-89fd-4905-97f6-0f67d9d76fd9");

        let bans: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(BANS)).unwrap()).unwrap();
        let bans = bans.as_array().unwrap();
        assert_eq!(bans.len(), 1, "Griefer unbanned, Alex banned");
        assert_eq!(bans[0]["name"], "Alex");
        assert_eq!(bans[0]["reason"], "testing");
        assert_eq!(bans[0]["expires"], "forever");

        assert!(JavaPlayers
            .apply_offline(&ctx, dir, "Alex", &[], &PlayerAction::Kick { reason: None })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn offline_mode_servers_use_offline_uuids_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("server.properties"), "online-mode=false\n").unwrap();
        JavaPlayers
            .apply_offline(&ctx(dir), dir, "Notch", &[], &PlayerAction::Whitelist { on: true })
            .await
            .unwrap();
        let wl: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(WHITELIST)).unwrap()).unwrap();
        assert_eq!(wl[0]["uuid"], offline_uuid("Notch"));
    }

    #[test]
    fn offline_uuid_matches_java() {
        // Java: UUID.nameUUIDFromBytes("OfflinePlayer:Notch".getBytes(UTF_8))
        assert_eq!(offline_uuid("Notch"), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    }
}
