//! Minecraft: Java Edition — Vanilla (Mojang) and Paper (PaperMC).

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use super::{
    compare_versions, Eula, FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome,
    LaunchSpec, LineEvent, Progress,
};
use crate::error::{Error, IoContext, Result};
use crate::http::{self, Checksum};
use crate::instance::Instance;
use crate::net::Protocol;
use crate::properties::Properties;

const MOJANG_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const PAPER_API: &str = "https://fill.papermc.io/v3/projects/paper";
const JAR: &str = "server.jar";
pub const DEFAULT_MEMORY_MB: u32 = 2048;

pub struct MinecraftJava;

// ---- Mojang ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VersionDetail {
    pub downloads: HashMap<String, DownloadInfo>,
    #[serde(rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DownloadInfo {
    pub sha1: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct JavaVersion {
    #[serde(rename = "majorVersion")]
    pub major: u32,
}

// ---- Paper (Fill v3) -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PaperProject {
    /// Version family ("1.21") -> versions ("1.21.11", "1.21.11-rc3", ...).
    pub versions: HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaperVersion {
    pub version: PaperVersionInfo,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaperVersionInfo {
    pub java: PaperJava,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaperJava {
    pub version: PaperJavaVersion,
    #[serde(default)]
    pub flags: PaperFlags,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaperJavaVersion {
    pub minimum: u32,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PaperFlags {
    #[serde(default)]
    pub recommended: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaperBuild {
    pub id: u64,
    pub downloads: HashMap<String, PaperDownload>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaperDownload {
    pub url: String,
    pub checksums: PaperChecksums,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaperChecksums {
    pub sha256: String,
}

pub(crate) fn vanilla_versions(m: &Manifest) -> Vec<GameVersion> {
    // The manifest is already newest-first. Snapshots are left out to keep the list usable.
    m.versions
        .iter()
        .filter(|v| v.kind == "release")
        .map(|v| GameVersion { id: v.id.clone(), label: v.id.clone(), stable: true })
        .collect()
}

pub(crate) fn paper_versions(p: &PaperProject) -> Vec<GameVersion> {
    let mut all: Vec<&String> = p.versions.values().flatten().collect();
    all.sort_by(|a, b| compare_versions(b, a));
    all.into_iter()
        .map(|v| {
            let stable = !v.contains('-');
            let label = if stable { v.clone() } else { format!("{v} (pre-release)") };
            GameVersion { id: v.clone(), label, stable }
        })
        .collect()
}

#[async_trait]
impl GameProvider for MinecraftJava {
    fn info(&self) -> GameInfo {
        GameInfo {
            id: "minecraft-java",
            name: "Minecraft: Java Edition",
            description: "The original PC edition. Players join from the Java launcher.",
            flavors: vec![
                FlavorInfo {
                    id: "paper",
                    name: "Paper",
                    description: "Faster than vanilla and supports Bukkit/Paper plugins. Recommended.",
                },
                FlavorInfo { id: "vanilla", name: "Vanilla", description: "Mojang's official server, unmodified." },
            ],
            default_port: 25565,
            protocol: Protocol::Tcp,
            configurable_memory: true,
            eula: Some(Eula {
                text: "Running a Minecraft server requires agreeing to the Minecraft End User License Agreement.",
                links: vec![("Minecraft EULA", "https://aka.ms/MinecraftEULA")],
            }),
            playit_tunnel_type: Some("minecraft-java"),
        }
    }

    async fn versions(&self, ctx: &GameContext, flavor: &str) -> Result<Vec<GameVersion>> {
        match flavor {
            "vanilla" => Ok(vanilla_versions(&http::get_json(&ctx.client, MOJANG_MANIFEST).await?)),
            "paper" => Ok(paper_versions(&http::get_json(&ctx.client, PAPER_API).await?)),
            other => Err(Error::InvalidInput(format!("Unknown Minecraft Java flavor: {other}"))),
        }
    }

    async fn install(
        &self,
        ctx: &GameContext,
        inst: &Instance,
        server_dir: &Path,
        fresh: bool,
        progress: Progress<'_>,
    ) -> Result<InstallOutcome> {
        tokio::fs::create_dir_all(server_dir).await.at(server_dir)?;
        let jar = server_dir.join(JAR);

        progress("Looking up server version", None);
        let (url, checksum, java_major, build, jvm_args) = match inst.flavor.as_str() {
            "vanilla" => {
                let detail = vanilla_detail(ctx, &inst.version).await?;
                let server = detail.downloads.get("server").ok_or_else(|| {
                    Error::InvalidInput(format!("Minecraft {} has no official server download", inst.version))
                })?;
                // Versions too old to declare one ran on Java 8.
                let major = detail.java_version.map(|j| j.major).unwrap_or(8);
                (server.url.clone(), Checksum::Sha1(server.sha1.clone()), major, None, vec![])
            }
            "paper" => {
                let v: PaperVersion =
                    http::get_json(&ctx.client, &format!("{PAPER_API}/versions/{}", inst.version)).await?;
                let b: PaperBuild =
                    http::get_json(&ctx.client, &format!("{PAPER_API}/versions/{}/builds/latest", inst.version))
                        .await?;
                let d = b.downloads.get("server:default").ok_or_else(|| Error::BadResponse {
                    source_name: "PaperMC".into(),
                    detail: "build has no server download".into(),
                })?;
                (
                    d.url.clone(),
                    Checksum::Sha256(d.checksums.sha256.clone()),
                    v.version.java.version.minimum,
                    Some(format!("build {}", b.id)),
                    v.version.java.flags.recommended,
                )
            }
            other => return Err(Error::InvalidInput(format!("Unknown Minecraft Java flavor: {other}"))),
        };

        // Java first: it is the bigger download and a failure there should not leave a new jar.
        ctx.java.ensure(java_major, |stage, f| progress(stage, f)).await?;

        http::download(&ctx.client, &url, &jar, &checksum, |f| progress("Downloading server", f)).await?;

        // The installer only runs after the user accepted the EULA in the UI.
        let eula = server_dir.join("eula.txt");
        tokio::fs::write(&eula, "# Accepted in Hearth (https://aka.ms/MinecraftEULA)\neula=true\n")
            .await
            .at(&eula)?;

        if fresh {
            let path = server_dir.join("server.properties");
            let mut props = Properties::load(&path)?;
            props.set("server-port", inst.port.to_string());
            props.set("motd", inst.name.clone());
            props.save(&path)?;
        }

        Ok(InstallOutcome {
            installed_build: build,
            java_major: Some(java_major),
            jvm_args: Some(jvm_args),
        })
    }

    fn launch(&self, ctx: &GameContext, inst: &Instance, server_dir: &Path) -> Result<LaunchSpec> {
        let major = inst
            .java_major
            .ok_or_else(|| Error::InvalidInput("This server is not fully installed. Try reinstalling it.".into()))?;
        let java = ctx
            .java
            .installed(major)
            .ok_or_else(|| Error::InvalidInput(format!("Java {major} is missing. Try reinstalling the server.")))?;
        let mem = inst.memory_mb.unwrap_or(DEFAULT_MEMORY_MB);
        let mut args = vec![format!("-Xms{mem}M"), format!("-Xmx{mem}M")];
        args.extend(inst.jvm_args.iter().cloned());
        args.extend(["-jar".into(), JAR.into(), "nogui".into()]);
        Ok(LaunchSpec { program: java, args, cwd: server_dir.to_path_buf(), env: vec![] })
    }

    fn parse_line(&self, line: &str) -> Option<LineEvent> {
        parse_java_line(line)
    }

    fn apply_port(&self, server_dir: &Path, port: u16) -> Result<()> {
        let path = server_dir.join("server.properties");
        let mut props = Properties::load(&path)?;
        props.set("server-port", port.to_string());
        props.save(&path)
    }

    fn players(&self) -> Option<&dyn crate::players::PlayerAdmin> {
        Some(&super::java_players::JavaPlayers)
    }

    fn parse_identity(&self, line: &str) -> Option<(String, Option<String>)> {
        // "[22:52:49 INFO]: UUID of player Bacon_Dude117 is 33f7864d-..."
        let msg = line.split_once("]: ").map(|(_, m)| m).unwrap_or(line).trim_end();
        let (name, uuid) = msg.strip_prefix("UUID of player ")?.split_once(" is ")?;
        (super::java_players::valid_name(name) && uuid::Uuid::parse_str(uuid).is_ok())
            .then(|| (name.to_owned(), Some(uuid.to_owned())))
    }
}

async fn vanilla_detail(ctx: &GameContext, version: &str) -> Result<VersionDetail> {
    let manifest: Manifest = http::get_json(&ctx.client, MOJANG_MANIFEST).await?;
    let entry = manifest
        .versions
        .iter()
        .find(|v| v.id == version)
        .ok_or_else(|| Error::InvalidInput(format!("Unknown Minecraft version {version}")))?;
    http::get_json(&ctx.client, &entry.url).await
}

/// Handles both Vanilla (`[12:00:00] [Server thread/INFO]: msg`) and Paper
/// (`[12:00:00 INFO]: msg`) log prefixes.
pub(crate) fn parse_java_line(line: &str) -> Option<LineEvent> {
    let msg = line.split_once("]: ").map(|(_, m)| m).unwrap_or(line).trim_end();
    if msg.starts_with("Done (") && msg.contains("For help, type") {
        return Some(LineEvent::Ready);
    }
    // Minecraft 26.x logs server-sent messages as "System chat: <message>". Player chat never
    // starts this way (it starts with "<name>"), so this can't be spoofed from chat.
    let msg = msg.strip_prefix("System chat: ").unwrap_or(msg);
    if let Some(name) = msg.strip_suffix(" joined the game").filter(|n| is_player_name(n)) {
        return Some(LineEvent::PlayerJoined(name.to_owned()));
    }
    if let Some(name) = msg.strip_suffix(" left the game").filter(|n| is_player_name(n)) {
        return Some(LineEvent::PlayerLeft(name.to_owned()));
    }
    None
}

/// Java names are 3-16 of `[A-Za-z0-9_]`; Geyser/Floodgate prefixes Bedrock players with `.`.
/// Rejecting anything else stops chat messages like `<bob> x joined the game` from matching.
fn is_player_name(n: &str) -> bool {
    let n = n.strip_prefix('.').unwrap_or(n);
    (1..=16).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_fixture() {
        let m: Manifest = serde_json::from_str(include_str!("../../tests/fixtures/mojang_manifest.json")).unwrap();
        let v = vanilla_versions(&m);
        assert!(!v.is_empty());
        assert!(v.iter().all(|v| !v.id.contains("snapshot")));
        assert_eq!(v[0].id, "26.3");
    }

    #[test]
    fn parses_version_detail_fixture() {
        let d: VersionDetail = serde_json::from_str(include_str!("../../tests/fixtures/mojang_26.3.json")).unwrap();
        assert_eq!(d.java_version.unwrap().major, 25);
        let s = &d.downloads["server"];
        assert_eq!(s.sha1.len(), 40);
        assert!(s.url.ends_with("server.jar"));
    }

    #[test]
    fn parses_paper_fixtures() {
        let p: PaperProject = serde_json::from_str(include_str!("../../tests/fixtures/paper_project.json")).unwrap();
        let v = paper_versions(&p);
        assert_eq!(v[0].id, "26.3");
        assert!(v[0].stable);
        assert!(v.iter().any(|v| !v.stable));
        // Newest first across families.
        let pos = |id: &str| v.iter().position(|x| x.id == id).unwrap();
        assert!(pos("26.1.2") < pos("1.21.11"));

        let pv: PaperVersion =
            serde_json::from_str(include_str!("../../tests/fixtures/paper_version_26.3.json")).unwrap();
        assert_eq!(pv.version.java.version.minimum, 25);
        assert!(pv.version.java.flags.recommended.iter().any(|f| f.contains("G1")));

        let b: PaperBuild = serde_json::from_str(include_str!("../../tests/fixtures/paper_build_latest.json")).unwrap();
        assert_eq!(b.downloads["server:default"].checksums.sha256.len(), 64);
    }

    /// 26.x prefixes server-sent messages with "System chat: " (real Paper 26.3 output).
    #[test]
    fn parses_system_chat_prefixed_lines() {
        assert_eq!(
            parse_java_line("[22:52:51 INFO]: System chat: Bacon_Dude117 joined the game"),
            Some(LineEvent::PlayerJoined("Bacon_Dude117".into()))
        );
        assert_eq!(
            parse_java_line("[22:59:10 INFO]: System chat: Bacon_Dude117 left the game"),
            Some(LineEvent::PlayerLeft("Bacon_Dude117".into()))
        );
        assert_eq!(
            parse_java_line("[02:55:43] [Server thread/INFO]: System chat: .GeyserFriend joined the game"),
            Some(LineEvent::PlayerJoined(".GeyserFriend".into()))
        );
        // Other lines around a join must not count.
        assert_eq!(parse_java_line("[22:52:49 INFO]: UUID of player Bacon_Dude117 is 33f7864d-a02c-4e21-ae28-3ecf6f0fd767"), None);
        assert_eq!(parse_java_line("[22:53:06 INFO]: System chat: There are 1 of a max of 20 players online: Bacon_Dude117"), None);
    }

    #[test]
    fn parses_identity_lines() {
        assert_eq!(
            MinecraftJava.parse_identity("[22:52:49 INFO]: UUID of player Bacon_Dude117 is 33f7864d-a02c-4e21-ae28-3ecf6f0fd767"),
            Some(("Bacon_Dude117".into(), Some("33f7864d-a02c-4e21-ae28-3ecf6f0fd767".into())))
        );
        assert_eq!(MinecraftJava.parse_identity("[22:52:49 INFO]: <bob> UUID of player Eve is 33f7864d-a02c-4e21-ae28-3ecf6f0fd767"), None);
        assert_eq!(MinecraftJava.parse_identity("[22:52:49 INFO]: UUID of player Eve is nope"), None);
    }

    #[test]
    fn parses_log_lines() {
        let ready_vanilla = "[12:00:01] [Server thread/INFO]: Done (3.214s)! For help, type \"help\"";
        let ready_paper = "[12:00:01 INFO]: Done (5.551s)! For help, type \"help\"";
        assert_eq!(parse_java_line(ready_vanilla), Some(LineEvent::Ready));
        assert_eq!(parse_java_line(ready_paper), Some(LineEvent::Ready));
        assert_eq!(
            parse_java_line("[12:01:00 INFO]: Steve_2 joined the game"),
            Some(LineEvent::PlayerJoined("Steve_2".into()))
        );
        assert_eq!(
            parse_java_line("[12:01:00] [Server thread/INFO]: .BedrockGuy left the game"),
            Some(LineEvent::PlayerLeft(".BedrockGuy".into()))
        );
        // Chat spoofing.
        assert_eq!(parse_java_line("[12:01:00 INFO]: <bob> Steve joined the game"), None);
        assert_eq!(parse_java_line("[12:01:00 INFO]: <bob> System chat: Steve joined the game"), None);
        assert_eq!(parse_java_line("[12:01:00 INFO]: [Not Secure] <bob> System chat: Steve joined the game"), None);
        assert_eq!(parse_java_line("[12:01:00 INFO]: Starting minecraft server version 26.3"), None);
    }
}
