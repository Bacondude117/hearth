//! Simple Voice Chat (henkelmax) for Minecraft Java: proximity voice chat as a server mod
//! (Fabric/Forge/NeoForge) or Paper plugin. It listens on its own UDP port; players need the
//! mod in their game too.
//!
//! Config: `<config|plugins>/voicechat/voicechat-server.properties` with `port` (UDP) and
//! `voice_host` (the address clients should connect to, e.g. a playit tunnel).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::properties::Properties;

pub const PROJECT: &str = "simple-voice-chat";
pub const DEFAULT_PORT: u16 = 24454;
const MODRINTH_API: &str = "https://api.modrinth.com/v2";

/// Voice chat state stored on an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceChat {
    /// UDP port the voice server listens on.
    pub port: u16,
    /// Jar file name in the mods/plugins folder.
    pub file: String,
    pub version: String,
}

/// Modrinth loader name for a Hearth flavor, if Simple Voice Chat supports it.
pub fn modrinth_loader(flavor: &str) -> Option<&'static str> {
    match flavor {
        "paper" => Some("paper"),
        "fabric" => Some("fabric"),
        "forge" => Some("forge"),
        "neoforge" => Some("neoforge"),
        _ => None,
    }
}

pub fn config_path(server_dir: &Path, flavor: &str) -> PathBuf {
    let base = if flavor == "paper" { "plugins" } else { "config" };
    server_dir
        .join(base)
        .join("voicechat")
        .join("voicechat-server.properties")
}

/// Sets the voice port and the address clients connect to, keeping every other setting.
/// The mod fills in its remaining defaults on first start.
pub fn write_config(server_dir: &Path, flavor: &str, port: u16, voice_host: Option<&str>) -> Result<()> {
    let path = config_path(server_dir, flavor);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
    }
    let mut p = Properties::load(&path)?;
    p.set("port", port.to_string());
    p.set("voice_host", voice_host.unwrap_or(""));
    p.save(&path)
}

pub fn read_voice_host(server_dir: &Path, flavor: &str) -> Option<String> {
    Properties::load(&config_path(server_dir, flavor))
        .ok()?
        .get("voice_host")
        .map(str::to_owned)
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MrVersion {
    pub version_number: String,
    pub version_type: String,
    pub files: Vec<MrFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MrFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    pub hashes: std::collections::HashMap<String, String>,
}

/// The newest build, preferring a release over betas (Modrinth lists newest first).
pub(crate) fn pick_build(versions: &[MrVersion]) -> Option<(&MrVersion, &MrFile)> {
    let v = versions
        .iter()
        .find(|v| v.version_type == "release")
        .or_else(|| versions.first())?;
    let f = v.files.iter().find(|f| f.primary).or_else(|| v.files.first())?;
    Some((v, f))
}

/// Finds the Simple Voice Chat build for this loader and Minecraft version.
pub async fn find_build(client: &reqwest::Client, loader: &str, mc: &str) -> Result<(String, String, String, String)> {
    let versions: Vec<MrVersion> = client
        .get(format!("{MODRINTH_API}/project/{PROJECT}/version"))
        .query(&[
            ("loaders", format!("[\"{loader}\"]")),
            ("game_versions", format!("[\"{mc}\"]")),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let (v, f) = pick_build(&versions).ok_or_else(|| {
        Error::InvalidInput(format!(
            "Simple Voice Chat isn't available for this server type on Minecraft {mc} yet."
        ))
    })?;
    let sha512 = f.hashes.get("sha512").cloned().ok_or_else(|| Error::BadResponse {
        source_name: "Modrinth".into(),
        detail: "voice chat file has no checksum".into(),
    })?;
    Ok((f.url.clone(), sha512, f.filename.clone(), v.version_number.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_from_the_real_modrinth_listing() {
        let v: Vec<MrVersion> =
            serde_json::from_str(include_str!("../../tests/fixtures/svc_fabric_26.3.json")).unwrap();
        let (ver, file) = pick_build(&v).unwrap();
        assert!(ver.version_number.starts_with("fabric-"), "{}", ver.version_number);
        assert!(file.filename.starts_with("voicechat-fabric-") && file.filename.ends_with(".jar"));
        assert_eq!(file.hashes["sha512"].len(), 128);
        assert!(pick_build(&[]).is_none());
    }

    #[test]
    fn prefers_releases_over_betas() {
        let mk = |n: &str, t: &str| MrVersion {
            version_number: n.into(),
            version_type: t.into(),
            files: vec![MrFile {
                url: "u".into(),
                filename: format!("{n}.jar"),
                primary: true,
                hashes: Default::default(),
            }],
        };
        let v = vec![mk("2.7-beta", "beta"), mk("2.6", "release")];
        assert_eq!(pick_build(&v).unwrap().0.version_number, "2.6");
    }

    #[test]
    fn config_location_and_keys() {
        let t = tempfile::tempdir().unwrap();
        assert!(config_path(t.path(), "paper").ends_with("plugins/voicechat/voicechat-server.properties"));
        assert!(config_path(t.path(), "fabric").ends_with("config/voicechat/voicechat-server.properties"));

        let path = config_path(t.path(), "fabric");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# Simple Voice Chat server config\nport=24454\nmax_voice_distance=48.0\nvoice_host=\n",
        )
        .unwrap();
        write_config(t.path(), "fabric", 24460, Some("147.185.221.20:51234")).unwrap();
        let p = Properties::load(&path).unwrap();
        assert_eq!(p.get("port"), Some("24460"));
        assert_eq!(p.get("max_voice_distance"), Some("48.0"), "other settings kept");
        assert_eq!(
            read_voice_host(t.path(), "fabric").as_deref(),
            Some("147.185.221.20:51234")
        );
        write_config(t.path(), "fabric", 24460, None).unwrap();
        assert_eq!(read_voice_host(t.path(), "fabric").as_deref(), Some(""));
    }

    #[test]
    fn supported_server_types() {
        assert_eq!(modrinth_loader("paper"), Some("paper"));
        assert_eq!(modrinth_loader("neoforge"), Some("neoforge"));
        assert_eq!(modrinth_loader("vanilla"), None);
    }
}
