//! Palworld dedicated server (Steam app 2394010), installed with SteamCMD.
//!
//! The server has no stdin console, so Hearth controls it through its REST admin API
//! (enabled by Hearth on 127.0.0.1, port = game port + 1, random admin password).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::palworld_api::PalApi;
use super::palworld_settings::PalSettings;
use super::{
    FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome, LaunchSpec, LineEvent, Progress,
    RemotePlayer,
};
use crate::error::{Error, IoContext, Result};
use crate::instance::Instance;
use crate::net::Protocol;
use crate::players::{PlayerAction, PlayerAdmin, PlayerCaps, PlayerEntry, Role, SeenPlayer};
use crate::steamcmd;

pub const APP_ID: u32 = 2394010;

pub struct Palworld;

fn platform_dir() -> &'static str {
    if cfg!(windows) {
        "WindowsServer"
    } else {
        "LinuxServer"
    }
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join("Pal/Saved/Config")
        .join(platform_dir())
        .join("PalWorldSettings.ini")
}

fn banlist_path(dir: &Path) -> PathBuf {
    dir.join("Pal/Saved/SaveGames/banlist.txt")
}

fn executable(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("Pal/Binaries/Win64/PalServer-Win64-Shipping-Cmd.exe")
    } else {
        dir.join("Pal/Binaries/Linux/PalServer-Linux-Shipping")
    }
}

/// The REST admin port Hearth assigns for a game port.
pub fn rest_port(game_port: u16) -> u16 {
    game_port.saturating_add(1)
}

/// Builds an API client from the server's own settings (port + admin password).
fn api(dir: &Path) -> Result<PalApi> {
    let s = PalSettings::load(&settings_path(dir))?;
    let port: u16 = s.get("RESTAPIPort").and_then(|p| p.parse().ok()).ok_or_else(|| {
        Error::InvalidInput("RESTAPIPort is missing from PalWorldSettings.ini. Try reinstalling.".into())
    })?;
    let password = s.get("AdminPassword").filter(|p| !p.is_empty()).ok_or_else(|| {
        Error::InvalidInput(
            "AdminPassword is empty in PalWorldSettings.ini; Hearth needs it to control the server.".into(),
        )
    })?;
    Ok(PalApi::new(port, &password))
}

/// Makes sure the settings file exists and lets Hearth manage the server.
pub(crate) fn prepare_settings(dir: &Path, inst: &Instance, fresh: bool) -> Result<()> {
    let path = settings_path(dir);
    let mut s = PalSettings::load(&path)?;
    if s.entries().is_empty() {
        // First run: start from the defaults the server ships with.
        let default = dir.join("DefaultPalWorldSettings.ini");
        if default.exists() {
            s = PalSettings::parse(&std::fs::read_to_string(&default).at(&default)?)?;
        }
    }
    s.set("RESTAPIEnabled", "True");
    if s.get("AdminPassword").is_none_or(|p| p.is_empty()) {
        s.set("AdminPassword", &hex::encode(rand::random::<[u8; 12]>()));
    }
    if fresh || s.get("RESTAPIPort").is_none() {
        s.set("RESTAPIPort", &rest_port(inst.port).to_string());
    }
    if fresh {
        s.set("ServerName", &inst.name);
        s.set("PublicPort", &inst.port.to_string());
    }
    s.save(&path)
}

#[async_trait]
impl GameProvider for Palworld {
    fn info(&self) -> GameInfo {
        GameInfo {
            id: "palworld",
            name: "Palworld",
            description: "Creature-collecting survival. Friends join from \"Join Multiplayer Game\" with your address.",
            flavors: vec![FlavorInfo {
                id: "steam",
                name: "Dedicated server",
                description: "Pocketpair's official dedicated server, installed through Steam.",
            }],
            default_port: 8211,
            protocol: Protocol::Udp,
            configurable_memory: false,
            memory_limit: true,
            eula: None,
            // Accepted by playit's API although its published client doesn't list it.
            playit_tunnel_type: Some("palworld"),
            command_hint: "Broadcast a message to everyone in the game",
            setup_note: Some(
                "Palworld servers need a lot of memory: 8 GB at minimum, 16 GB or more recommended. The first install downloads about 4 GB through Steam.",
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
        prepare_settings(dir, inst, fresh)?;
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
                "The Palworld server files are missing. Try reinstalling (Settings → Update).".into(),
            ));
        }
        Ok(LaunchSpec {
            program: exe,
            args: vec![
                "Pal".into(),
                format!("-port={}", inst.port),
                "-useperfthreads".into(),
                "-NoAsyncLoadingThread".into(),
                "-UseMultithreadForDS".into(),
            ],
            cwd: dir.to_path_buf(),
            env: vec![],
            console_encoding: Default::default(),
            memory_limit_mb: None,
        })
    }

    fn parse_line(&self, _line: &str) -> Option<LineEvent> {
        // Readiness and players come from the REST API (see `poll`).
        None
    }

    fn stop_command(&self) -> &'static str {
        ""
    }

    fn properties_file(&self) -> Option<&'static str> {
        None
    }

    fn ports_used(&self, port: u16) -> Vec<(u16, Protocol)> {
        vec![(port, Protocol::Udp), (rest_port(port), Protocol::Tcp)]
    }

    fn port_setting_key(&self) -> &'static str {
        "PublicPort"
    }

    fn apply_port(&self, dir: &Path, port: u16) -> Result<()> {
        let path = settings_path(dir);
        let mut s = PalSettings::load(&path)?;
        s.set("PublicPort", &port.to_string());
        s.set("RESTAPIPort", &rest_port(port).to_string());
        s.save(&path)
    }

    fn read_settings(&self, dir: &Path) -> Result<Vec<(String, String)>> {
        Ok(PalSettings::load(&settings_path(dir))?.entries())
    }

    fn write_settings(&self, dir: &Path, changes: &[(String, String)]) -> Result<()> {
        let path = settings_path(dir);
        let mut s = PalSettings::load(&path)?;
        for (k, v) in changes {
            if k == "RESTAPIEnabled" && !v.eq_ignore_ascii_case("true") {
                return Err(Error::InvalidInput(
                    "Hearth needs the REST API to control Palworld; it can't be turned off.".into(),
                ));
            }
            if k == "AdminPassword" && v.trim().is_empty() {
                return Err(Error::InvalidInput(
                    "The admin password can't be empty; Hearth uses it to control the server.".into(),
                ));
            }
            s.set(k, v);
        }
        s.save(&path)
    }

    async fn poll(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path) -> Option<Result<Vec<RemotePlayer>>> {
        let api = match api(dir) {
            Ok(a) => a,
            Err(e) => return Some(Err(e)),
        };
        Some(api.players().await.map(|ps| {
            ps.into_iter()
                .map(|p| RemotePlayer {
                    id: (!p.user_id.is_empty()).then_some(p.user_id),
                    name: p.name,
                })
                .collect()
        }))
    }

    async fn remote_stop(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path) -> bool {
        let Ok(api) = api(dir) else { return false };
        if let Err(e) = api.save().await {
            tracing::warn!("Palworld save before stop failed: {e}");
        }
        api.shutdown(1, "Server is shutting down").await.is_ok()
    }

    async fn console_input(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path, line: &str) -> Option<Result<()>> {
        Some(match api(dir) {
            Ok(api) => api.announce(line).await,
            Err(e) => Err(e),
        })
    }

    fn players(&self) -> Option<&dyn PlayerAdmin> {
        Some(&PalworldPlayers)
    }
}

// ---- players --------------------------------------------------------------------------------

pub struct PalworldPlayers;

fn read_banlist(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(banlist_path(dir))
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn write_banlist(dir: &Path, ids: &[String]) -> Result<()> {
    let path = banlist_path(dir);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).at(d)?;
    }
    let mut text = ids.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    std::fs::write(&path, text).at(&path)
}

fn user_id(name: &str, seen: &[SeenPlayer]) -> Result<String> {
    seen.iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .and_then(|s| s.id.clone())
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "Hearth doesn't know {name}'s Steam ID yet; they need to join the server once."
            ))
        })
}

#[async_trait]
impl PlayerAdmin for PalworldPlayers {
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
        let bans = read_banlist(dir);
        let mut names: Vec<String> = vec![];
        for n in online.iter().chain(seen.iter().map(|s| &s.name)) {
            if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                names.push(n.clone());
            }
        }
        let mut out: Vec<PlayerEntry> = names
            .into_iter()
            .map(|name| {
                let id = seen
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(&name))
                    .and_then(|s| s.id.clone());
                PlayerEntry {
                    online: online.iter().any(|o| o.eq_ignore_ascii_case(&name)),
                    role: Role::Player,
                    whitelisted: false,
                    banned: id.as_ref().is_some_and(|i| bans.contains(i)),
                    ban_reason: None,
                    id,
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

    fn whitelist_enabled(&self, _dir: &Path) -> bool {
        false
    }

    fn commands(&self, _name: &str, _action: &PlayerAction) -> Result<Vec<String>> {
        Err(Error::InvalidInput(
            "Palworld is controlled through its admin API, not console commands.".into(),
        ))
    }

    async fn apply_live(
        &self,
        dir: &Path,
        name: &str,
        seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Option<Result<()>> {
        Some(
            async {
                let api = api(dir)?;
                let id = user_id(name, seen)?;
                let reason =
                    |r: &Option<String>, d: &str| crate::players::clean_reason(r).unwrap_or_else(|| d.to_owned());
                match action {
                    PlayerAction::Kick { reason: r } => api.kick(&id, &reason(r, "You were kicked.")).await,
                    PlayerAction::Ban { reason: r } => api.ban(&id, &reason(r, "You are banned.")).await,
                    PlayerAction::Unban => api.unban(&id).await,
                    _ => Err(Error::InvalidInput(
                        "Palworld servers don't have roles, whitelists or game modes.".into(),
                    )),
                }
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
        match action {
            PlayerAction::Ban { .. } => {
                let id = user_id(name, seen)?;
                let mut bans = read_banlist(dir);
                if !bans.contains(&id) {
                    bans.push(id);
                }
                write_banlist(dir, &bans)
            }
            PlayerAction::Unban => {
                let id = user_id(name, seen)?;
                let bans: Vec<String> = read_banlist(dir).into_iter().filter(|b| *b != id).collect();
                write_banlist(dir, &bans)
            }
            PlayerAction::Kick { .. } => Err(Error::Conflict("The player has to be online for that.".into())),
            _ => Err(Error::InvalidInput(
                "Palworld servers don't have roles, whitelists or game modes.".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn inst(port: u16) -> Instance {
        Instance {
            id: "pal-0001".into(),
            name: "Pals, Friends".into(),
            game: "palworld".into(),
            flavor: "steam".into(),
            version: "latest".into(),
            installed_build: None,
            port,
            memory_mb: None,
            java_major: None,
            jvm_args: vec![],
            install_state: crate::instance::InstallState::Installed,
            created_at: Utc::now(),
            playit_tunnel_id: None,
            launch: None,
            pack: None,
            voice: None,
        }
    }

    const DEFAULT: &str = "[/Script/Pal.PalGameWorldSettings]\nOptionSettings=(Difficulty=None,ServerPlayerMaxNum=32,ServerName=\"Default Palworld Server\",AdminPassword=\"\",PublicPort=8211,RESTAPIEnabled=False,RESTAPIPort=8212)\n";

    #[test]
    fn prepares_settings_from_the_shipped_defaults() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("DefaultPalWorldSettings.ini"), DEFAULT).unwrap();
        prepare_settings(t.path(), &inst(8300), true).unwrap();
        let s = PalSettings::load(&settings_path(t.path())).unwrap();
        assert_eq!(s.raw("RESTAPIEnabled"), Some("True"));
        assert_eq!(s.raw("RESTAPIPort"), Some("8301"));
        assert_eq!(s.raw("PublicPort"), Some("8300"));
        assert_eq!(s.get("ServerName").as_deref(), Some("Pals, Friends"));
        let pw = s.get("AdminPassword").unwrap();
        assert_eq!(pw.len(), 24);

        // A reinstall keeps the password and the user's name.
        let mut edited = s.clone();
        edited.set("ServerName", "Renamed");
        edited.save(&settings_path(t.path())).unwrap();
        prepare_settings(t.path(), &inst(8300), false).unwrap();
        let s = PalSettings::load(&settings_path(t.path())).unwrap();
        assert_eq!(s.get("AdminPassword").unwrap(), pw);
        assert_eq!(s.get("ServerName").as_deref(), Some("Renamed"));
    }

    #[test]
    fn uses_playits_palworld_tunnel_type() {
        // Accepted by playit's live API (2026-11-09), though missing from its published client.
        assert_eq!(Palworld.info().playit_tunnel_type, Some("palworld"));
    }

    #[test]
    fn ports_and_launch() {
        assert_eq!(
            Palworld.ports_used(8211),
            vec![(8211, Protocol::Udp), (8212, Protocol::Tcp)]
        );
        let t = tempfile::tempdir().unwrap();
        assert!(Palworld
            .launch(
                &GameContext::new(crate::paths::AppPaths::new(t.path()), crate::http::client()),
                &inst(8211),
                t.path()
            )
            .is_err());
        std::fs::create_dir_all(executable(t.path()).parent().unwrap()).unwrap();
        std::fs::write(executable(t.path()), b"").unwrap();
        let spec = Palworld
            .launch(
                &GameContext::new(crate::paths::AppPaths::new(t.path()), crate::http::client()),
                &inst(8211),
                t.path(),
            )
            .unwrap();
        assert_eq!(spec.args[0], "Pal");
        assert!(spec.args.contains(&"-port=8211".to_string()));
    }

    #[test]
    fn settings_guard_the_admin_api() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("DefaultPalWorldSettings.ini"), DEFAULT).unwrap();
        prepare_settings(t.path(), &inst(8211), true).unwrap();
        assert!(Palworld
            .write_settings(t.path(), &[("RESTAPIEnabled".into(), "False".into())])
            .is_err());
        assert!(Palworld
            .write_settings(t.path(), &[("AdminPassword".into(), "".into())])
            .is_err());
        Palworld
            .write_settings(t.path(), &[("ServerPlayerMaxNum".into(), "8".into())])
            .unwrap();
        assert!(Palworld
            .read_settings(t.path())
            .unwrap()
            .contains(&("ServerPlayerMaxNum".into(), "8".into())));
        Palworld.apply_port(t.path(), 9000).unwrap();
        let s = PalSettings::load(&settings_path(t.path())).unwrap();
        assert_eq!(
            (s.raw("PublicPort"), s.raw("RESTAPIPort")),
            (Some("9000"), Some("9001"))
        );
    }

    #[tokio::test]
    async fn offline_bans_edit_the_banlist() {
        let t = tempfile::tempdir().unwrap();
        let seen = vec![SeenPlayer {
            name: "Bacon_Dude117".into(),
            id: Some("steam_765611".into()),
            last_seen: Utc::now(),
        }];
        let ctx = GameContext::new(crate::paths::AppPaths::new(t.path()), crate::http::client());
        let ban = PlayerAction::Ban { reason: None };
        PalworldPlayers
            .apply_offline(&ctx, t.path(), "bacon_dude117", &seen, &ban)
            .await
            .unwrap();
        PalworldPlayers
            .apply_offline(&ctx, t.path(), "Bacon_Dude117", &seen, &ban)
            .await
            .unwrap();
        assert_eq!(read_banlist(t.path()), vec!["steam_765611"]);
        let list = PalworldPlayers.list(t.path(), &[], &seen).unwrap();
        assert!(list[0].banned);
        PalworldPlayers
            .apply_offline(&ctx, t.path(), "Bacon_Dude117", &seen, &PlayerAction::Unban)
            .await
            .unwrap();
        assert!(read_banlist(t.path()).is_empty());
        let e = PalworldPlayers
            .apply_offline(&ctx, t.path(), "Stranger", &seen, &ban)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("join the server once"));
    }
}
