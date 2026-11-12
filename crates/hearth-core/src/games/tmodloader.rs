//! tModLoader: modded Terraria. A flavor of the Terraria provider.
//!
//! The server is tModLoader's GitHub release zip, run on a Hearth-managed .NET runtime
//! (`dotnet tModLoader.dll -server ...`) rather than through its bash launch scripts.
//! `-tmlsavedirectory` points at the server folder, so mods live in `<server>/Mods` and never
//! mix with the player's own game. Hearth writes `Mods/enabled.json` from the enabled `.tmod`
//! files at every start. Players joining with missing mods get them from the server.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::http::Checksum;

const RELEASES_API: &str = "https://api.github.com/repos/tModLoader/tModLoader/releases?per_page=20";
/// Redirects to `.../releases/download/<tag>/tModLoader.zip` for the latest stable release.
const LATEST_ZIP: &str = "https://github.com/tModLoader/tModLoader/releases/latest/download/tModLoader.zip";
pub const ZIP_NAME: &str = "tModLoader.zip";
pub const MODS: &str = "Mods";
/// Steam app id of tModLoader (its workshop items live under this id).
const STEAM_APP: &str = "1281930";

pub fn download_url(tag: &str) -> String {
    format!("https://github.com/tModLoader/tModLoader/releases/download/{tag}/{ZIP_NAME}")
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GhRelease {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GhAsset {
    pub name: String,
    /// `sha256:<hex>` on releases GitHub has hashed.
    #[serde(default)]
    pub digest: Option<String>,
}

/// The newest stable release that has the server zip, with its checksum when GitHub has one.
pub(crate) fn pick_release(releases: &[GhRelease]) -> Option<(String, Checksum)> {
    releases.iter().filter(|r| !r.prerelease && !r.draft).find_map(|r| {
        let asset = r.assets.iter().find(|a| a.name == ZIP_NAME)?;
        let sum = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(|h| Checksum::Sha256(h.to_owned()))
            .unwrap_or(Checksum::None);
        Some((r.tag_name.clone(), sum))
    })
}

/// `.../releases/download/v2026.09.3.1/tModLoader.zip` -> `v2026.09.3.1`
pub(crate) fn tag_from_url(url: &str) -> Option<String> {
    let rest = url.split_once("/releases/download/")?.1;
    let tag = rest.split('/').next()?;
    (tag.starts_with('v') && tag.len() > 1).then(|| tag.to_owned())
}

/// The latest stable tModLoader. Uses GitHub's API (which also gives a checksum), falling back
/// to GitHub's "latest release" download link if the API is unavailable (e.g. rate-limited).
pub async fn latest(client: &reqwest::Client) -> Result<(String, Checksum)> {
    let api = async {
        let releases: Vec<GhRelease> = client
            .get(RELEASES_API)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok::<_, Error>(pick_release(&releases))
    };
    if let Ok(Some(found)) = api.await {
        return Ok(found);
    }
    let target = crate::http::redirect_target(LATEST_ZIP).await?;
    let tag = tag_from_url(&target).ok_or_else(|| Error::BadResponse {
        source_name: "GitHub".into(),
        detail: "couldn't find the latest tModLoader release".into(),
    })?;
    Ok((tag, Checksum::None))
}

/// `v2026.09.3.1` -> `(2026, 9)`: the version Steam workshop folders are named by.
pub fn release_line(tag: &str) -> Option<(u32, u32)> {
    let mut parts = tag.trim_start_matches('v').split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

// ---- .tmod files ---------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmodHeader {
    /// Internal name, as listed in `enabled.json`.
    pub name: String,
    pub version: String,
    /// tModLoader version the mod was built for.
    pub built_for: String,
}

/// .NET `BinaryReader.ReadString`: 7-bit encoded length, then UTF-8.
fn read_dotnet_string(r: &mut impl Read) -> Option<String> {
    let mut len: usize = 0;
    for shift in (0..35).step_by(7) {
        let mut b = [0u8];
        r.read_exact(&mut b).ok()?;
        len |= ((b[0] & 0x7f) as usize) << shift;
        if b[0] & 0x80 == 0 {
            if len > 4096 {
                return None;
            }
            let mut buf = vec![0; len];
            r.read_exact(&mut buf).ok()?;
            return String::from_utf8(buf).ok();
        }
    }
    None
}

/// Reads a `.tmod` file's header: `TMOD`, tML version, hash (20), signature (256),
/// data length (i32), then the mod's name and version.
pub fn read_header(path: &Path) -> Option<TmodHeader> {
    let mut f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    if &magic != b"TMOD" {
        return None;
    }
    let built_for = read_dotnet_string(&mut f)?;
    let mut skip = [0u8; 20 + 256 + 4];
    f.read_exact(&mut skip).ok()?;
    let name = read_dotnet_string(&mut f)?;
    let version = read_dotnet_string(&mut f)?;
    (!name.is_empty()).then_some(TmodHeader {
        name,
        version,
        built_for,
    })
}

/// Writes `Mods/enabled.json` listing every enabled `.tmod` in the server's Mods folder.
pub fn write_enabled(server_dir: &Path) -> Result<Vec<String>> {
    let mods = server_dir.join(MODS);
    std::fs::create_dir_all(&mods).at(&mods)?;
    let mut names = vec![];
    for e in std::fs::read_dir(&mods).at(&mods)? {
        let path = e.at(&mods)?.path();
        if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("tmod")) {
            let name = read_header(&path)
                .map(|h| h.name)
                .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()));
            names.extend(name);
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    let path = mods.join("enabled.json");
    let json = serde_json::to_string_pretty(&names).expect("strings serialize");
    std::fs::write(&path, json).at(&path)?;
    Ok(names)
}

// ---- copying mods from the player's own tModLoader ----------------------------------------------

/// Where the player's game keeps its mods.
#[derive(Debug, Clone, Default)]
pub struct ClientLocations {
    /// `Documents/My Games/Terraria/tModLoader/Mods` (has `enabled.json` and local `.tmod`s).
    pub mods_dir: Option<PathBuf>,
    /// `<library>/steamapps/workshop/content/1281930` for each Steam library.
    pub workshop_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientImport {
    /// Mods copied to the server (display "Name v1.2").
    pub copied: Vec<String>,
    /// Enabled in the game but no file found for this server version.
    pub missing: Vec<String>,
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    version: Vec<u64>,
}

fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Workshop item folders hold one subfolder per tModLoader line (`2025.5`), or the `.tmod`
/// directly for old items. Picks the newest line the server can load.
fn workshop_files(item: &Path, server_line: Option<(u32, u32)>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(item) else {
        return vec![];
    };
    let mut root_files = vec![];
    let mut lines: Vec<((u32, u32), PathBuf)> = vec![];
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(line) = p.file_name().and_then(|n| release_line(&n.to_string_lossy())) {
                if server_line.is_none_or(|s| line <= s) {
                    lines.push((line, p));
                }
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tmod")) {
            root_files.push(p);
        }
    }
    lines.sort();
    match lines.pop() {
        Some((_, dir)) => std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tmod")))
                    .collect()
            })
            .unwrap_or_default(),
        None => root_files,
    }
}

/// Copies the mods enabled in the player's tModLoader into the server's Mods folder, choosing
/// the newest copy of each that the server's tModLoader version can load.
pub fn copy_client_mods(
    loc: &ClientLocations,
    server_dir: &Path,
    server_line: Option<(u32, u32)>,
) -> Result<ClientImport> {
    let mods_dir = loc
        .mods_dir
        .as_ref()
        .filter(|d| d.join("enabled.json").exists())
        .ok_or_else(|| {
            Error::InvalidInput(
                "Couldn't find tModLoader on this PC. Open tModLoader once, enable your mods, then try again.".into(),
            )
        })?;
    let enabled_path = mods_dir.join("enabled.json");
    let text = std::fs::read_to_string(&enabled_path).at(&enabled_path)?;
    let enabled: Vec<String> = serde_json::from_str(&text).unwrap_or_default();

    let mut found: HashMap<String, Candidate> = HashMap::new();
    let mut consider = |path: PathBuf| {
        if let Some(h) = read_header(&path) {
            let version = version_key(&h.version);
            let better = found.get(&h.name).is_none_or(|c| version > c.version);
            if better {
                found.insert(h.name, Candidate { path, version });
            }
        }
    };
    for root in &loc.workshop_dirs {
        for item in std::fs::read_dir(root).into_iter().flatten().flatten() {
            if item.path().is_dir() {
                workshop_files(&item.path(), server_line)
                    .into_iter()
                    .for_each(&mut consider);
            }
        }
    }
    for e in std::fs::read_dir(mods_dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tmod")) {
            consider(p);
        }
    }

    let dest = server_dir.join(MODS);
    std::fs::create_dir_all(&dest).at(&dest)?;
    let mut out = ClientImport::default();
    for name in enabled {
        match found.get(&name) {
            Some(c) => {
                let target = dest.join(format!("{name}.tmod"));
                let _ = std::fs::remove_file(dest.join(format!("{name}.tmod{}", crate::content::DISABLED_SUFFIX)));
                std::fs::copy(&c.path, &target).at(&c.path)?;
                let v = c.version.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
                out.copied.push(format!("{name} v{v}"));
            }
            None => out.missing.push(name),
        }
    }
    Ok(out)
}

/// The player's tModLoader folders on this PC.
pub fn client_locations() -> ClientLocations {
    let mods_dir = platform::saves_dir().map(|d| d.join("Terraria").join("tModLoader").join(MODS));
    let mut workshop_dirs = vec![];
    for lib in crate::steamlib::libraries() {
        let dir = lib.join("steamapps").join("workshop").join("content").join(STEAM_APP);
        if dir.is_dir() && !workshop_dirs.contains(&dir) {
            workshop_dirs.push(dir);
        }
    }
    ClientLocations {
        mods_dir,
        workshop_dirs,
    }
}

#[cfg(windows)]
mod platform {
    use std::path::PathBuf;

    /// `Documents/My Games` (Documents may be redirected, e.g. to OneDrive).
    pub fn saves_dir() -> Option<PathBuf> {
        crate::steamlib::documents_dir().map(|d| d.join("My Games"))
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::PathBuf;

    /// tModLoader on Linux keeps saves under `~/.local/share/Terraria`.
    pub fn saves_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes a minimal `.tmod` the way tModLoader does (header only; no file table needed).
    pub fn write_tmod(path: &Path, name: &str, version: &str) {
        fn s(out: &mut Vec<u8>, v: &str) {
            out.push(v.len() as u8);
            out.extend_from_slice(v.as_bytes());
        }
        let mut data = vec![];
        s(&mut data, name);
        s(&mut data, version);
        data.extend_from_slice(&0i32.to_le_bytes()); // file count
        let mut out = b"TMOD".to_vec();
        s(&mut out, "2026.9.3.1");
        out.extend_from_slice(&[0u8; 20 + 256]);
        out.extend_from_slice(&(data.len() as i32).to_le_bytes());
        out.extend_from_slice(&data);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn reads_tmod_headers() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("CalamityMod.tmod");
        write_tmod(&p, "CalamityMod", "2.0.4.3");
        assert_eq!(
            read_header(&p),
            Some(TmodHeader {
                name: "CalamityMod".into(),
                version: "2.0.4.3".into(),
                built_for: "2026.9.3.1".into()
            })
        );
        std::fs::write(t.path().join("junk.tmod"), b"nope").unwrap();
        assert_eq!(read_header(&t.path().join("junk.tmod")), None);
    }

    #[test]
    fn enabled_json_lists_enabled_mods_only() {
        let t = tempfile::tempdir().unwrap();
        let mods = t.path().join(MODS);
        write_tmod(&mods.join("BossChecklist.tmod"), "BossChecklist", "1.6");
        write_tmod(&mods.join("RecipeBrowser.tmod.disabled"), "RecipeBrowser", "1.0");
        assert_eq!(write_enabled(t.path()).unwrap(), vec!["BossChecklist"]);
        let json: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(mods.join("enabled.json")).unwrap()).unwrap();
        assert_eq!(json, vec!["BossChecklist"]);
    }

    #[test]
    fn picks_the_stable_release_and_its_checksum() {
        let releases: Vec<GhRelease> =
            serde_json::from_str(include_str!("../../tests/fixtures/tml_releases.json")).unwrap();
        let (tag, sum) = pick_release(&releases).unwrap();
        assert_eq!(tag, "v2026.09.3.1", "skips the preview release");
        assert!(matches!(sum, Checksum::Sha256(h) if h.len() == 64));
        assert_eq!(
            tag_from_url("https://github.com/tModLoader/tModLoader/releases/download/v2026.09.3.1/tModLoader.zip")
                .as_deref(),
            Some("v2026.09.3.1")
        );
        assert_eq!(tag_from_url("https://github.com/tModLoader/tModLoader/releases"), None);
        assert_eq!(release_line("v2026.09.3.1"), Some((2026, 9)));
        assert_eq!(release_line("2025.5"), Some((2025, 5)));
        assert_eq!(
            download_url("v2026.09.3.1"),
            "https://github.com/tModLoader/tModLoader/releases/download/v2026.09.3.1/tModLoader.zip"
        );
    }

    #[test]
    fn copies_the_players_enabled_mods() {
        let t = tempfile::tempdir().unwrap();
        let client = t.path().join("client");
        let workshop = t.path().join("steam/steamapps/workshop/content/1281930");
        let server = t.path().join("server");
        std::fs::create_dir_all(&client).unwrap();
        std::fs::write(
            client.join("enabled.json"),
            r#"["CalamityMod","BossChecklist","MyLocalMod","GoneMod"]"#,
        )
        .unwrap();
        // Workshop item with per-version folders: one too new for the server.
        write_tmod(
            &workshop.join("2824688072/2025.5/CalamityMod.tmod"),
            "CalamityMod",
            "2.0.3",
        );
        write_tmod(
            &workshop.join("2824688072/2026.9/CalamityMod.tmod"),
            "CalamityMod",
            "2.0.4",
        );
        write_tmod(
            &workshop.join("2824688072/2027.1/CalamityMod.tmod"),
            "CalamityMod",
            "2.1.0",
        );
        // Old-style item: file at the root.
        write_tmod(&workshop.join("2669644269/BossChecklist.tmod"), "BossChecklist", "1.6");
        // A mod not on the workshop, and one that's enabled but not enabled for this server.
        write_tmod(&client.join("MyLocalMod.tmod"), "MyLocalMod", "0.1");
        write_tmod(&workshop.join("1/2026.9/Unused.tmod"), "Unused", "1.0");

        let loc = ClientLocations {
            mods_dir: Some(client),
            workshop_dirs: vec![workshop],
        };
        let r = copy_client_mods(&loc, &server, Some((2026, 9))).unwrap();
        assert_eq!(
            r.copied,
            vec!["CalamityMod v2.0.4", "BossChecklist v1.6", "MyLocalMod v0.1"]
        );
        assert_eq!(r.missing, vec!["GoneMod"]);
        assert_eq!(
            read_header(&server.join("Mods/CalamityMod.tmod")).unwrap().version,
            "2.0.4"
        );
        assert!(!server.join("Mods/Unused.tmod").exists());

        let none = ClientLocations::default();
        let err = copy_client_mods(&none, &server, None).unwrap_err().to_string();
        assert!(err.contains("Couldn't find tModLoader"), "{err}");
    }
}
