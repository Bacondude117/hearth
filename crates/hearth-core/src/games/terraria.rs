//! Terraria (vanilla) dedicated server, downloaded from terraria.org.
//!
//! Hearth starts it with `-config hearth-serverconfig.txt` so it runs unattended: the world is
//! auto-created on first start from the size/difficulty/name in Settings. The server reads
//! console commands (`say`, `kick`, `ban`, `save`, `exit`, ...) from stdin.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::{
    FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome, LaunchSpec, LineEvent, Progress,
};
use crate::archive::extract_zip_prefix;
use crate::error::{Error, IoContext, Result};
use crate::http::{self, Checksum};
use crate::instance::Instance;
use crate::net::Protocol;
use crate::players::{PlayerAction, PlayerAdmin, PlayerCaps, PlayerEntry, Role, SeenPlayer};
use crate::properties::Properties;

const VERSIONS_API: &str = "https://terraria.org/api/get/dedicated-servers-names";
const DOWNLOAD_BASE: &str = "https://terraria.org/api/download/pc-dedicated-server";
/// Hearth's own config file (the zip ships a `serverconfig.txt` that updates would overwrite).
pub const CONFIG: &str = "hearth-serverconfig.txt";
const BANLIST: &str = "banlist.txt";
const VERSION_FILE: &str = ".hearth-terraria-version";

pub struct Terraria;

/// `terraria-server-1458.zip` -> `1458`
pub fn build_from_zip_name(name: &str) -> Option<String> {
    let v = name.strip_prefix("terraria-server-")?.strip_suffix(".zip")?;
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit())).then(|| v.to_owned())
}

/// `1458` -> `1.4.5.8` (Terraria's builds are its version digits).
pub fn display_version(build: &str) -> String {
    build.chars().map(|c| c.to_string()).collect::<Vec<_>>().join(".")
}

fn platform_folder() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    }
}

fn executable(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("TerrariaServer.exe")
    } else {
        dir.join("TerrariaServer.bin.x86_64")
    }
}

/// World file names: keep it to safe characters so the path is predictable.
fn world_file_name(world_name: &str) -> String {
    let s: String = world_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().replace(' ', "_");
    if s.is_empty() {
        "World".into()
    } else {
        s.chars().take(40).collect()
    }
}

/// Points `world=` at `<dir>/worlds/<name>.wld` for the configured world name.
fn update_world_path(dir: &Path, cfg: &mut Properties) {
    let name = cfg.get("worldname").unwrap_or("World").to_owned();
    let path = dir.join("worlds").join(format!("{}.wld", world_file_name(&name)));
    cfg.set("world", path.display().to_string());
    cfg.set("worldpath", dir.join("worlds").display().to_string());
}

/// Writes Hearth's server config, keeping the user's settings on reinstall/update.
pub(crate) fn prepare_config(dir: &Path, inst: &Instance, fresh: bool) -> Result<()> {
    let path = dir.join(CONFIG);
    let mut cfg = Properties::load(&path)?;
    let first = fresh || cfg.entries().is_empty();
    if first {
        cfg.set("worldname", inst.name.clone());
        cfg.set("autocreate", "2"); // medium
        cfg.set("difficulty", "0"); // classic
        cfg.set("maxplayers", "8");
        cfg.set("port", inst.port.to_string());
        cfg.set("password", "");
        cfg.set("motd", format!("Welcome to {}!", inst.name));
        cfg.set("secure", "1");
        // Hearth shares servers through playit; UPnP router changes aren't needed.
        cfg.set("upnp", "0");
    }
    cfg.set("banlist", BANLIST);
    update_world_path(dir, &mut cfg);
    std::fs::create_dir_all(dir.join("worlds")).at(dir.join("worlds"))?;
    cfg.save(&path)
}

#[async_trait]
impl GameProvider for Terraria {
    fn info(&self) -> GameInfo {
        GameInfo {
            id: "terraria",
            name: "Terraria",
            description: "2D sandbox adventure. Friends join from Multiplayer → Join via IP with your address.",
            flavors: vec![FlavorInfo {
                id: "vanilla",
                name: "Dedicated server",
                description: "Re-Logic's official dedicated server from terraria.org.",
            }],
            default_port: 7777,
            protocol: Protocol::Tcp,
            configurable_memory: false,
            eula: None,
            playit_tunnel_type: Some("terraria"),
            command_hint: "Type a command, e.g. say hello, save, or playing",
            setup_note: Some(
                "The world is created the first time you start the server. Set its size, difficulty and name in Settings before you press Start.",
            ),
        }
    }

    async fn versions(&self, ctx: &GameContext, _flavor: &str) -> Result<Vec<GameVersion>> {
        let names: Vec<String> = http::get_json(&ctx.client, VERSIONS_API).await?;
        let build = names.iter().find_map(|n| build_from_zip_name(n)).unwrap_or_default();
        Ok(vec![GameVersion {
            id: "latest".into(),
            label: if build.is_empty() {
                "Latest".into()
            } else {
                format!("Latest ({})", display_version(&build))
            },
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
        progress("Looking up the latest Terraria server", None);
        let names: Vec<String> = http::get_json(&ctx.client, VERSIONS_API).await?;
        let (zip_name, build) = names
            .iter()
            .find_map(|n| build_from_zip_name(n).map(|b| (n.clone(), b)))
            .ok_or_else(|| Error::BadResponse {
                source_name: "terraria.org".into(),
                detail: "no server downloads listed".into(),
            })?;

        let zip = ctx.paths.cache().join(&zip_name);
        // terraria.org publishes no checksum; HTTPS only.
        http::download(
            &ctx.client,
            &format!("{DOWNLOAD_BASE}/{zip_name}"),
            &zip,
            &Checksum::None,
            |f| progress("Downloading Terraria server", f),
        )
        .await?;

        progress("Unpacking", None);
        let (zip2, dir2, prefix) = (zip.clone(), dir.to_path_buf(), format!("{build}/{}", platform_folder()));
        let written = tokio::task::spawn_blocking(move || extract_zip_prefix(&zip2, &prefix, &dir2))
            .await
            .expect("extract task panicked")?;
        let _ = tokio::fs::remove_file(&zip).await;
        if written == 0 || !executable(dir).exists() {
            return Err(Error::BadResponse {
                source_name: "terraria.org".into(),
                detail: format!("the download had no {} server", platform_folder()),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let exe = executable(dir);
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).at(&exe)?;
        }
        tokio::fs::write(dir.join(VERSION_FILE), &build).await.at(dir)?;
        prepare_config(dir, inst, fresh)?;
        Ok(InstallOutcome {
            installed_build: Some(display_version(&build)),
            java_major: None,
            jvm_args: None,
            launch: None,
        })
    }

    fn launch(&self, _ctx: &GameContext, _inst: &Instance, dir: &Path) -> Result<LaunchSpec> {
        let exe = executable(dir);
        if !exe.exists() {
            return Err(Error::InvalidInput(
                "The Terraria server files are missing. Try reinstalling (Settings → Update).".into(),
            ));
        }
        Ok(LaunchSpec {
            program: exe,
            args: vec!["-config".into(), dir.join(CONFIG).display().to_string()],
            cwd: dir.to_path_buf(),
            env: vec![],
        })
    }

    fn parse_line(&self, line: &str) -> Option<LineEvent> {
        parse_terraria_line(line)
    }

    fn stop_command(&self) -> &'static str {
        // Saves the world, then exits.
        "exit"
    }

    fn properties_file(&self) -> Option<&'static str> {
        Some(CONFIG)
    }

    fn port_setting_key(&self) -> &'static str {
        "port"
    }

    fn apply_port(&self, dir: &Path, port: u16) -> Result<()> {
        let path = dir.join(CONFIG);
        let mut cfg = Properties::load(&path)?;
        cfg.set("port", port.to_string());
        cfg.save(&path)
    }

    fn write_settings(&self, dir: &Path, changes: &[(String, String)]) -> Result<()> {
        let path = dir.join(CONFIG);
        let mut cfg = Properties::load(&path)?;
        for (k, v) in changes {
            if k == "world" || k == "worldpath" {
                continue; // derived from the world name
            }
            cfg.set(k, v.clone());
        }
        update_world_path(dir, &mut cfg);
        cfg.save(&path)
    }

    fn players(&self) -> Option<&dyn PlayerAdmin> {
        Some(&TerrariaPlayers)
    }
}

/// Terraria's console has no timestamps: `Server started`, `Bacon has joined.`, `Bacon has left.`
/// Chat shows as `<Name> message`, so lines starting with `<` are never joins.
pub(crate) fn parse_terraria_line(line: &str) -> Option<LineEvent> {
    // World generation progress uses carriage returns; look at the last segment.
    let msg = line.rsplit('\r').next().unwrap_or(line).trim();
    if msg == "Server started" {
        return Some(LineEvent::Ready);
    }
    if msg.starts_with('<') || msg.starts_with(':') {
        return None;
    }
    let name_ok = |n: &str| !n.is_empty() && n.chars().count() <= 20;
    if let Some(n) = msg.strip_suffix(" has joined.").filter(|n| name_ok(n)) {
        return Some(LineEvent::PlayerJoined(n.to_owned()));
    }
    if let Some(n) = msg.strip_suffix(" has left.").filter(|n| name_ok(n)) {
        return Some(LineEvent::PlayerLeft(n.to_owned()));
    }
    None
}

// ---- players --------------------------------------------------------------------------------

pub struct TerrariaPlayers;

/// banlist.txt pairs a `//Name` line with the player's IP on the next line.
fn read_bans(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(BANLIST))
        .map(|s| {
            s.lines()
                .filter_map(|l| l.trim().strip_prefix("//").map(|n| n.trim().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Removes `name` (and the IP line after it) from banlist.txt.
fn unban(dir: &Path, name: &str) -> Result<()> {
    let path = dir.join(BANLIST);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut out: Vec<&str> = vec![];
    let mut skip_next = false;
    for line in text.lines() {
        if skip_next {
            skip_next = false;
            if !line.trim().starts_with("//") {
                continue;
            }
        }
        if line
            .trim()
            .strip_prefix("//")
            .is_some_and(|n| n.trim().eq_ignore_ascii_case(name))
        {
            skip_next = true;
            continue;
        }
        out.push(line);
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    std::fs::write(&path, s).at(&path)
}

fn check_name(n: &str) -> Result<()> {
    if n.is_empty() || n.chars().count() > 20 || n.contains(['\n', '\r']) {
        return Err(Error::InvalidInput(format!(
            "\"{n}\" isn't a valid Terraria player name."
        )));
    }
    Ok(())
}

#[async_trait]
impl PlayerAdmin for TerrariaPlayers {
    fn caps(&self) -> PlayerCaps {
        PlayerCaps {
            roles: vec![],
            can_ban: true,
            game_modes: vec![],
            has_whitelist: false,
            whitelist_label: "",
            role_needs_id: false,
        }
    }

    fn list(&self, dir: &Path, online: &[String], seen: &[SeenPlayer]) -> Result<Vec<PlayerEntry>> {
        let bans = read_bans(dir);
        let mut names: Vec<String> = vec![];
        for n in online.iter().chain(seen.iter().map(|s| &s.name)).chain(bans.iter()) {
            if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                names.push(n.clone());
            }
        }
        let mut out: Vec<PlayerEntry> = names
            .into_iter()
            .map(|name| PlayerEntry {
                online: online.iter().any(|o| o.eq_ignore_ascii_case(&name)),
                role: Role::Player,
                whitelisted: false,
                banned: bans.iter().any(|b| b.eq_ignore_ascii_case(&name)),
                ban_reason: None,
                id: None,
                name,
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

    fn commands(&self, name: &str, action: &PlayerAction) -> Result<Vec<String>> {
        check_name(name)?;
        Ok(vec![match action {
            PlayerAction::Kick { .. } => format!("kick {name}"),
            PlayerAction::Ban { .. } => format!("ban {name}"),
            _ => {
                return Err(Error::InvalidInput(
                    "Terraria servers don't have roles, whitelists or game modes.".into(),
                ))
            }
        }])
    }

    async fn apply_live(
        &self,
        dir: &Path,
        name: &str,
        _seen: &[SeenPlayer],
        action: &PlayerAction,
    ) -> Option<Result<()>> {
        // The server has no unban command; edit the ban list (read again on the next start).
        matches!(action, PlayerAction::Unban).then(|| check_name(name).and_then(|_| unban(dir, name)))
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
            PlayerAction::Unban => unban(dir, name),
            PlayerAction::Kick { .. } | PlayerAction::Ban { .. } => Err(Error::Conflict(
                "The player has to be online for that (Terraria bans by IP address).".into(),
            )),
            _ => Err(Error::InvalidInput(
                "Terraria servers don't have roles, whitelists or game modes.".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn inst() -> Instance {
        Instance {
            id: "terra-0001".into(),
            name: "Ryker's World!".into(),
            game: "terraria".into(),
            flavor: "vanilla".into(),
            version: "latest".into(),
            installed_build: None,
            port: 7790,
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

    #[test]
    fn versions_from_the_real_api_shape() {
        // terraria.org/api/get/dedicated-servers-names, 2026-11-11
        let names: Vec<String> =
            serde_json::from_str(r#"["terraria-server-1458.zip","terraria-server-1458.zip"]"#).unwrap();
        assert_eq!(
            names.iter().find_map(|n| build_from_zip_name(n)).as_deref(),
            Some("1458")
        );
        assert_eq!(display_version("1458"), "1.4.5.8");
        assert_eq!(build_from_zip_name("evil.zip"), None);
        assert_eq!(build_from_zip_name("terraria-server-../x.zip"), None);
    }

    #[test]
    fn config_for_unattended_start() {
        let t = tempfile::tempdir().unwrap();
        prepare_config(t.path(), &inst(), true).unwrap();
        let cfg = Properties::load(&t.path().join(CONFIG)).unwrap();
        assert_eq!(cfg.get("port"), Some("7790"));
        assert_eq!(cfg.get("autocreate"), Some("2"));
        assert_eq!(cfg.get("worldname"), Some("Ryker's World!"));
        assert_eq!(cfg.get("upnp"), Some("0"));
        let world = cfg.get("world").unwrap();
        assert!(world.ends_with("Ryker_s_World_.wld"), "{world}");
        assert!(t.path().join("worlds").is_dir());

        // Renaming the world in Settings moves the world path with it; updates keep settings.
        Terraria
            .write_settings(
                t.path(),
                &[
                    ("worldname".into(), "Hard Mode".into()),
                    ("maxplayers".into(), "16".into()),
                ],
            )
            .unwrap();
        prepare_config(t.path(), &inst(), false).unwrap();
        let cfg = Properties::load(&t.path().join(CONFIG)).unwrap();
        assert!(cfg.get("world").unwrap().ends_with("Hard_Mode.wld"));
        assert_eq!(cfg.get("maxplayers"), Some("16"));
        assert_eq!(cfg.get("port"), Some("7790"));
    }

    #[test]
    fn parses_console_lines() {
        assert_eq!(parse_terraria_line("Server started"), Some(LineEvent::Ready));
        assert_eq!(
            parse_terraria_line("Bacon Dude has joined."),
            Some(LineEvent::PlayerJoined("Bacon Dude".into()))
        );
        assert_eq!(
            parse_terraria_line("Bacon Dude has left."),
            Some(LineEvent::PlayerLeft("Bacon Dude".into()))
        );
        assert_eq!(
            parse_terraria_line("<Griefer> Notch has joined."),
            None,
            "chat can't fake joins"
        );
        assert_eq!(parse_terraria_line("Listening on port 7777"), None);
        assert_eq!(
            parse_terraria_line("Generating world terrain: 10%\rGenerating world terrain: 99%"),
            None
        );
    }

    #[test]
    fn console_commands_and_bans() {
        let c = |a: PlayerAction| TerrariaPlayers.commands("Cool Guy", &a).unwrap().join(";");
        assert_eq!(c(PlayerAction::Kick { reason: None }), "kick Cool Guy");
        assert_eq!(c(PlayerAction::Ban { reason: None }), "ban Cool Guy");
        assert!(TerrariaPlayers.commands("a\nexit", &PlayerAction::Unban).is_err());

        let t = tempfile::tempdir().unwrap();
        std::fs::write(
            t.path().join(BANLIST),
            "//Griefer\n203.0.113.5\n//Cool Guy\n198.51.100.7\n",
        )
        .unwrap();
        let list = TerrariaPlayers.list(t.path(), &["Friend".into()], &[]).unwrap();
        assert_eq!(list.iter().filter(|p| p.banned).count(), 2);
        assert!(list[0].online);
        unban(t.path(), "cool guy").unwrap();
        assert_eq!(
            std::fs::read_to_string(t.path().join(BANLIST)).unwrap(),
            "//Griefer\n203.0.113.5\n"
        );
    }
}
