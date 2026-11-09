//! Managed Java runtimes (Eclipse Temurin via the Adoptium API).
//!
//! Each feature version lives in `runtimes/java-<major>/` and is shared by every server
//! that needs it.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::archive::{extract_zip, ExtractOptions};
use crate::error::{Error, IoContext, Result};
use crate::http::{self, Checksum};
use crate::paths::AppPaths;

const ADOPTIUM: &str = "https://api.adoptium.net/v3";
const MARKER: &str = ".hearth-runtime";

pub struct JavaRuntimes {
    paths: AppPaths,
    client: reqwest::Client,
    /// Serializes installs so two servers needing Java 21 don't download it twice.
    lock: Mutex<()>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    binary: Binary,
    release_name: String,
}

#[derive(Debug, Deserialize)]
struct Binary {
    package: Package,
}

#[derive(Debug, Deserialize)]
struct Package {
    link: String,
    checksum: String,
    name: String,
}

impl JavaRuntimes {
    pub fn new(paths: AppPaths, client: reqwest::Client) -> Self {
        Self { paths, client, lock: Mutex::new(()) }
    }

    pub fn dir(&self, major: u32) -> PathBuf {
        self.paths.runtimes().join(format!("java-{major}"))
    }

    /// Path to the java executable for `major`, if installed.
    pub fn installed(&self, major: u32) -> Option<PathBuf> {
        let dir = self.dir(major);
        let exe = java_exe(&dir);
        (dir.join(MARKER).exists() && exe.exists()).then_some(exe)
    }

    /// Returns the java executable for `major`, downloading it first if needed.
    pub async fn ensure(&self, major: u32, mut progress: impl FnMut(&str, Option<f64>) + Send) -> Result<PathBuf> {
        let _guard = self.lock.lock().await;
        if let Some(exe) = self.installed(major) {
            return Ok(exe);
        }

        progress(&format!("Finding Java {major}"), None);
        let asset = self.find_asset(major).await?;

        let archive = self.paths.cache().join(&asset.binary.package.name);
        http::download(
            &self.client,
            &asset.binary.package.link,
            &archive,
            &Checksum::Sha256(asset.binary.package.checksum.clone()),
            |f| progress(&format!("Downloading Java {major}"), f),
        )
        .await?;

        progress(&format!("Unpacking Java {major}"), None);
        let dir = self.dir(major);
        let staging = dir.with_extension("staging");
        let archive2 = archive.clone();
        let staging2 = staging.clone();
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
        tokio::fs::write(dir.join(MARKER), &asset.release_name).await.at(&dir)?;
        let _ = tokio::fs::remove_file(&archive).await;

        let exe = java_exe(&dir);
        if !exe.exists() {
            return Err(Error::BadResponse {
                source_name: "Adoptium".into(),
                detail: format!("Java {major} archive did not contain {}", exe.display()),
            });
        }
        Ok(exe)
    }

    async fn find_asset(&self, major: u32) -> Result<Asset> {
        // Prefer a JRE (smaller); some old feature versions only ship a JDK.
        for image in ["jre", "jdk"] {
            let url = format!(
                "{ADOPTIUM}/assets/latest/{major}/hotspot?os={}&architecture={}&image_type={image}&vendor=eclipse",
                adoptium_os(),
                adoptium_arch()
            );
            let assets: Vec<Asset> = http::get_json(&self.client, &url).await?;
            if let Some(a) = assets.into_iter().next() {
                return Ok(a);
            }
        }
        Err(Error::BadResponse {
            source_name: "Adoptium".into(),
            detail: format!("no Java {major} build for this computer"),
        })
    }
}

fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        extract_zip(archive, dest, &ExtractOptions { strip_single_root: true, ..Default::default() })
    } else if name.ends_with(".tar.gz") {
        let f = std::fs::File::open(archive).at(archive)?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
        std::fs::create_dir_all(dest).at(dest)?;
        for entry in tar.entries().at(archive)? {
            let mut entry = entry.at(archive)?;
            let path = entry.path().at(archive)?.into_owned();
            // Strip the top-level "jdk-21.0.5+11-jre/" folder.
            let rel: PathBuf = path.components().skip(1).collect();
            if rel.as_os_str().is_empty() {
                continue;
            }
            if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
                return Err(Error::Archive(format!("unsafe path in archive: {}", path.display())));
            }
            entry.unpack(dest.join(rel)).at(archive)?;
        }
        Ok(())
    } else {
        Err(Error::Archive(format!("unknown archive type: {name}")))
    }
}

pub fn java_exe(runtime_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        runtime_dir.join("bin").join("java.exe")
    } else {
        runtime_dir.join("bin").join("java")
    }
}

fn adoptium_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

fn adoptium_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adoptium_assets() {
        let json = include_str!("../tests/fixtures/adoptium_25_jre.json");
        let assets: Vec<Asset> = serde_json::from_str(json).unwrap();
        let p = &assets[0].binary.package;
        assert!(p.link.starts_with("https://"));
        assert_eq!(p.checksum.len(), 64);
        assert!(p.name.ends_with(".zip"));
    }
}
