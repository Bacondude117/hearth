//! playit.gg integration: link an account, run the official `playitd` agent as a child
//! process, and expose servers through tunnels.

pub mod api;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{Error, IoContext, Result};
use crate::events::{Event, EventSink, LogStream};
use crate::games::{LaunchSpec, LineEvent};
use crate::http::{self, Checksum};
use crate::paths::AppPaths;
use crate::process::{Console, ServerProcess, ServerStatus};
use api::{ClaimSetup, PlayitApi, RunData};

/// Console id used for the agent's log stream. Instance ids always end in `-xxxx`, so this
/// can never collide with a server.
pub const CONSOLE_ID: &str = "playit";

const CLAIM_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const RUNDATA_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayitStatus {
    pub supported: bool,
    pub agent_installed: bool,
    pub linked: bool,
    pub agent: ServerStatus,
    pub claim: Option<ClaimInfo>,
    pub agent_id: Option<String>,
    pub tunnels: Vec<TunnelInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimInfo {
    pub url: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    pub local_port: Option<u16>,
    pub protocol: String,
    pub tunnel_type: Option<String>,
    pub disabled_reason: Option<String>,
}

struct Claim {
    code: String,
    state: String,
    cancel: tokio::sync::watch::Sender<bool>,
}

pub struct Playit {
    paths: AppPaths,
    client: reqwest::Client,
    sink: Arc<dyn EventSink>,
    console: Arc<Console>,
    daemon: tokio::sync::Mutex<Option<Arc<ServerProcess>>>,
    claim: Mutex<Option<Claim>>,
    rundata: Mutex<Option<(Instant, RunData)>>,
    last_error: Mutex<Option<String>>,
}

impl Playit {
    pub fn new(paths: AppPaths, client: reqwest::Client, sink: Arc<dyn EventSink>) -> Arc<Self> {
        let console = Arc::new(Console::new(CONSOLE_ID.into(), sink.clone()));
        Arc::new(Self {
            paths,
            client,
            sink,
            console,
            daemon: tokio::sync::Mutex::new(None),
            claim: Mutex::new(None),
            rundata: Mutex::new(None),
            last_error: Mutex::new(None),
        })
    }

    pub fn console(&self) -> &Arc<Console> {
        &self.console
    }

    fn secret_path(&self) -> PathBuf {
        self.paths.playit().join("playit.toml")
    }

    fn agent_path(&self) -> PathBuf {
        if let Ok(p) = std::env::var("HEARTH_PLAYITD") {
            return PathBuf::from(p);
        }
        let name = if cfg!(windows) { "playitd.exe" } else { "playitd" };
        self.paths.tools().join(name)
    }

    fn agent_download_url() -> Option<&'static str> {
        if cfg!(all(windows, target_arch = "x86_64")) {
            Some("https://github.com/playit-cloud/playit-agent/releases/latest/download/playitd-windows-x86_64.exe")
        } else if cfg!(all(windows, target_arch = "x86")) {
            Some("https://github.com/playit-cloud/playit-agent/releases/latest/download/playitd-windows-x86.exe")
        } else {
            None
        }
    }

    pub fn supported() -> bool {
        Self::agent_download_url().is_some() || std::env::var_os("HEARTH_PLAYITD").is_some()
    }

    fn read_secret(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.secret_path()).ok()?;
        parse_secret(&text)
    }

    pub fn is_linked(&self) -> bool {
        self.read_secret().is_some()
    }

    fn changed(&self) {
        self.sink.emit(Event::PlayitChanged);
    }

    fn set_error(&self, e: Option<String>) {
        *self.last_error.lock().unwrap() = e;
        self.changed();
    }

    // ---- linking --------------------------------------------------------------------------

    /// Starts the account-linking flow and returns the URL the user must open.
    pub fn start_claim(self: &Arc<Self>) -> Result<String> {
        let code = hex::encode(rand::random::<[u8; 5]>());
        let url = format!("https://playit.gg/claim/{code}");
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        if let Some(old) = self.claim.lock().unwrap().replace(Claim {
            code: code.clone(),
            state: "Waiting for you to open the link".into(),
            cancel: cancel_tx,
        }) {
            let _ = old.cancel.send(true);
        }
        self.set_error(None);

        let me = self.clone();
        tokio::spawn(async move {
            let result = me.run_claim(&code, cancel_rx).await;
            {
                let mut claim = me.claim.lock().unwrap();
                // Only clear the claim if it's still ours (not replaced by a newer attempt).
                if claim.as_ref().is_some_and(|c| c.code == code) {
                    *claim = None;
                }
            }
            match result {
                Ok(true) => {
                    me.set_error(None);
                    if let Err(e) = me.start().await {
                        me.set_error(Some(e.to_string()));
                    }
                }
                Ok(false) => me.changed(),
                Err(e) => me.set_error(Some(e.to_string())),
            }
        });
        self.changed();
        Ok(url)
    }

    /// Returns Ok(true) when linked, Ok(false) when cancelled.
    async fn run_claim(&self, code: &str, mut cancel: tokio::sync::watch::Receiver<bool>) -> Result<bool> {
        let api = PlayitApi::new(self.client.clone(), None);
        let deadline = Instant::now() + CLAIM_TIMEOUT;
        loop {
            if *cancel.borrow() {
                return Ok(false);
            }
            if Instant::now() > deadline {
                return Err(Error::Playit("linking timed out; please try again".into()));
            }
            let state = match api.claim_setup(code).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("claim setup: {e}");
                    ClaimSetup::WaitingForUserVisit
                }
            };
            let text = match state {
                ClaimSetup::WaitingForUserVisit => "Waiting for you to open the link",
                ClaimSetup::WaitingForUser => "Approve Hearth on the playit.gg page",
                ClaimSetup::UserRejected => return Err(Error::Playit("linking was rejected on playit.gg".into())),
                ClaimSetup::UserAccepted => {
                    let secret = api.claim_exchange(code).await?;
                    let dir = self.paths.playit();
                    tokio::fs::create_dir_all(&dir).await.at(&dir)?;
                    let path = self.secret_path();
                    tokio::fs::write(&path, format!("secret_key = \"{}\"\n", secret.trim()))
                        .await
                        .at(&path)?;
                    return Ok(true);
                }
            };
            if let Some(c) = self.claim.lock().unwrap().as_mut() {
                if c.state != text {
                    c.state = text.into();
                    self.changed();
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = cancel.changed() => {}
            }
        }
    }

    pub fn cancel_claim(&self) {
        if let Some(c) = self.claim.lock().unwrap().take() {
            let _ = c.cancel.send(true);
        }
        self.changed();
    }

    /// Stops the agent and forgets the account link. Tunnels stay in the playit account.
    pub async fn unlink(&self) -> Result<()> {
        self.stop().await;
        let path = self.secret_path();
        if path.exists() {
            tokio::fs::remove_file(&path).await.at(&path)?;
        }
        *self.rundata.lock().unwrap() = None;
        self.changed();
        Ok(())
    }

    // ---- agent process ----------------------------------------------------------------------

    async fn ensure_agent(&self) -> Result<PathBuf> {
        let path = self.agent_path();
        if path.exists() {
            return Ok(path);
        }
        let url = Self::agent_download_url().ok_or_else(|| Error::Unsupported("The playit agent".into()))?;
        self.console.push(LogStream::System, "Downloading the playit agent...");
        // playit does not publish checksums for the standalone daemon; HTTPS + GitHub only.
        http::download(&self.client, url, &path, &Checksum::None, |_| {}).await?;
        Ok(path)
    }

    pub async fn start(&self) -> Result<()> {
        let mut daemon = self.daemon.lock().await;
        if daemon.as_ref().is_some_and(|d| !d.has_exited()) {
            return Ok(());
        }
        if !self.is_linked() {
            return Err(Error::Playit("link your playit.gg account first".into()));
        }
        let exe = self.ensure_agent().await?;
        let socket = if cfg!(windows) {
            r"\\.\pipe\hearth-playitd".to_owned()
        } else {
            self.paths.playit().join("playitd.sock").to_string_lossy().into_owned()
        };
        let spec = LaunchSpec {
            program: exe,
            args: vec![
                "--secret-path".into(),
                self.secret_path().to_string_lossy().into_owned(),
                "--socket-path".into(),
                socket,
                "--log-path".into(),
                self.paths.playit().join("playitd.log").to_string_lossy().into_owned(),
            ],
            cwd: self.paths.playit(),
            env: vec![],
            console_encoding: Default::default(),
            memory_limit_mb: None,
            log_file: None,
        };
        // The agent is "ready" as soon as it runs; there is no banner to wait for.
        let parser: crate::process::LineParser = Arc::new(|_: &str| None::<LineEvent>);
        let proc = ServerProcess::spawn(
            CONSOLE_ID.into(),
            &spec,
            parser,
            "",
            self.console.clone(),
            Arc::new(Relay(self.sink.clone())),
        )?;
        // The agent prints no "ready" banner; a live process counts as running.
        proc.force_running();
        *daemon = Some(proc);
        drop(daemon);
        self.changed();
        Ok(())
    }

    pub async fn stop(&self) {
        let d = self.daemon.lock().await.take();
        if let Some(d) = d {
            d.kill();
            d.wait().await;
        }
        self.changed();
    }

    pub async fn agent_status(&self) -> ServerStatus {
        match self.daemon.lock().await.as_ref() {
            Some(d) => d.status(),
            None => ServerStatus::Stopped,
        }
    }

    // ---- tunnels ----------------------------------------------------------------------------

    fn api(&self) -> Result<PlayitApi> {
        let secret = self
            .read_secret()
            .ok_or_else(|| Error::Playit("link your playit.gg account first".into()))?;
        Ok(PlayitApi::new(self.client.clone(), Some(secret)))
    }

    pub async fn rundata(&self, force: bool) -> Result<RunData> {
        if !force {
            if let Some((at, data)) = self.rundata.lock().unwrap().as_ref() {
                if at.elapsed() < RUNDATA_TTL {
                    return Ok(data.clone());
                }
            }
        }
        let data = self.api()?.rundata().await?;
        *self.rundata.lock().unwrap() = Some((Instant::now(), data.clone()));
        Ok(data)
    }

    pub async fn status(&self) -> PlayitStatus {
        let linked = self.is_linked();
        let mut error = self.last_error.lock().unwrap().clone();
        let (agent_id, tunnels) = if linked {
            match self.rundata(false).await {
                Ok(r) => (Some(r.agent_id.clone()), r.tunnels.iter().map(tunnel_info).collect()),
                Err(e) => {
                    error.get_or_insert_with(|| e.to_string());
                    (None, vec![])
                }
            }
        } else {
            (None, vec![])
        };
        let claim = self.claim.lock().unwrap().as_ref().map(|c| ClaimInfo {
            url: format!("https://playit.gg/claim/{}", c.code),
            state: c.state.clone(),
        });
        PlayitStatus {
            supported: Self::supported(),
            agent_installed: self.agent_path().exists(),
            linked,
            agent: self.agent_status().await,
            claim,
            agent_id,
            tunnels,
            error,
        }
    }

    /// The tunnel pointing at `local_port` with the right protocol, if one exists.
    pub async fn tunnel_for_port(&self, local_port: u16, protocol: &str) -> Result<Option<TunnelInfo>> {
        let data = self.rundata(false).await?;
        Ok(data
            .tunnels
            .iter()
            .map(tunnel_info)
            .find(|t| t.local_port == Some(local_port) && (t.protocol == protocol || t.protocol == "both")))
    }

    /// `tunnel_type` is playit's game preset (e.g. `minecraft-java`), or `None` for a plain
    /// TCP/UDP tunnel.
    pub async fn create_tunnel(
        &self,
        tunnel_type: Option<&str>,
        port_type: &str,
        local_port: u16,
        name: &str,
    ) -> Result<String> {
        let data = self.rundata(true).await?;
        let id = self
            .api()?
            .create_tunnel(&data.agent_id, tunnel_type, port_type, local_port, name)
            .await?;
        *self.rundata.lock().unwrap() = None;
        self.changed();
        Ok(id)
    }

    pub async fn delete_tunnel(&self, tunnel_id: &str) -> Result<()> {
        self.api()?.delete_tunnel(tunnel_id).await?;
        *self.rundata.lock().unwrap() = None;
        self.changed();
        Ok(())
    }
}

fn tunnel_info(t: &api::RunTunnel) -> TunnelInfo {
    TunnelInfo {
        id: t.id.clone(),
        name: t.name.clone(),
        address: t.display_address.clone(),
        local_port: t.local_port(),
        protocol: t.port_type.clone(),
        tunnel_type: t.tunnel_type.clone(),
        disabled_reason: t.disabled_reason.clone(),
    }
}

/// Agent status changes become `PlayitChanged`. Its log lines arrive through the console as
/// `ServerLog` events with `instanceId == "playit"`.
struct Relay(Arc<dyn EventSink>);

impl EventSink for Relay {
    fn emit(&self, event: Event) {
        if let Event::ServerStatus { .. } = event {
            self.0.emit(Event::PlayitChanged)
        }
    }
}

/// Accepts both a bare key and `secret_key = "..."` (playit's own format).
pub(crate) fn parse_secret(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    for line in t.lines() {
        if let Some(rest) = line.trim().strip_prefix("secret_key") {
            let v = rest.trim_start().strip_prefix('=')?.trim().trim_matches('"').trim();
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
    (!t.contains(char::is_whitespace)).then(|| t.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_formats() {
        assert_eq!(parse_secret("secret_key = \"abc123\"\n"), Some("abc123".into()));
        assert_eq!(parse_secret("abc123\n"), Some("abc123".into()));
        assert_eq!(parse_secret("secret_key = \"\""), None);
        assert_eq!(parse_secret("  "), None);
    }
}
