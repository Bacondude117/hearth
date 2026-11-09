//! SteamCMD: Valve's command-line client, used to install and update Steam dedicated servers
//! (anonymous login; no Steam account needed for free dedicated-server apps).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

use crate::archive::{extract_zip, ExtractOptions};
use crate::error::{Error, IoContext, Result};
use crate::http::{self, Checksum};
use crate::paths::AppPaths;

const DOWNLOAD_WINDOWS: &str = "https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip";
const DOWNLOAD_LINUX: &str = "https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz";
/// SteamCMD updates itself on first run and sometimes exits non-zero while doing so.
const ATTEMPTS: usize = 3;

pub struct SteamCmd {
    paths: AppPaths,
    client: reqwest::Client,
    /// SteamCMD can't run two jobs against its own install at once.
    lock: Mutex<()>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SteamEvent {
    /// "downloading", "verifying", "preallocating", "committing" + percent (0..=100).
    Progress { state: String, percent: f64 },
    Success,
    Error(String),
}

/// Parses one line of SteamCMD output.
pub fn parse_line(line: &str) -> Option<SteamEvent> {
    let line = line.trim();
    if line.starts_with("Success! App '") && line.contains("fully installed") {
        return Some(SteamEvent::Success);
    }
    if let Some(rest) = line.strip_prefix("Error! ") {
        return Some(SteamEvent::Error(rest.to_owned()));
    }
    if line.starts_with("ERROR! ") || line.starts_with("FAILED") {
        return Some(SteamEvent::Error(line.to_owned()));
    }
    // " Update state (0x61) downloading, progress: 45.23 (1234 / 5678)"
    let rest = line.strip_prefix("Update state (")?;
    let (_, rest) = rest.split_once(") ")?;
    let (state, rest) = rest.split_once(", progress: ")?;
    let percent: f64 = rest.split_whitespace().next()?.parse().ok()?;
    Some(SteamEvent::Progress { state: state.trim().to_owned(), percent })
}

/// Reads `"buildid"` from `steamapps/appmanifest_<app>.acf` after an install.
pub fn installed_build(dir: &Path, app_id: u32) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(format!("steamapps/appmanifest_{app_id}.acf"))).ok()?;
    text.lines().find_map(|l| {
        let mut parts = l.split('"').filter(|p| !p.trim().is_empty());
        (parts.next()? == "buildid").then(|| parts.next().map(str::to_owned)).flatten()
    })
}

fn stage_label(state: &str) -> &'static str {
    match state {
        "downloading" => "Downloading server",
        "verifying" | "verifying install" | "verifying update" => "Verifying files",
        "preallocating" => "Preparing disk space",
        "committing" => "Installing files",
        _ => "Updating server",
    }
}

impl SteamCmd {
    pub fn new(paths: AppPaths, client: reqwest::Client) -> Self {
        Self { paths, client, lock: Mutex::new(()) }
    }

    fn dir(&self) -> PathBuf {
        self.paths.tools().join("steamcmd")
    }

    fn exe(&self) -> PathBuf {
        if cfg!(windows) {
            self.dir().join("steamcmd.exe")
        } else {
            self.dir().join("steamcmd.sh")
        }
    }

    async fn ensure(&self, progress: &(dyn Fn(&str, Option<f64>) + Send + Sync)) -> Result<PathBuf> {
        let exe = self.exe();
        if exe.exists() {
            return Ok(exe);
        }
        let dir = self.dir();
        let (url, name) = if cfg!(windows) {
            (DOWNLOAD_WINDOWS, "steamcmd.zip")
        } else {
            (DOWNLOAD_LINUX, "steamcmd_linux.tar.gz")
        };
        let archive = self.paths.cache().join(name);
        // Valve publishes no checksum for the bootstrapper; HTTPS only.
        http::download(&self.client, url, &archive, &Checksum::None, |f| progress("Downloading SteamCMD", f)).await?;
        let (a, d) = (archive.clone(), dir.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            if a.to_string_lossy().ends_with(".zip") {
                extract_zip(&a, &d, &ExtractOptions::default())
            } else {
                std::fs::create_dir_all(&d).at(&d)?;
                let f = std::fs::File::open(&a).at(&a)?;
                tar::Archive::new(flate2::read::GzDecoder::new(f)).unpack(&d).at(&d)
            }
        })
        .await
        .expect("extract task panicked")?;
        let _ = tokio::fs::remove_file(&archive).await;
        if !exe.exists() {
            return Err(Error::BadResponse { source_name: "Steam".into(), detail: "SteamCMD archive was missing steamcmd".into() });
        }
        Ok(exe)
    }

    /// Installs or updates `app_id` into `dir` (anonymous login), reporting progress.
    pub async fn app_update(
        &self,
        app_id: u32,
        dir: &Path,
        progress: &(dyn Fn(&str, Option<f64>) + Send + Sync),
    ) -> Result<()> {
        let _g = self.lock.lock().await;
        let exe = self.ensure(progress).await?;
        tokio::fs::create_dir_all(dir).await.at(dir)?;
        let mut last_error = String::new();
        for attempt in 1..=ATTEMPTS {
            progress(if attempt == 1 { "Starting SteamCMD" } else { "Retrying SteamCMD" }, None);
            match self.run_once(&exe, app_id, dir, progress).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!("steamcmd attempt {attempt} failed: {e}");
                    last_error = e.to_string();
                }
            }
        }
        Err(Error::InvalidInput(format!("SteamCMD couldn't install the server: {last_error}")))
    }

    async fn run_once(
        &self,
        exe: &Path,
        app_id: u32,
        dir: &Path,
        progress: &(dyn Fn(&str, Option<f64>) + Send + Sync),
    ) -> Result<()> {
        let mut cmd = tokio::process::Command::new(exe);
        cmd.arg("+force_install_dir")
            .arg(dir)
            .args(["+login", "anonymous", "+app_update", &app_id.to_string(), "validate", "+quit"])
            .current_dir(self.dir())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| Error::InvalidInput(format!("Could not start SteamCMD: {e}")))?;
        let stdout = child.stdout.take().expect("piped");
        // Drain stderr so a chatty SteamCMD can't block on a full pipe.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut l = BufReader::new(stderr).lines();
                while let Ok(Some(_)) = l.next_line().await {}
            });
        }
        let mut lines = BufReader::new(stdout).lines();
        let mut success = false;
        let mut error: Option<String> = None;
        let mut tail: Vec<String> = vec![];
        while let Ok(Some(line)) = lines.next_line().await {
            match parse_line(&line) {
                Some(SteamEvent::Progress { state, percent }) => progress(stage_label(&state), Some((percent / 100.0).clamp(0.0, 1.0))),
                Some(SteamEvent::Success) => success = true,
                Some(SteamEvent::Error(e)) => error = Some(e),
                None => {}
            }
            if !line.trim().is_empty() {
                if tail.len() >= 8 {
                    tail.remove(0);
                }
                tail.push(line);
            }
        }
        let status = child.wait().await?;
        if success {
            return Ok(());
        }
        Err(Error::InvalidInput(match error {
            Some(e) => e,
            None => format!(
                "SteamCMD exited (code {}) without finishing. Last output:\n{}",
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                tail.join("\n")
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steamcmd_output() {
        assert_eq!(
            parse_line(" Update state (0x61) downloading, progress: 45.23 (1462829312 / 3234059869)"),
            Some(SteamEvent::Progress { state: "downloading".into(), percent: 45.23 })
        );
        assert_eq!(
            parse_line(" Update state (0x81) verifying update, progress: 3.10 (100 / 3234059869)"),
            Some(SteamEvent::Progress { state: "verifying update".into(), percent: 3.10 })
        );
        assert_eq!(parse_line("Success! App '2394010' fully installed."), Some(SteamEvent::Success));
        assert_eq!(
            parse_line("Error! App '2394010' state is 0x202 after update job."),
            Some(SteamEvent::Error("App '2394010' state is 0x202 after update job.".into()))
        );
        assert_eq!(parse_line("Loading Steam API...OK"), None);
        assert_eq!(parse_line("[  0%] Checking for available updates..."), None);
    }

    #[test]
    fn reads_build_id_from_app_manifest() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("steamapps")).unwrap();
        std::fs::write(
            t.path().join("steamapps/appmanifest_2394010.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"2394010\"\n\t\"buildid\"\t\t\"20481234\"\n\t\"name\"\t\t\"Palworld Dedicated Server\"\n}\n",
        )
        .unwrap();
        assert_eq!(installed_build(t.path(), 2394010).as_deref(), Some("20481234"));
        assert_eq!(installed_build(t.path(), 1), None);
    }
}
