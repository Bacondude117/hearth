//! Minecraft mod loaders: Fabric, Forge and NeoForge.
//!
//! Fabric ships a self-contained server launcher jar. Forge and NeoForge ship an installer that
//! builds the server (`--installServer`) and leaves `user_jvm_args.txt` plus
//! `libraries/.../{win,unix}_args.txt`, which the server is started with.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{compare_versions, GameVersion, Progress};
use crate::error::{Error, Result};
use crate::http;

pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
pub const FORGE_PROMOS: &str = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
pub const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
pub const NEOFORGE_VERSIONS: &str = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

/// The launcher jar name Hearth gives Fabric servers it installs.
pub const FABRIC_LAUNCHER: &str = "fabric-server-launch.jar";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Fabric,
    Forge,
    NeoForge,
}

impl Loader {
    pub fn from_flavor(f: &str) -> Option<Self> {
        match f {
            "fabric" => Some(Self::Fabric),
            "forge" => Some(Self::Forge),
            "neoforge" => Some(Self::NeoForge),
            _ => None,
        }
    }

    pub fn flavor(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }
}

// ---- Fabric ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct FabricVersion {
    pub version: String,
    pub stable: bool,
}

pub(crate) fn fabric_game_versions(list: &[FabricVersion]) -> Vec<GameVersion> {
    list.iter()
        .filter(|v| v.stable)
        .map(|v| GameVersion {
            id: v.version.clone(),
            label: v.version.clone(),
            stable: true,
        })
        .collect()
}

pub(crate) fn latest_stable(list: &[FabricVersion]) -> Option<String> {
    list.iter()
        .find(|v| v.stable)
        .or(list.first())
        .map(|v| v.version.clone())
}

pub async fn fabric_versions(client: &reqwest::Client) -> Result<Vec<GameVersion>> {
    let list: Vec<FabricVersion> = http::get_json(client, &format!("{FABRIC_META}/versions/game")).await?;
    Ok(fabric_game_versions(&list))
}

/// Downloads the Fabric server launcher for `mc`. Returns the loader version used.
pub async fn install_fabric(
    client: &reqwest::Client,
    mc: &str,
    loader: Option<&str>,
    dir: &Path,
    progress: Progress<'_>,
) -> Result<String> {
    let loader = match loader {
        Some(l) => l.to_owned(),
        None => {
            let list: Vec<FabricVersion> = http::get_json(client, &format!("{FABRIC_META}/versions/loader")).await?;
            latest_stable(&list).ok_or_else(|| bad("Fabric", "no loader versions"))?
        }
    };
    let installers: Vec<FabricVersion> = http::get_json(client, &format!("{FABRIC_META}/versions/installer")).await?;
    let installer = latest_stable(&installers).ok_or_else(|| bad("Fabric", "no installer versions"))?;
    let url = format!("{FABRIC_META}/versions/loader/{mc}/{loader}/{installer}/server/jar");
    // Fabric publishes no checksum for the generated launcher; HTTPS only.
    http::download(client, &url, &dir.join(FABRIC_LAUNCHER), &http::Checksum::None, |f| {
        progress("Downloading Fabric", f)
    })
    .await?;
    Ok(loader)
}

// ---- Forge ----------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct ForgePromos {
    pub promos: HashMap<String, String>,
}

pub(crate) fn forge_game_versions(p: &ForgePromos) -> Vec<GameVersion> {
    let mut mcs: Vec<&str> = p
        .promos
        .keys()
        .filter_map(|k| k.rsplit_once('-').map(|(mc, _)| mc))
        .collect();
    mcs.sort_by(|a, b| compare_versions(b, a));
    mcs.dedup();
    mcs.into_iter()
        .map(|v| GameVersion {
            id: v.into(),
            label: v.into(),
            stable: true,
        })
        .collect()
}

/// Recommended build for `mc`, else the latest.
pub(crate) fn forge_pick(p: &ForgePromos, mc: &str) -> Option<String> {
    p.promos
        .get(&format!("{mc}-recommended"))
        .or_else(|| p.promos.get(&format!("{mc}-latest")))
        .cloned()
}

pub fn forge_installer_url(mc: &str, forge: &str) -> String {
    format!("{FORGE_MAVEN}/{mc}-{forge}/forge-{mc}-{forge}-installer.jar")
}

// ---- NeoForge -------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct NeoVersions {
    pub versions: Vec<String>,
}

/// NeoForge version -> Minecraft version.
/// New scheme (2025+): `26.3.0.101` -> `26.3`, `26.1.2.125` -> `26.1.2`.
/// Old scheme: `21.1.77` -> `1.21.1`, `21.0.5` -> `1.21`.
pub fn neoforge_mc_version(v: &str) -> Option<String> {
    if v.contains('-') {
        return None; // betas and odd snapshot builds
    }
    let parts: Vec<u32> = v.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [major, minor, patch, _build] if *major >= 25 => Some(if *patch == 0 {
            format!("{major}.{minor}")
        } else {
            format!("{major}.{minor}.{patch}")
        }),
        [major, minor, _build] if (20..25).contains(major) => Some(if *minor == 0 {
            format!("1.{major}")
        } else {
            format!("1.{major}.{minor}")
        }),
        _ => None,
    }
}

pub(crate) fn neoforge_game_versions(list: &NeoVersions) -> Vec<GameVersion> {
    let mut mcs: Vec<String> = list.versions.iter().filter_map(|v| neoforge_mc_version(v)).collect();
    mcs.sort_by(|a, b| compare_versions(b, a));
    mcs.dedup();
    mcs.into_iter()
        .map(|v| GameVersion {
            label: v.clone(),
            id: v,
            stable: true,
        })
        .collect()
}

/// Newest stable NeoForge build for `mc`.
pub(crate) fn neoforge_pick(list: &NeoVersions, mc: &str) -> Option<String> {
    list.versions
        .iter()
        .filter(|v| neoforge_mc_version(v).as_deref() == Some(mc))
        .max_by(|a, b| compare_versions(a, b))
        .cloned()
}

pub fn neoforge_installer_url(v: &str) -> String {
    format!("{NEOFORGE_MAVEN}/{v}/neoforge-{v}-installer.jar")
}

// ---- Forge / NeoForge installers ------------------------------------------------------------

pub async fn loader_versions(client: &reqwest::Client, loader: Loader) -> Result<Vec<GameVersion>> {
    match loader {
        Loader::Fabric => fabric_versions(client).await,
        Loader::Forge => Ok(forge_game_versions(&http::get_json(client, FORGE_PROMOS).await?)),
        Loader::NeoForge => Ok(neoforge_game_versions(
            &http::get_json(client, NEOFORGE_VERSIONS).await?,
        )),
    }
}

/// Resolves the Forge/NeoForge build for `mc` and downloads its installer into `dir`.
pub async fn download_installer(
    client: &reqwest::Client,
    loader: Loader,
    mc: &str,
    pinned: Option<&str>,
    dir: &Path,
    progress: Progress<'_>,
) -> Result<(PathBuf, String)> {
    let version = match (pinned, loader) {
        (Some(v), _) => v.to_owned(),
        (None, Loader::Forge) => forge_pick(&http::get_json(client, FORGE_PROMOS).await?, mc)
            .ok_or_else(|| Error::InvalidInput(format!("Forge has no build for Minecraft {mc}.")))?,
        (None, Loader::NeoForge) => neoforge_pick(&http::get_json(client, NEOFORGE_VERSIONS).await?, mc)
            .ok_or_else(|| Error::InvalidInput(format!("NeoForge has no build for Minecraft {mc}.")))?,
        (None, Loader::Fabric) => unreachable!("Fabric has no installer step"),
    };
    let url = match loader {
        Loader::Forge => forge_installer_url(mc, &version),
        _ => neoforge_installer_url(&version),
    };
    let path = dir.join(format!("{}-installer.jar", loader.flavor()));
    // The mavens publish .sha1 files next to each artifact.
    let sha1 = client
        .get(format!("{url}.sha1"))
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success());
    let checksum = match sha1 {
        Some(r) => r
            .text()
            .await
            .ok()
            .map(|t| t.split_whitespace().next().unwrap_or_default().to_owned())
            .filter(|h| h.len() == 40)
            .map(http::Checksum::Sha1)
            .unwrap_or(http::Checksum::None),
        None => http::Checksum::None,
    };
    http::download(client, &url, &path, &checksum, |f| {
        progress(&format!("Downloading {} installer", loader.display()), f)
    })
    .await?;
    Ok((path, version))
}

/// Runs a Forge/NeoForge installer in server mode inside `dir`.
pub async fn run_installer(
    java: &Path,
    installer: &Path,
    dir: &Path,
    loader: Loader,
    progress: Progress<'_>,
) -> Result<()> {
    progress(
        &format!("Installing {} (this can take a few minutes)", loader.display()),
        None,
    );
    let mut cmd = tokio::process::Command::new(java);
    cmd.arg("-jar")
        .arg(installer)
        .arg("--installServer")
        .arg(".")
        .current_dir(dir);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(crate::system::CHILD_PROCESS_FLAGS);
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::InvalidInput(format!("Could not run the {} installer: {e}", loader.display())))?;

    // Keep the tail of the output for the error message.
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let tail = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<String>::new()));
    let collect = |stream: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                   tail: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>| async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let mut t = tail.lock().unwrap();
            if t.len() >= 15 {
                t.pop_front();
            }
            t.push_back(l);
        }
    };
    let (a, b) = (
        collect(Box::new(stdout), tail.clone()),
        collect(Box::new(stderr), tail.clone()),
    );
    let status = tokio::time::timeout(std::time::Duration::from_secs(20 * 60), async {
        let (_, _, s) = tokio::join!(a, b, child.wait());
        s
    })
    .await
    .map_err(|_| {
        Error::InvalidInput(format!(
            "The {} installer took too long and was stopped.",
            loader.display()
        ))
    })??;

    if !status.success() {
        let tail = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
        return Err(Error::InvalidInput(format!(
            "The {} installer failed (exit code {}).\n{tail}",
            loader.display(),
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        )));
    }
    Ok(())
}

fn args_file_name() -> &'static str {
    if cfg!(windows) {
        "win_args.txt"
    } else {
        "unix_args.txt"
    }
}

/// Finds Forge/NeoForge's generated args file (relative to `dir`), newest version first.
pub fn find_args_file(dir: &Path) -> Option<String> {
    let mut found: Vec<(String, String)> = vec![]; // (version dir name, relative path)
    for base in [
        "libraries/net/neoforged/neoforge",
        "libraries/net/minecraftforge/forge",
        "libraries/net/neoforged/forge",
    ] {
        let Ok(entries) = std::fs::read_dir(dir.join(base)) else {
            continue;
        };
        for e in entries.flatten() {
            let candidate = e.path().join(args_file_name());
            if candidate.is_file() {
                let v = e.file_name().to_string_lossy().into_owned();
                found.push((v.clone(), format!("{base}/{v}/{}", args_file_name())));
            }
        }
    }
    found.sort_by(|a, b| compare_versions(&b.0, &a.0));
    found.into_iter().next().map(|(_, p)| p)
}

/// A Fabric server launcher in `dir`, if present (as named by Hearth, the installer, or meta).
pub fn find_fabric_launcher(dir: &Path) -> Option<String> {
    if dir.join(FABRIC_LAUNCHER).is_file() {
        return Some(FABRIC_LAUNCHER.into());
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.starts_with("fabric-server-") && n.ends_with(".jar")).then_some(n)
    })
}

/// A Forge/NeoForge installer jar sitting in `dir` (server packs often include one).
pub fn find_installer_jar(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        let n = p
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        n.ends_with(".jar") && n.contains("installer") && (n.contains("forge") || n.contains("neoforge"))
    })
}

#[derive(Debug, Deserialize)]
struct InstallProfile {
    #[serde(default)]
    profile: String,
    version: String,
    minecraft: String,
}

/// Reads an installer's `install_profile.json` -> (loader, Minecraft version, loader version).
pub fn parse_install_profile(json: &str) -> Option<(Loader, String, String)> {
    let p: InstallProfile = serde_json::from_str(json).ok()?;
    if let Some(v) = p.version.strip_prefix("neoforge-") {
        return Some((Loader::NeoForge, p.minecraft, v.to_owned()));
    }
    if let Some((_, v)) = p.version.split_once("-forge-") {
        let loader = if p.profile.eq_ignore_ascii_case("neoforge") {
            Loader::NeoForge
        } else {
            Loader::Forge
        };
        return Some((loader, p.minecraft, v.to_owned()));
    }
    None
}

pub fn read_install_profile_from_jar(bytes: &[u8]) -> Option<(Loader, String, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut s = String::new();
    z.by_name("install_profile.json").ok()?.read_to_string(&mut s).ok()?;
    parse_install_profile(&s)
}

/// `fabric-server-mc.26.3-loader.0.20.1-launcher.1.1.2.jar` -> (`26.3`, `0.20.1`)
pub fn parse_fabric_launcher_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("fabric-server-mc.")?;
    let (mc, rest) = rest.split_once("-loader.")?;
    let (loader, _) = rest.split_once("-launcher.")?;
    Some((mc.to_owned(), loader.to_owned()))
}

/// `.../forge/26.3-66.1.0/win_args.txt` -> (Forge, `26.3`, `66.1.0`);
/// `.../neoforge/26.3.0.101/win_args.txt` -> (NeoForge, `26.3`, `26.3.0.101`).
pub fn parse_args_path(path: &str) -> Option<(Loader, Option<String>, String)> {
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    let v = *parts.get(parts.len().checked_sub(2)?)?;
    if parts.contains(&"neoforge") {
        return Some((Loader::NeoForge, neoforge_mc_version(v), v.to_owned()));
    }
    if parts.contains(&"minecraftforge") {
        let (mc, forge) = v.split_once('-')?;
        return Some((Loader::Forge, Some(mc.to_owned()), forge.to_owned()));
    }
    None
}

/// Removes files a previous loader install generated, so a version change starts clean.
pub fn clear_loader_files(dir: &Path, loader: Loader) {
    let targets: &[&str] = match loader {
        Loader::Fabric => &[FABRIC_LAUNCHER, ".fabric"],
        Loader::Forge => &["libraries/net/minecraftforge", "run.bat", "run.sh"],
        Loader::NeoForge => &["libraries/net/neoforged", "run.bat", "run.sh"],
    };
    for t in targets {
        let p = dir.join(t);
        let _ = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
    }
}

fn bad(source: &str, detail: &str) -> Error {
    Error::BadResponse {
        source_name: source.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neoforge_version_mapping() {
        assert_eq!(neoforge_mc_version("26.3.0.101").as_deref(), Some("26.3"));
        assert_eq!(neoforge_mc_version("26.1.2.125").as_deref(), Some("26.1.2"));
        assert_eq!(neoforge_mc_version("21.1.77").as_deref(), Some("1.21.1"));
        assert_eq!(neoforge_mc_version("21.0.150").as_deref(), Some("1.21"));
        assert_eq!(neoforge_mc_version("20.4.237").as_deref(), Some("1.20.4"));
        assert_eq!(neoforge_mc_version("21.0.5-beta"), None);
        assert_eq!(neoforge_mc_version("0.25w14craftmine.3-beta"), None);
    }

    #[test]
    fn neoforge_fixture() {
        let l: NeoVersions = serde_json::from_str(include_str!("../../tests/fixtures/neoforge_versions.json")).unwrap();
        let mcs: Vec<String> = neoforge_game_versions(&l).into_iter().map(|v| v.id).collect();
        assert_eq!(mcs, vec!["26.3", "26.1.2", "1.21.1", "1.21"]);
        assert_eq!(neoforge_pick(&l, "26.3").as_deref(), Some("26.3.0.101"));
        assert_eq!(neoforge_pick(&l, "1.21.1").as_deref(), Some("21.1.79"));
    }

    #[test]
    fn forge_fixture() {
        let p: ForgePromos = serde_json::from_str(include_str!("../../tests/fixtures/forge_promos.json")).unwrap();
        let mcs: Vec<String> = forge_game_versions(&p).into_iter().map(|v| v.id).collect();
        assert_eq!(mcs.first().map(String::as_str), Some("26.3"));
        assert!(mcs.contains(&"1.20.1".to_string()));
        assert_eq!(
            forge_pick(&p, "26.3").as_deref(),
            Some("66.1.0"),
            "recommended wins over latest"
        );
        assert_eq!(
            forge_installer_url("26.3", "66.1.0"),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/26.3-66.1.0/forge-26.3-66.1.0-installer.jar"
        );
    }

    #[test]
    fn fabric_fixtures() {
        let g: Vec<FabricVersion> =
            serde_json::from_str(include_str!("../../tests/fixtures/fabric_game.json")).unwrap();
        let v = fabric_game_versions(&g);
        assert!(v.iter().all(|v| !v.id.contains("snapshot")));
        assert!(v.iter().any(|v| v.id == "1.21.1"));
        let l: Vec<FabricVersion> =
            serde_json::from_str(include_str!("../../tests/fixtures/fabric_loader.json")).unwrap();
        assert_eq!(latest_stable(&l).as_deref(), Some("0.20.1"));
    }

    #[test]
    fn install_profiles() {
        assert_eq!(
            parse_install_profile(include_str!("../../tests/fixtures/forge_install_profile.json")),
            Some((Loader::Forge, "26.3".into(), "66.1.0".into()))
        );
        assert_eq!(
            parse_install_profile(include_str!("../../tests/fixtures/neoforge_install_profile.json")),
            Some((Loader::NeoForge, "26.3".into(), "26.3.0.101".into()))
        );
    }

    #[test]
    fn launcher_and_args_paths() {
        assert_eq!(
            parse_fabric_launcher_name("fabric-server-mc.26.3-loader.0.20.1-launcher.1.1.2.jar"),
            Some(("26.3".into(), "0.20.1".into()))
        );
        assert_eq!(
            parse_args_path("libraries/net/minecraftforge/forge/26.3-66.1.0/win_args.txt"),
            Some((Loader::Forge, Some("26.3".into()), "66.1.0".into()))
        );
        assert_eq!(
            parse_args_path("libraries/net/neoforged/neoforge/26.3.0.101/unix_args.txt"),
            Some((Loader::NeoForge, Some("26.3".into()), "26.3.0.101".into()))
        );
    }

    #[test]
    fn finds_generated_launch_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert_eq!(find_args_file(d), None);
        for v in ["26.1.2.125", "26.3.0.101"] {
            let p = d.join("libraries/net/neoforged/neoforge").join(v);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join(args_file_name()), "-jar x").unwrap();
        }
        assert_eq!(
            find_args_file(d),
            Some(format!(
                "libraries/net/neoforged/neoforge/26.3.0.101/{}",
                args_file_name()
            ))
        );

        std::fs::write(d.join("fabric-server-mc.26.3-loader.0.20.1-launcher.1.1.2.jar"), "").unwrap();
        assert_eq!(
            find_fabric_launcher(d).as_deref(),
            Some("fabric-server-mc.26.3-loader.0.20.1-launcher.1.1.2.jar")
        );
        std::fs::write(d.join("forge-1.20.1-47.3.0-installer.jar"), "").unwrap();
        assert!(find_installer_jar(d).is_some());
    }
}
