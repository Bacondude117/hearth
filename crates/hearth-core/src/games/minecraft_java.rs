//! Minecraft: Java Edition — Vanilla (Mojang) and Paper (PaperMC).

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use super::mc_loaders::{self, Loader};
use super::mc_packs;
use super::{
    compare_versions, Eula, FlavorInfo, GameContext, GameInfo, GameProvider, GameVersion, InstallOutcome, LaunchSpec,
    LineEvent, Progress,
};
use crate::content::ContentFolder;
use crate::error::{Error, IoContext, Result};
use crate::http::{self, Checksum};
use crate::instance::{Instance, JavaLaunch};
use crate::net::Protocol;
use crate::packs::PackInfo;
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
        .map(|v| GameVersion {
            id: v.id.clone(),
            label: v.id.clone(),
            stable: true,
        })
        .collect()
}

pub(crate) fn paper_versions(p: &PaperProject) -> Vec<GameVersion> {
    let mut all: Vec<&String> = p.versions.values().flatten().collect();
    all.sort_by(|a, b| compare_versions(b, a));
    all.into_iter()
        .map(|v| {
            let stable = !v.contains('-');
            let label = if stable {
                v.clone()
            } else {
                format!("{v} (pre-release)")
            };
            GameVersion {
                id: v.clone(),
                label,
                stable,
            }
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
                FlavorInfo {
                    id: "vanilla",
                    name: "Vanilla",
                    description: "Mojang's official server, unmodified.",
                },
                FlavorInfo {
                    id: "fabric",
                    name: "Fabric",
                    description: "Lightweight mod loader. Most performance and utility mods.",
                },
                FlavorInfo {
                    id: "neoforge",
                    name: "NeoForge",
                    description: "Mod loader for big content mods on newer versions.",
                },
                FlavorInfo {
                    id: "forge",
                    name: "Forge",
                    description: "The classic mod loader. Needed for many older modpacks.",
                },
            ],
            default_port: 25565,
            protocol: Protocol::Tcp,
            configurable_memory: true,
            memory_limit: false,
            eula: Some(Eula {
                text: "Running a Minecraft server requires agreeing to the Minecraft End User License Agreement.",
                links: vec![("Minecraft EULA", "https://aka.ms/MinecraftEULA")],
            }),
            playit_tunnel_type: Some("minecraft-java"),
            command_hint: "Type a command, e.g. say hello or op YourName",
            setup_note: None,
        }
    }

    async fn versions(&self, ctx: &GameContext, flavor: &str) -> Result<Vec<GameVersion>> {
        match flavor {
            "vanilla" => Ok(vanilla_versions(&http::get_json(&ctx.client, MOJANG_MANIFEST).await?)),
            "paper" => Ok(paper_versions(&http::get_json(&ctx.client, PAPER_API).await?)),
            other => match Loader::from_flavor(other) {
                Some(l) => mc_loaders::loader_versions(&ctx.client, l).await,
                None => Err(Error::InvalidInput(format!("Unknown Minecraft Java flavor: {other}"))),
            },
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
        if let Some(loader) = Loader::from_flavor(&inst.flavor) {
            return install_modded(ctx, inst, server_dir, fresh, loader, progress).await;
        }
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
                (
                    server.url.clone(),
                    Checksum::Sha1(server.sha1.clone()),
                    major,
                    None,
                    vec![],
                )
            }
            "paper" => {
                let v: PaperVersion =
                    http::get_json(&ctx.client, &format!("{PAPER_API}/versions/{}", inst.version)).await?;
                let b: PaperBuild = http::get_json(
                    &ctx.client,
                    &format!("{PAPER_API}/versions/{}/builds/latest", inst.version),
                )
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

        http::download(&ctx.client, &url, &jar, &checksum, |f| {
            progress("Downloading server", f)
        })
        .await?;

        finish_setup(inst, server_dir, fresh).await?;
        Ok(InstallOutcome {
            installed_build: build,
            java_major: Some(java_major),
            jvm_args: Some(jvm_args),
            launch: Some(JavaLaunch::Jar { jar: JAR.into() }),
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
        Ok(LaunchSpec {
            program: java,
            args: java_args(inst, server_dir),
            cwd: server_dir.to_path_buf(),
            env: vec![],
            console_encoding: Default::default(),
            memory_limit_mb: None,
            log_file: None,
        })
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

    async fn inspect_pack(&self, ctx: &GameContext, file: &Path) -> Option<Result<PackInfo>> {
        let ext = file.extension()?.to_string_lossy().to_lowercase();
        if !matches!(ext.as_str(), "mrpack" | "zip") {
            return None;
        }
        Some(mc_packs::inspect(&ctx.client, file).await)
    }

    fn content_folder(&self, inst: &Instance) -> Option<ContentFolder> {
        match inst.flavor.as_str() {
            "paper" => Some(ContentFolder {
                label: "Plugins",
                folder: "plugins",
                extension: "jar",
                import_label: None,
            }),
            f if Loader::from_flavor(f).is_some() => Some(ContentFolder {
                label: "Mods",
                folder: "mods",
                extension: "jar",
                import_label: None,
            }),
            _ => None,
        }
    }

    fn server_icon_file(&self) -> Option<(&'static str, u32)> {
        // Shown next to the server in players' multiplayer list; must be 64x64 PNG.
        Some(("server-icon.png", 64))
    }

    fn describe_content(&self, file: &Path) -> Option<(String, Option<String>)> {
        super::mc_content::describe_jar(file)
    }

    fn parse_identity(&self, line: &str) -> Option<(String, Option<String>)> {
        // "[22:52:49 INFO]: UUID of player Bacon_Dude117 is 33f7864d-..."
        let msg = line.split_once("]: ").map(|(_, m)| m).unwrap_or(line).trim_end();
        let (name, uuid) = msg.strip_prefix("UUID of player ")?.split_once(" is ")?;
        (super::java_players::valid_name(name) && uuid::Uuid::parse_str(uuid).is_ok())
            .then(|| (name.to_owned(), Some(uuid.to_owned())))
    }
}

/// JVM arguments for any Java-edition server. Hearth's memory flags come after the pack's own
/// `user_jvm_args.txt` so they take precedence.
pub(crate) fn java_args(inst: &Instance, server_dir: &Path) -> Vec<String> {
    let mem = inst.memory_mb.unwrap_or(DEFAULT_MEMORY_MB);
    let mut args = vec![];
    let launch = inst.launch.clone().unwrap_or(JavaLaunch::Jar { jar: JAR.into() });
    if matches!(launch, JavaLaunch::ArgsFile { .. }) && server_dir.join("user_jvm_args.txt").exists() {
        args.push("@user_jvm_args.txt".to_owned());
    }
    args.extend([format!("-Xms{mem}M"), format!("-Xmx{mem}M")]);
    args.extend(inst.jvm_args.iter().cloned());
    match launch {
        JavaLaunch::Jar { jar } => args.extend(["-jar".into(), jar]),
        JavaLaunch::ArgsFile { args_file } => args.push(format!("@{args_file}")),
    }
    args.push("nogui".into());
    args
}

/// EULA, and on a fresh install the port and MOTD (keeping a pack's own MOTD).
async fn finish_setup(inst: &Instance, server_dir: &Path, fresh: bool) -> Result<()> {
    // The installer only runs after the user accepted the EULA in the UI.
    let eula = server_dir.join("eula.txt");
    tokio::fs::write(
        &eula,
        "# Accepted in Hearth (https://aka.ms/MinecraftEULA)\neula=true\n",
    )
    .await
    .at(&eula)?;
    if fresh {
        let path = server_dir.join("server.properties");
        let mut props = Properties::load(&path)?;
        props.set("server-port", inst.port.to_string());
        // Minecraft 26.x generates `white-list=true`; start new servers open (users can turn
        // the whitelist on in Settings). Packs that ship their own value keep it.
        if props.get("white-list").is_none() {
            props.set("white-list", "false");
            props.set("enforce-whitelist", "false");
        }
        if props
            .get("motd")
            .is_none_or(|m| m.is_empty() || m == "A Minecraft Server")
        {
            props.set("motd", inst.name.clone());
        }
        props.save(&path)?;
    }
    Ok(())
}

/// Fabric / Forge / NeoForge: unpack the modpack (if any), install Java and the loader.
async fn install_modded(
    ctx: &GameContext,
    inst: &Instance,
    dir: &Path,
    fresh: bool,
    loader: Loader,
    progress: Progress<'_>,
) -> Result<InstallOutcome> {
    let pack_file = inst
        .pack
        .as_ref()
        .and_then(|p| p.file.as_ref())
        .map(|f| ctx.paths.instance_dir(&inst.id).join(f))
        .filter(|p| p.exists());
    let pinned = inst.pack.as_ref().and_then(|p| p.loader_version.clone());

    if let Some(file) = &pack_file {
        mc_packs::unpack(ctx, file, dir, progress).await?;
    } else if !fresh && inst.pack.is_none() {
        // Version change on a plain modded server: start the loader install clean.
        mc_loaders::clear_loader_files(dir, loader);
    }

    progress("Looking up Minecraft version", None);
    let detail = vanilla_detail(ctx, &inst.version).await?;
    let major = detail.java_version.as_ref().map(|j| j.major).unwrap_or(8);
    let java = ctx.java.ensure(major, |stage, f| progress(stage, f)).await?;

    let (launch, build) = match loader {
        Loader::Fabric => match mc_loaders::find_fabric_launcher(dir) {
            Some(jar) => {
                let v = mc_loaders::parse_fabric_launcher_name(&jar).map(|(_, l)| l).or(pinned);
                (JavaLaunch::Jar { jar }, v)
            }
            None => {
                let v =
                    mc_loaders::install_fabric(&ctx.client, &inst.version, pinned.as_deref(), dir, progress).await?;
                (
                    JavaLaunch::Jar {
                        jar: mc_loaders::FABRIC_LAUNCHER.into(),
                    },
                    Some(v),
                )
            }
        },
        Loader::Forge | Loader::NeoForge => {
            let mut version = pinned.clone();
            if mc_loaders::find_args_file(dir).is_none() {
                let installer = match mc_loaders::find_installer_jar(dir) {
                    Some(p) => p,
                    None => {
                        let (p, v) = mc_loaders::download_installer(
                            &ctx.client,
                            loader,
                            &inst.version,
                            pinned.as_deref(),
                            dir,
                            progress,
                        )
                        .await?;
                        version = Some(v);
                        p
                    }
                };
                mc_loaders::run_installer(&java, &installer, dir, loader, progress).await?;
                let _ = tokio::fs::remove_file(&installer).await;
                let _ = tokio::fs::remove_file(dir.join(format!("{}.log", installer.display()))).await;
            }
            let args_file = mc_loaders::find_args_file(dir).ok_or_else(|| {
                Error::InvalidInput(format!(
                    "The {} install finished but its start files are missing. Try reinstalling.",
                    loader.display()
                ))
            })?;
            let v = mc_loaders::parse_args_path(&args_file).map(|(_, _, v)| v).or(version);
            (JavaLaunch::ArgsFile { args_file }, v)
        }
    };

    if loader == Loader::Fabric {
        // Fabric's launcher would otherwise fetch the vanilla jar on first start, without
        // progress. It looks for `.fabric/server/<mc>-server.jar` (see fabric-installer's
        // ServerLauncher), so put a checksum-verified copy there now.
        if let Some(server) = detail.downloads.get("server") {
            let jar = dir.join(".fabric/server").join(format!("{}-server.jar", inst.version));
            if !jar.exists() {
                http::download(
                    &ctx.client,
                    &server.url,
                    &jar,
                    &Checksum::Sha1(server.sha1.clone()),
                    |f| progress("Downloading Minecraft server", f),
                )
                .await?;
            }
        }
    }

    finish_setup(inst, dir, fresh).await?;
    Ok(InstallOutcome {
        installed_build: Some(match build {
            Some(v) => format!("{} {v}", loader.display()),
            None => loader.display().to_owned(),
        }),
        java_major: Some(major),
        jvm_args: Some(vec![]),
        launch: Some(launch),
    })
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
    // Minecraft writes a crash report, then shuts down "cleanly" with exit code 0.
    if msg.starts_with("This crash report has been saved to:") || msg == "Encountered an unexpected exception" {
        return Some(LineEvent::Crashed);
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
        assert_eq!(
            parse_java_line("[22:52:49 INFO]: UUID of player Bacon_Dude117 is 33f7864d-a02c-4e21-ae28-3ecf6f0fd767"),
            None
        );
        assert_eq!(
            parse_java_line("[22:53:06 INFO]: System chat: There are 1 of a max of 20 players online: Bacon_Dude117"),
            None
        );
    }

    fn test_instance() -> Instance {
        Instance {
            id: "t-0001".into(),
            name: "Fresh".into(),
            game: "minecraft-java".into(),
            flavor: "paper".into(),
            version: "26.3".into(),
            installed_build: None,
            port: 25570,
            memory_mb: Some(2048),
            java_major: Some(25),
            jvm_args: vec![],
            install_state: crate::instance::InstallState::Installed,
            created_at: chrono::Utc::now(),
            playit_tunnel_id: None,
            launch: None,
            pack: None,
            voice: None,
        }
    }

    /// Minecraft 26.x generates `white-list=true` by default; new Hearth servers start open.
    #[tokio::test]
    async fn new_servers_start_with_the_whitelist_off() {
        let t = tempfile::tempdir().unwrap();
        finish_setup(&test_instance(), t.path(), true).await.unwrap();
        let p = Properties::load(&t.path().join("server.properties")).unwrap();
        assert_eq!(p.get("white-list"), Some("false"));
        assert_eq!(p.get("enforce-whitelist"), Some("false"));
        assert_eq!(p.get("server-port"), Some("25570"));

        // Reinstalls/updates don't touch the user's choice.
        let mut p = p;
        p.set("white-list", "true");
        p.save(&t.path().join("server.properties")).unwrap();
        finish_setup(&test_instance(), t.path(), false).await.unwrap();
        let p = Properties::load(&t.path().join("server.properties")).unwrap();
        assert_eq!(p.get("white-list"), Some("true"));
    }

    #[test]
    fn detects_crash_reports() {
        assert_eq!(
            parse_java_line("[00:28:08] [Server thread/ERROR]: This crash report has been saved to: C:\\x\\crash.txt"),
            Some(LineEvent::Crashed)
        );
        assert_eq!(
            parse_java_line("[00:28:08 ERROR]: Encountered an unexpected exception"),
            Some(LineEvent::Crashed)
        );
        assert_eq!(
            parse_java_line("[00:28:08 INFO]: <bob> Encountered an unexpected exception"),
            None
        );
    }

    #[test]
    fn parses_identity_lines() {
        assert_eq!(
            MinecraftJava.parse_identity(
                "[22:52:49 INFO]: UUID of player Bacon_Dude117 is 33f7864d-a02c-4e21-ae28-3ecf6f0fd767"
            ),
            Some((
                "Bacon_Dude117".into(),
                Some("33f7864d-a02c-4e21-ae28-3ecf6f0fd767".into())
            ))
        );
        assert_eq!(
            MinecraftJava
                .parse_identity("[22:52:49 INFO]: <bob> UUID of player Eve is 33f7864d-a02c-4e21-ae28-3ecf6f0fd767"),
            None
        );
        assert_eq!(
            MinecraftJava.parse_identity("[22:52:49 INFO]: UUID of player Eve is nope"),
            None
        );
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
        assert_eq!(
            parse_java_line("[12:01:00 INFO]: <bob> System chat: Steve joined the game"),
            None
        );
        assert_eq!(
            parse_java_line("[12:01:00 INFO]: [Not Secure] <bob> System chat: Steve joined the game"),
            None
        );
        assert_eq!(
            parse_java_line("[12:01:00 INFO]: Starting minecraft server version 26.3"),
            None
        );
    }
}
