//! Managed .NET runtimes (Microsoft's portable builds), for games built on .NET such as
//! tModLoader.
//!
//! Each channel (e.g. `8.0`) lives in `runtimes/dotnet-<channel>/` and is shared by every server
//! that needs it. Downloads are verified against the SHA-512 Microsoft publishes.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::archive::{extract_zip, ExtractOptions};
use crate::error::{Error, IoContext, Result};
use crate::http::{self, Checksum};
use crate::paths::AppPaths;

const METADATA: &str = "https://builds.dotnet.microsoft.com/dotnet/release-metadata";
const MARKER: &str = ".hearth-runtime";

pub struct DotnetRuntimes {
    paths: AppPaths,
    client: reqwest::Client,
    lock: Mutex<()>,
}

#[derive(Debug, Deserialize)]
struct ReleasesIndex {
    #[serde(rename = "releases-index")]
    releases_index: Vec<Channel>,
}

#[derive(Debug, Deserialize)]
struct Channel {
    #[serde(rename = "channel-version")]
    channel_version: String,
    #[serde(rename = "latest-runtime")]
    latest_runtime: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseFile {
    release: Release,
}

#[derive(Debug, Deserialize)]
struct Release {
    runtime: Runtime,
}

#[derive(Debug, Deserialize)]
struct Runtime {
    version: String,
    files: Vec<RuntimeFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RuntimeFile {
    pub name: String,
    pub rid: String,
    pub url: String,
    pub hash: String,
}

/// .NET's runtime identifier for this computer.
fn rid() -> &'static str {
    match (cfg!(windows), cfg!(target_arch = "aarch64")) {
        (true, false) => "win-x64",
        (true, true) => "win-arm64",
        (false, false) => "linux-x64",
        (false, true) => "linux-arm64",
    }
}

/// The portable archive (zip on Windows, tar.gz elsewhere) for `rid`.
pub(crate) fn pick_file<'a>(files: &'a [RuntimeFile], rid: &str) -> Option<&'a RuntimeFile> {
    let ext = if rid.starts_with("win-") { ".zip" } else { ".tar.gz" };
    files
        .iter()
        .find(|f| f.rid == rid && f.name.starts_with("dotnet-runtime-") && f.name.ends_with(ext))
}

/// `8.0.0` (from a runtimeconfig) -> `8.0`.
pub fn channel_of(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let (major, minor) = (parts.next()?, parts.next()?);
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    (numeric(major) && numeric(minor)).then(|| format!("{major}.{minor}"))
}

/// The .NET channel an app needs, from its `<app>.runtimeconfig.json`.
pub fn required_channel(runtimeconfig: &Path) -> Result<String> {
    #[derive(Deserialize)]
    struct Config {
        #[serde(rename = "runtimeOptions")]
        runtime_options: Options,
    }
    #[derive(Deserialize)]
    struct Options {
        framework: Framework,
    }
    #[derive(Deserialize)]
    struct Framework {
        version: String,
    }
    let text = std::fs::read_to_string(runtimeconfig).at(runtimeconfig)?;
    let cfg: Config = serde_json::from_str(&text).map_err(|e| Error::BadResponse {
        source_name: runtimeconfig.display().to_string(),
        detail: e.to_string(),
    })?;
    channel_of(&cfg.runtime_options.framework.version).ok_or_else(|| Error::BadResponse {
        source_name: runtimeconfig.display().to_string(),
        detail: format!("unexpected .NET version {}", cfg.runtime_options.framework.version),
    })
}

pub fn dotnet_exe(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(if cfg!(windows) { "dotnet.exe" } else { "dotnet" })
}

impl DotnetRuntimes {
    pub fn new(paths: AppPaths, client: reqwest::Client) -> Self {
        Self {
            paths,
            client,
            lock: Mutex::new(()),
        }
    }

    pub fn dir(&self, channel: &str) -> PathBuf {
        self.paths.runtimes().join(format!("dotnet-{channel}"))
    }

    /// The `dotnet` host for `channel`, if installed.
    pub fn installed(&self, channel: &str) -> Option<PathBuf> {
        let dir = self.dir(channel);
        let exe = dotnet_exe(&dir);
        (dir.join(MARKER).exists() && exe.exists()).then_some(exe)
    }

    /// Returns the `dotnet` host for `channel`, downloading the latest runtime of that channel
    /// first if needed.
    pub async fn ensure(&self, channel: &str, mut progress: impl FnMut(&str, Option<f64>) + Send) -> Result<PathBuf> {
        let _guard = self.lock.lock().await;
        if let Some(exe) = self.installed(channel) {
            return Ok(exe);
        }

        progress(&format!("Finding .NET {channel}"), None);
        let index: ReleasesIndex = http::get_json(&self.client, &format!("{METADATA}/releases-index.json")).await?;
        let version = index
            .releases_index
            .iter()
            .find(|c| c.channel_version == channel)
            .map(|c| c.latest_runtime.clone())
            .ok_or_else(|| Error::BadResponse {
                source_name: "Microsoft".into(),
                detail: format!(".NET {channel} isn't listed"),
            })?;
        let release: ReleaseFile =
            http::get_json(&self.client, &format!("{METADATA}/{channel}/{version}/release.json")).await?;
        let file = pick_file(&release.release.runtime.files, rid())
            .cloned()
            .ok_or_else(|| Error::BadResponse {
                source_name: "Microsoft".into(),
                detail: format!("no .NET {version} runtime for this computer"),
            })?;

        let archive = self.paths.cache().join(&file.name);
        http::download(
            &self.client,
            &file.url,
            &archive,
            &Checksum::Sha512(file.hash.to_lowercase()),
            |f| progress(&format!("Downloading .NET {channel}"), f),
        )
        .await?;

        progress(&format!("Unpacking .NET {channel}"), None);
        let dir = self.dir(channel);
        let staging = dir.with_extension("staging");
        let (archive2, staging2) = (archive.clone(), staging.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            if staging2.exists() {
                std::fs::remove_dir_all(&staging2).at(&staging2)?;
            }
            unpack(&archive2, &staging2)
        })
        .await
        .expect("unpack task panicked")?;

        if dir.exists() {
            tokio::fs::remove_dir_all(&dir).await.at(&dir)?;
        }
        tokio::fs::rename(&staging, &dir).await.at(&dir)?;
        tokio::fs::write(dir.join(MARKER), &release.release.runtime.version)
            .await
            .at(&dir)?;
        let _ = tokio::fs::remove_file(&archive).await;

        let exe = dotnet_exe(&dir);
        if !exe.exists() {
            return Err(Error::BadResponse {
                source_name: "Microsoft".into(),
                detail: format!("the .NET {version} download had no dotnet program"),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).at(&exe)?;
        }
        Ok(exe)
    }
}

/// Microsoft's portable archives hold the runtime at their root (no top-level folder).
fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    if archive.to_string_lossy().ends_with(".zip") {
        return extract_zip(archive, dest, &ExtractOptions::default());
    }
    let f = std::fs::File::open(archive).at(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    std::fs::create_dir_all(dest).at(dest)?;
    for entry in tar.entries().at(archive)? {
        let mut entry = entry.at(archive)?;
        let path = entry.path().at(archive)?.into_owned();
        let rel: PathBuf = path.components().filter(|c| !matches!(c, Component::CurDir)).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(Error::Archive(format!("unsafe path in archive: {}", path.display())));
        }
        entry.unpack(dest.join(rel)).at(archive)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_latest_runtime_from_real_metadata() {
        let index: ReleasesIndex =
            serde_json::from_str(include_str!("../tests/fixtures/dotnet_releases_index.json")).unwrap();
        let eight = index
            .releases_index
            .iter()
            .find(|c| c.channel_version == "8.0")
            .unwrap();
        assert!(eight.latest_runtime.starts_with("8.0."));

        let release: ReleaseFile =
            serde_json::from_str(include_str!("../tests/fixtures/dotnet_8_release.json")).unwrap();
        let files = &release.release.runtime.files;
        let win = pick_file(files, "win-x64").unwrap();
        assert!(
            win.name.ends_with("win-x64.zip") || win.url.ends_with("win-x64.zip"),
            "{}",
            win.url
        );
        assert_eq!(win.hash.len(), 128, "sha512");
        let linux = pick_file(files, "linux-x64").unwrap();
        assert!(linux.url.ends_with("linux-x64.tar.gz"));
    }

    #[test]
    fn channels() {
        assert_eq!(channel_of("8.0.0").as_deref(), Some("8.0"));
        assert_eq!(channel_of("10.0.1").as_deref(), Some("10.0"));
        assert_eq!(channel_of("8"), None);
        assert_eq!(channel_of("x.y.z"), None);

        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("tModLoader.runtimeconfig.json");
        std::fs::write(
            &p,
            r#"{"runtimeOptions":{"tfm":"net8.0","framework":{"name":"Microsoft.NETCore.App","version":"8.0.0"}}}"#,
        )
        .unwrap();
        assert_eq!(required_channel(&p).unwrap(), "8.0");
    }
}
