//! Minecraft: Bedrock Edition — Mojang's official Bedrock Dedicated Server (BDS).
//!
//! Mojang only publishes the newest build, so the only "version" offered is `latest`, and
//! updating means re-downloading it while keeping worlds and settings.

use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use super::{
    Eula, FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome, LaunchSpec, LineEvent, Progress,
};
use crate::archive::{extract_zip, ExtractOptions};
use crate::error::{Error, Result};
use crate::http::{self, Checksum};
use crate::instance::Instance;
use crate::net::Protocol;
use crate::properties::Properties;

const LINKS_API: &str = "https://net-secondary.web.minecraft-services.net/api/v1.0/download/links";

/// Files and folders a BDS update must never overwrite.
pub const PRESERVE_ON_UPDATE: &[&str] = &[
    "worlds",
    "server.properties",
    "permissions.json",
    "allowlist.json",
    "whitelist.json",
    "config",
];

pub struct MinecraftBedrock;

#[derive(Debug, Deserialize)]
pub(crate) struct LinksResponse {
    pub result: LinksResult,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LinksResult {
    pub links: Vec<Link>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Link {
    #[serde(rename = "downloadType")]
    pub download_type: String,
    #[serde(rename = "downloadUrl")]
    pub download_url: String,
}

fn platform_download_type() -> &'static str {
    if cfg!(windows) {
        "serverBedrockWindows"
    } else {
        "serverBedrockLinux"
    }
}

pub(crate) fn pick_link(links: &LinksResponse) -> Result<&Link> {
    links
        .result
        .links
        .iter()
        .find(|l| l.download_type == platform_download_type())
        .ok_or_else(|| Error::BadResponse {
            source_name: "minecraft.net".into(),
            detail: format!("no {} download listed", platform_download_type()),
        })
}

/// `.../bedrock-server-1.26.63.1.zip` -> `1.26.63.1`
pub(crate) fn version_from_url(url: &str) -> Option<String> {
    let file = url.rsplit('/').next()?;
    let v = file.strip_prefix("bedrock-server-")?.strip_suffix(".zip")?;
    (!v.is_empty()).then(|| v.to_owned())
}

fn executable(server_dir: &Path) -> std::path::PathBuf {
    if cfg!(windows) {
        server_dir.join("bedrock_server.exe")
    } else {
        server_dir.join("bedrock_server")
    }
}

#[async_trait]
impl GameProvider for MinecraftBedrock {
    fn info(&self) -> GameInfo {
        GameInfo {
            id: "minecraft-bedrock",
            name: "Minecraft: Bedrock Edition",
            description: "For Windows, consoles and mobile players. Uses Mojang's official server.",
            flavors: vec![FlavorInfo {
                id: "bedrock",
                name: "Bedrock Dedicated Server",
                description: "Mojang's official server. Always the latest version.",
            }],
            default_port: 19132,
            protocol: Protocol::Udp,
            configurable_memory: false,
            eula: Some(Eula {
                text: "Running the Bedrock Dedicated Server requires agreeing to the Minecraft End User License Agreement and the Microsoft Privacy Policy.",
                links: vec![
                    ("Minecraft EULA", "https://minecraft.net/terms"),
                    ("Privacy Policy", "https://go.microsoft.com/fwlink/?LinkId=521839"),
                ],
            }),
            playit_tunnel_type: Some("minecraft-bedrock"),
            command_hint: "Type a command, e.g. say hello or op \"Your Gamertag\"",
            setup_note: None,
        }
    }

    async fn versions(&self, ctx: &GameContext, _flavor: &str) -> Result<Vec<GameVersion>> {
        let links: LinksResponse = http::get_json(&ctx.client, LINKS_API).await?;
        let link = pick_link(&links)?;
        let v = version_from_url(&link.download_url).unwrap_or_else(|| "unknown".into());
        Ok(vec![GameVersion {
            id: "latest".into(),
            label: format!("Latest ({v})"),
            stable: true,
        }])
    }

    async fn install(
        &self,
        ctx: &GameContext,
        inst: &Instance,
        server_dir: &Path,
        fresh: bool,
        progress: Progress<'_>,
    ) -> Result<InstallOutcome> {
        progress("Looking up latest Bedrock server", None);
        let links: LinksResponse = http::get_json(&ctx.client, LINKS_API).await?;
        let link = pick_link(&links)?;
        let version = version_from_url(&link.download_url);

        let zip = ctx.paths.cache().join(format!(
            "bedrock-{}-{}.zip",
            inst.id,
            version.as_deref().unwrap_or("latest")
        ));
        // Mojang publishes no checksum for BDS; we rely on HTTPS and the zip's own CRCs.
        http::download(&ctx.client, &link.download_url, &zip, &Checksum::None, |f| {
            progress("Downloading Bedrock server", f)
        })
        .await?;

        progress("Unpacking", None);
        let opts = ExtractOptions {
            strip_single_root: false,
            preserve: if fresh {
                vec![]
            } else {
                PRESERVE_ON_UPDATE.iter().map(|s| s.to_string()).collect()
            },
        };
        let (zip2, dir2) = (zip.clone(), server_dir.to_path_buf());
        tokio::task::spawn_blocking(move || extract_zip(&zip2, &dir2, &opts))
            .await
            .expect("extract task panicked")?;
        let _ = tokio::fs::remove_file(&zip).await;

        if fresh {
            let path = server_dir.join("server.properties");
            let mut props = Properties::load(&path)?;
            props.set("server-name", inst.name.clone());
            props.save(&path)?;
            self.apply_port(server_dir, inst.port)?;
        }

        Ok(InstallOutcome {
            installed_build: version,
            java_major: None,
            jvm_args: None,
            launch: None,
        })
    }

    fn launch(&self, _ctx: &GameContext, _inst: &Instance, server_dir: &Path) -> Result<LaunchSpec> {
        let exe = executable(server_dir);
        if !exe.exists() {
            return Err(Error::InvalidInput(
                "The Bedrock server files are missing. Try reinstalling.".into(),
            ));
        }
        let env = if cfg!(windows) {
            vec![]
        } else {
            vec![("LD_LIBRARY_PATH".into(), ".".into())]
        };
        Ok(LaunchSpec {
            program: exe,
            args: vec![],
            cwd: server_dir.to_path_buf(),
            env,
        })
    }

    fn parse_line(&self, line: &str) -> Option<LineEvent> {
        parse_bedrock_line(line)
    }

    fn ports_used(&self, port: u16) -> Vec<(u16, Protocol)> {
        vec![(port, Protocol::Udp), (port.saturating_add(1), Protocol::Udp)]
    }

    fn players(&self) -> Option<&dyn crate::players::PlayerAdmin> {
        Some(&super::bedrock_players::BedrockPlayers)
    }

    fn parse_identity(&self, line: &str) -> Option<(String, Option<String>)> {
        // "[...] Player connected: Cool Guy 22, xuid: 2535412345"
        let msg = line.split_once("] ").map(|(_, m)| m).unwrap_or(line);
        let rest = msg.strip_prefix("Player connected: ")?;
        let (name, tail) = rest.split_once(", xuid: ")?;
        let xuid: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!name.trim().is_empty()).then(|| (name.trim().to_owned(), (!xuid.is_empty()).then_some(xuid)))
    }

    fn apply_port(&self, server_dir: &Path, port: u16) -> Result<()> {
        let path = server_dir.join("server.properties");
        let mut props = Properties::load(&path)?;
        props.set("server-port", port.to_string());
        // BDS also binds an IPv6 port; keep it next to the main one so servers don't collide.
        props.set("server-portv6", port.saturating_add(1).to_string());
        props.save(&path)
    }
}

/// Lines look like `[2026-11-09 12:00:00:123 INFO] Player connected: Steve, xuid: 2535...`.
pub(crate) fn parse_bedrock_line(line: &str) -> Option<LineEvent> {
    let msg = match line.find("] ") {
        Some(i) if line.starts_with('[') => &line[i + 2..],
        _ => line,
    }
    .trim_end();
    if msg == "Server started." {
        return Some(LineEvent::Ready);
    }
    let name_of = |rest: &str| {
        rest.split(", xuid")
            .next()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
    };
    if let Some(rest) = msg.strip_prefix("Player connected: ") {
        return name_of(rest).map(LineEvent::PlayerJoined);
    }
    if let Some(rest) = msg.strip_prefix("Player disconnected: ") {
        return name_of(rest).map(LineEvent::PlayerLeft);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links_fixture() {
        let l: LinksResponse = serde_json::from_str(include_str!("../../tests/fixtures/bedrock_links.json")).unwrap();
        let link = pick_link(&l).unwrap();
        assert!(link.download_url.contains("bedrock-server-"));
        assert!(version_from_url(&link.download_url).unwrap().starts_with("1."));
    }

    #[test]
    fn version_from_url_works() {
        assert_eq!(
            version_from_url("https://www.minecraft.net/bedrockdedicatedserver/bin-win/bedrock-server-1.26.63.1.zip"),
            Some("1.26.63.1".into())
        );
        assert_eq!(version_from_url("https://x/y.zip"), None);
    }

    #[test]
    fn parses_log_lines() {
        assert_eq!(
            parse_bedrock_line("[2026-11-09 12:00:00:123 INFO] Server started."),
            Some(LineEvent::Ready)
        );
        assert_eq!(
            parse_bedrock_line("[2026-11-09 12:00:00:123 INFO] Player connected: Cool Guy 22, xuid: 2535412345"),
            Some(LineEvent::PlayerJoined("Cool Guy 22".into()))
        );
        assert_eq!(
            parse_bedrock_line(
                "[2026-11-09 12:00:00:123 INFO] Player disconnected: Cool Guy 22, xuid: 2535412345, pfid: abc"
            ),
            Some(LineEvent::PlayerLeft("Cool Guy 22".into()))
        );
        assert_eq!(
            parse_bedrock_line("[2026-11-09 12:00:00:123 INFO] Starting Server"),
            None
        );
    }

    #[test]
    fn parses_identity() {
        assert_eq!(
            MinecraftBedrock.parse_identity(
                "[2026-11-09 12:00:00:123 INFO] Player connected: Cool Guy 22, xuid: 2535412345, pfid: abc"
            ),
            Some(("Cool Guy 22".into(), Some("2535412345".into())))
        );
        assert_eq!(
            MinecraftBedrock.parse_identity("[2026-11-09 12:00:00:123 INFO] Player disconnected: X, xuid: 1"),
            None
        );
    }

    #[test]
    fn apply_port_sets_both_ports() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("server.properties"),
            "server-port=19132\nserver-portv6=19133\n",
        )
        .unwrap();
        MinecraftBedrock.apply_port(tmp.path(), 19140).unwrap();
        let p = Properties::load(&tmp.path().join("server.properties")).unwrap();
        assert_eq!(p.get("server-port"), Some("19140"));
        assert_eq!(p.get("server-portv6"), Some("19141"));
    }
}
