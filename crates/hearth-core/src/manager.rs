//! `ServerManager` is the one entry point the UI talks to.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::content::{self, ContentEntry, ContentFolder};
use crate::error::{Error, Result};
use crate::events::{Event, EventSink, LogLine, LogStream};
use crate::games::mc_voicechat;
use crate::games::{GameContext, GameInfo, GameProvider, GameVersion, Registry};
use crate::instance::PackSource;
use crate::instance::{InstallState, Instance, InstanceStore};
use crate::net::{self, Protocol};
use crate::packs::PackInfo;
use crate::paths::AppPaths;
use crate::players::{PlayerAction, PlayerBook, PlayersView};
use crate::playit::{Playit, TunnelInfo};
use crate::process::{Console, ServerProcess, ServerStatus};

pub const STOP_TIMEOUT: Duration = Duration::from_secs(45);
pub const MIN_MEMORY_MB: u32 = 512;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateServer {
    pub name: String,
    pub game: String,
    pub flavor: String,
    pub version: String,
    pub port: Option<u16>,
    pub memory_mb: Option<u32>,
    pub accept_eula: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSummary {
    #[serde(flatten)]
    pub instance: Instance,
    pub status: ServerStatus,
    pub players: Vec<String>,
    pub protocol: Protocol,
    pub game_name: String,
    /// The server's mods/plugins folder, if it has one.
    pub content: Option<ContentFolder>,
    /// Changes when the custom icon changes; `None` means the game's default icon.
    pub icon_version: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPack {
    pub file: PathBuf,
    pub name: String,
    pub port: Option<u16>,
    pub memory_mb: Option<u32>,
    pub accept_eula: bool,
    /// Overrides for what inspection couldn't detect (or got wrong).
    pub minecraft: Option<String>,
    pub loader: Option<String>,
}

struct Slot {
    console: Arc<Console>,
    process: Option<Arc<ServerProcess>>,
}

pub struct ServerManager {
    paths: AppPaths,
    store: InstanceStore,
    registry: Registry,
    ctx: Arc<GameContext>,
    sink: Arc<dyn EventSink>,
    slots: Mutex<HashMap<String, Slot>>,
    installing: Mutex<HashSet<String>>,
    /// Serializes start/stop per manager so double-clicks can't launch twice.
    lifecycle: tokio::sync::Mutex<()>,
    /// Serializes tunnel create/delete so a double-click can't open two tunnels for one server.
    tunnels: tokio::sync::Mutex<()>,
    /// Tunnels created moments ago, per server. playit's run data can take a little while to
    /// list a new tunnel; until it does, sharing again must not create a second one.
    fresh_tunnels: Mutex<HashMap<String, std::time::Instant>>,
    pub playit: Arc<Playit>,
}

impl ServerManager {
    pub fn new(root: impl Into<PathBuf>, sink: Arc<dyn EventSink>) -> Arc<Self> {
        let paths = AppPaths::new(root);
        let client = crate::http::client();
        let ctx = Arc::new(GameContext::new(paths.clone(), client.clone()));
        let playit = Playit::new(paths.clone(), client, sink.clone());
        let mgr = Arc::new(Self {
            store: InstanceStore::new(paths.clone()),
            paths,
            registry: Registry::builtin(),
            ctx,
            sink,
            slots: Mutex::new(HashMap::new()),
            installing: Mutex::new(HashSet::new()),
            lifecycle: tokio::sync::Mutex::new(()),
            tunnels: tokio::sync::Mutex::new(()),
            fresh_tunnels: Mutex::new(HashMap::new()),
            playit,
        });
        mgr.recover_interrupted_installs();
        mgr
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    /// An install that was running when the app closed can't be resumed; mark it failed so
    /// the user gets a Retry button.
    fn recover_interrupted_installs(&self) {
        if let Ok(list) = self.store.list() {
            for mut inst in list {
                if matches!(inst.install_state, InstallState::Installing | InstallState::Pending) {
                    inst.install_state = InstallState::Failed {
                        message: "Installation was interrupted. Click Retry.".into(),
                    };
                    let _ = self.store.save(&inst);
                }
            }
        }
    }

    fn changed(&self) {
        self.sink.emit(Event::InstancesChanged);
    }

    fn console(&self, id: &str) -> Arc<Console> {
        self.slots
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_insert_with(|| Slot {
                console: Arc::new(Console::new(id.to_owned(), self.sink.clone())),
                process: None,
            })
            .console
            .clone()
    }

    fn process(&self, id: &str) -> Option<Arc<ServerProcess>> {
        self.slots.lock().unwrap().get(id).and_then(|s| s.process.clone())
    }

    fn status_of(&self, id: &str) -> ServerStatus {
        self.process(id).map(|p| p.status()).unwrap_or(ServerStatus::Stopped)
    }

    fn is_active(&self, id: &str) -> bool {
        self.status_of(id).is_active()
    }

    fn summary(&self, inst: Instance) -> ServerSummary {
        let proc = self.process(&inst.id);
        let (protocol, game_name, content) = match self.registry.get(&inst.game) {
            Ok(p) => {
                let i = p.info();
                (i.protocol, i.name.to_owned(), p.content_folder(&inst))
            }
            Err(_) => (Protocol::Tcp, inst.game.clone(), None),
        };
        ServerSummary {
            status: proc.as_ref().map(|p| p.status()).unwrap_or(ServerStatus::Stopped),
            players: proc.as_ref().map(|p| p.players()).unwrap_or_default(),
            protocol,
            game_name,
            content,
            icon_version: crate::icons::icon_version(&self.paths.instance_dir(&inst.id)),
            instance: inst,
        }
    }

    // ---- catalog ----------------------------------------------------------------------------

    pub fn games(&self) -> Vec<GameInfo> {
        self.registry.infos()
    }

    pub async fn versions(&self, game: &str, flavor: &str) -> Result<Vec<GameVersion>> {
        self.registry.get(game)?.versions(&self.ctx, flavor).await
    }

    /// Ports claimed by existing servers (including Bedrock's extra IPv6 port).
    fn reserved_ports(&self, except: Option<&str>) -> Vec<u16> {
        self.store
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| Some(i.id.as_str()) != except)
            .flat_map(|i| {
                let mut ports: Vec<u16> = match self.registry.get(&i.game) {
                    Ok(p) => p.ports_used(i.port).into_iter().map(|(n, _)| n).collect(),
                    Err(_) => vec![i.port],
                };
                ports.extend(i.voice.as_ref().map(|v| v.port));
                ports
            })
            .collect()
    }

    pub fn suggest_port(&self, game: &str) -> Result<u16> {
        let p = self.registry.get(game)?;
        let info = p.info();
        let reserved = self.reserved_ports(None);
        let mut port = info.default_port;
        loop {
            let used = p.ports_used(port);
            if used
                .iter()
                .all(|(u, proto)| !reserved.contains(u) && net::port_is_free(*u, *proto))
            {
                return Ok(port);
            }
            port = port
                .checked_add(1)
                .ok_or_else(|| Error::Conflict("No free port available".into()))?;
        }
    }

    // ---- servers ----------------------------------------------------------------------------

    pub fn list(&self) -> Result<Vec<ServerSummary>> {
        Ok(self.store.list()?.into_iter().map(|i| self.summary(i)).collect())
    }

    pub fn get(&self, id: &str) -> Result<ServerSummary> {
        Ok(self.summary(self.store.get(id)?))
    }

    pub fn server_dir(&self, id: &str) -> Result<PathBuf> {
        self.store.get(id)?;
        Ok(self.paths.server_dir(id))
    }

    pub fn create(self: &Arc<Self>, req: CreateServer) -> Result<Instance> {
        let inst = self.create_instance(req)?;
        self.changed();
        self.spawn_install(inst.id.clone(), true)?;
        Ok(inst)
    }

    /// Validates and saves a new instance record (nothing is installed yet).
    fn create_instance(&self, req: CreateServer) -> Result<Instance> {
        let provider = self.registry.get(&req.game)?;
        let info = provider.info();
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Error::InvalidInput("Give your server a name.".into()));
        }
        if name.chars().count() > 48 {
            return Err(Error::InvalidInput("Server names can be at most 48 characters.".into()));
        }
        if !info.flavors.iter().any(|f| f.id == req.flavor) {
            return Err(Error::InvalidInput(format!("Unknown server type: {}", req.flavor)));
        }
        if req.version.trim().is_empty() {
            return Err(Error::InvalidInput("Pick a version.".into()));
        }
        if info.eula.is_some() && !req.accept_eula {
            return Err(Error::InvalidInput(
                "You need to accept the license agreement to continue.".into(),
            ));
        }
        let port = match req.port {
            Some(p) => {
                self.check_port_reserved(provider.as_ref(), p, None)?;
                p
            }
            None => self.suggest_port(&req.game)?,
        };
        let memory_mb = if info.configurable_memory {
            let m = req.memory_mb.unwrap_or(crate::games::minecraft_java::DEFAULT_MEMORY_MB);
            validate_memory(m)?;
            Some(m)
        } else {
            None
        };

        let inst = Instance {
            id: self.store.new_id(name),
            name: name.to_owned(),
            game: req.game,
            flavor: req.flavor,
            version: req.version.trim().to_owned(),
            installed_build: None,
            port,
            memory_mb,
            java_major: None,
            jvm_args: vec![],
            install_state: InstallState::Pending,
            created_at: Utc::now(),
            playit_tunnel_id: None,
            launch: None,
            pack: None,
            voice: None,
        };
        self.store.save(&inst)?;
        Ok(inst)
    }

    fn check_port_reserved(&self, provider: &dyn GameProvider, port: u16, except: Option<&str>) -> Result<()> {
        if port < 1024 {
            return Err(Error::InvalidInput("Pick a port between 1024 and 65535.".into()));
        }
        let reserved = self.reserved_ports(except);
        if let Some((p, _)) = provider
            .ports_used(port)
            .into_iter()
            .find(|(p, _)| reserved.contains(p))
        {
            return Err(Error::Conflict(format!(
                "Port {p} is already used by another server in Hearth."
            )));
        }
        Ok(())
    }

    /// Re-runs the installer: retry after failure, or update to `version`.
    pub fn reinstall(self: &Arc<Self>, id: &str, version: Option<String>) -> Result<()> {
        if self.is_active(id) {
            return Err(Error::Conflict("Stop the server before updating it.".into()));
        }
        let mut inst = self.store.get(id)?;
        if let Some(v) = version.filter(|v| !v.trim().is_empty() && v.trim() != inst.version) {
            if inst.pack.is_some() {
                return Err(Error::Conflict(
                    "This server came from a modpack, so its Minecraft version can't be changed (the mods would break).".into(),
                ));
            }
            inst.version = v.trim().to_owned();
        }
        // A server that never finished installing has nothing to preserve.
        let pack_pending = inst.pack.as_ref().and_then(|p| p.file.as_ref()).is_some();
        let fresh = pack_pending || !self.paths.server_dir(id).join(provider_marker(&inst)).exists();
        self.store.save(&inst)?;
        self.spawn_install(id.to_owned(), fresh)
    }

    fn spawn_install(self: &Arc<Self>, id: String, fresh: bool) -> Result<()> {
        if !self.installing.lock().unwrap().insert(id.clone()) {
            return Err(Error::Conflict("This server is already being installed.".into()));
        }
        let me = self.clone();
        tokio::spawn(async move {
            let result = me.run_install(&id, fresh).await;
            me.installing.lock().unwrap().remove(&id);
            let error = result.err().map(|e| e.to_string());
            if let Ok(mut inst) = me.store.get(&id) {
                inst.install_state = match &error {
                    None => InstallState::Installed,
                    Some(m) => InstallState::Failed { message: m.clone() },
                };
                let _ = me.store.save(&inst);
            }
            let console = me.console(&id);
            match &error {
                None => console.push(
                    LogStream::System,
                    "Installation complete. Press Start to launch the server.",
                ),
                Some(e) => console.push(LogStream::System, format!("Installation failed: {e}")),
            }
            me.sink.emit(Event::InstallFinished {
                instance_id: id.clone(),
                error,
            });
            me.changed();
        });
        Ok(())
    }

    async fn run_install(&self, id: &str, fresh: bool) -> Result<()> {
        let mut inst = self.store.get(id)?;
        let provider = self.registry.get(&inst.game)?;
        inst.install_state = InstallState::Installing;
        self.store.save(&inst)?;
        self.changed();

        let sink = self.sink.clone();
        let console = self.console(id);
        let last_stage = Mutex::new(String::new());
        let progress = |stage: &str, fraction: Option<f64>| {
            let mut last = last_stage.lock().unwrap();
            if *last != stage {
                *last = stage.to_owned();
                console.push(LogStream::System, format!("{stage}..."));
            }
            sink.emit(Event::InstallProgress {
                instance_id: id.to_owned(),
                stage: stage.to_owned(),
                fraction,
            });
        };
        let dir = self.paths.server_dir(id);
        let outcome = provider.install(&self.ctx, &inst, &dir, fresh, &progress).await?;

        // Re-read: the user may have renamed the server while it installed.
        let mut inst = self.store.get(id)?;
        if outcome.installed_build.is_some() {
            inst.installed_build = outcome.installed_build;
        }
        if outcome.java_major.is_some() {
            inst.java_major = outcome.java_major;
        }
        if let Some(args) = outcome.jvm_args {
            inst.jvm_args = args;
        }
        if outcome.launch.is_some() {
            inst.launch = outcome.launch;
        }
        // The pack's contents are in place; its copy is no longer needed for retries.
        if let Some(pack) = inst.pack.as_mut() {
            if let Some(f) = pack.file.take() {
                let _ = tokio::fs::remove_file(self.paths.instance_dir(id).join(f)).await;
            }
        }
        self.store.save(&inst)?;
        // A version change needs the matching voice chat build.
        if !fresh {
            if let Some(v) = inst.voice.clone() {
                match self.install_voice_jar(&inst).await {
                    Ok((file, version)) => {
                        inst.voice = Some(mc_voicechat::VoiceChat { file, version, ..v });
                        self.store.save(&inst)?;
                    }
                    Err(e) => self
                        .console(id)
                        .push(LogStream::System, format!("Couldn't update voice chat: {e}")),
                }
            }
        }
        Ok(())
    }

    pub async fn delete(&self, id: &str, remove_tunnel: bool) -> Result<()> {
        let _g = self.lifecycle.lock().await;
        if self.is_active(id) {
            return Err(Error::Conflict("Stop the server before deleting it.".into()));
        }
        if self.installing.lock().unwrap().contains(id) {
            return Err(Error::Conflict("Wait for the installation to finish first.".into()));
        }
        let inst = self.store.get(id)?;
        if remove_tunnel {
            if let Some(t) = &inst.playit_tunnel_id {
                if let Err(e) = self.playit.delete_tunnel(t).await {
                    tracing::warn!("could not delete tunnel {t}: {e}");
                }
            }
            self.remove_voice_tunnel(&inst).await;
        }
        self.store.delete(id)?;
        self.slots.lock().unwrap().remove(id);
        self.changed();
        Ok(())
    }

    pub async fn start(&self, id: &str) -> Result<()> {
        let _g = self.lifecycle.lock().await;
        if self.is_active(id) {
            return Ok(());
        }
        let inst = self.store.get(id)?;
        if inst.install_state != InstallState::Installed {
            return Err(Error::Conflict("This server isn't installed yet.".into()));
        }
        let provider = self.registry.get(&inst.game)?;
        for (p, protocol) in provider.ports_used(inst.port) {
            if !net::port_is_free(p, protocol) {
                return Err(Error::Conflict(format!(
                    "Port {p} is already in use by another program. Close it or change this server's port in Settings."
                )));
            }
        }
        if let Some(v) = &inst.voice {
            if !net::port_is_free(v.port, Protocol::Udp) {
                return Err(Error::Conflict(format!(
                    "Voice chat port {} is already in use by another program.",
                    v.port
                )));
            }
            self.sync_voice_host(&inst).await;
        }
        let dir = self.paths.server_dir(id);
        let spec = provider.launch(&self.ctx, &inst, &dir)?;
        let console = self.console(id);
        let parser_provider = provider.clone();
        let book = self.player_book(id);
        let parser: crate::process::LineParser = Arc::new(move |l: &str| {
            // Remember everyone who joins (and their account id) for the Players tab.
            if let Some((name, pid)) = parser_provider.parse_identity(l) {
                if let Err(e) = book.record(&name, pid.as_deref()) {
                    tracing::warn!("could not record player {name}: {e}");
                }
            }
            let ev = parser_provider.parse_line(l);
            if let Some(crate::games::LineEvent::PlayerJoined(name)) = &ev {
                let _ = book.record(name, None);
            }
            ev
        });
        let proc = ServerProcess::spawn(
            id.to_owned(),
            &spec,
            parser,
            provider.stop_command(),
            console,
            self.sink.clone(),
        )?;
        if let Some(slot) = self.slots.lock().unwrap().get_mut(id) {
            slot.process = Some(proc.clone());
        }
        self.spawn_monitor(provider, inst, dir, proc);
        Ok(())
    }

    /// For games reporting through an admin API: poll it for readiness and online players
    /// while the process lives. Stops immediately for games that don't poll.
    fn spawn_monitor(&self, provider: Arc<dyn GameProvider>, inst: Instance, dir: PathBuf, proc: Arc<ServerProcess>) {
        let ctx = self.ctx.clone();
        let book = self.player_book(&inst.id);
        tokio::spawn(async move {
            let mut ready = false;
            loop {
                tokio::time::sleep(Duration::from_secs(if ready { 10 } else { 3 })).await;
                if proc.has_exited() || proc.status() == ServerStatus::Stopping {
                    if proc.has_exited() {
                        break;
                    }
                    continue;
                }
                match provider.poll(&ctx, &inst, &dir).await {
                    None => break,
                    Some(Ok(players)) => {
                        if !ready {
                            ready = true;
                            proc.force_running();
                        }
                        for p in &players {
                            let _ = book.record(&p.name, p.id.as_deref());
                        }
                        proc.set_players(players.into_iter().map(|p| p.name).collect());
                    }
                    Some(Err(e)) => {
                        if ready {
                            tracing::debug!("poll failed: {e}");
                        }
                    }
                }
            }
        });
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        let _g = self.lifecycle.lock().await;
        if let Some(p) = self.process(id) {
            self.stop_process(id, &p).await;
        }
        Ok(())
    }

    /// Graceful stop: console `stop` command, or the game's admin API for console-less games.
    async fn stop_process(&self, id: &str, p: &Arc<ServerProcess>) {
        if p.has_exited() {
            return;
        }
        if let Ok(inst) = self.store.get(id) {
            if let Ok(provider) = self.registry.get(&inst.game) {
                if provider.stop_command().is_empty() {
                    self.console(id)
                        .push(LogStream::System, "Saving and stopping server...");
                    if !provider.remote_stop(&self.ctx, &inst, &self.paths.server_dir(id)).await {
                        self.console(id)
                            .push(LogStream::System, "The server didn't answer; forcing it to close.");
                        p.kill();
                    }
                }
            }
        }
        p.stop(STOP_TIMEOUT).await;
    }

    pub async fn restart(&self, id: &str) -> Result<()> {
        self.stop(id).await?;
        self.start(id).await
    }

    /// Immediate termination, for a server that is stuck.
    pub fn kill(&self, id: &str) -> Result<()> {
        if let Some(p) = self.process(id) {
            p.kill();
        }
        Ok(())
    }

    pub async fn command(&self, id: &str, line: &str) -> Result<()> {
        let p = self
            .process(id)
            .ok_or_else(|| Error::Conflict("The server is not running.".into()))?;
        let inst = self.store.get(id)?;
        let provider = self.registry.get(&inst.game)?;
        if let Some(r) = provider
            .console_input(&self.ctx, &inst, &self.paths.server_dir(id), line)
            .await
        {
            r?;
            self.console(id)
                .push(LogStream::System, format!("> (broadcast) {line}"));
            return Ok(());
        }
        p.send_command(line).await?;
        self.console(id).push(LogStream::System, format!("> {line}"));
        Ok(())
    }

    pub fn console_lines(&self, id: &str) -> Vec<LogLine> {
        self.console(id).snapshot()
    }

    /// Stops every running server (app exit).
    pub async fn shutdown(&self) {
        let procs: Vec<(String, Arc<ServerProcess>)> = self
            .slots
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, s)| s.process.clone().map(|p| (id.clone(), p)))
            .collect();
        futures_util::future::join_all(procs.iter().map(|(id, p)| self.stop_process(id, p))).await;
        self.playit.stop().await;
    }

    pub fn running_count(&self) -> usize {
        self.slots
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.process.as_ref().is_some_and(|p| p.status().is_active()))
            .count()
    }

    // ---- settings ---------------------------------------------------------------------------

    pub fn properties(&self, id: &str) -> Result<Vec<(String, String)>> {
        let inst = self.store.get(id)?;
        let provider = self.registry.get(&inst.game)?;
        provider.read_settings(&self.paths.server_dir(id))
    }

    /// Applies changed properties. Port changes are validated and mirrored into the instance.
    pub fn set_properties(&self, id: &str, changes: HashMap<String, String>) -> Result<()> {
        let mut inst = self.store.get(id)?;
        let provider = self.registry.get(&inst.game)?;
        let port_key = provider.port_setting_key();
        let mut rest: Vec<(String, String)> = vec![];
        let mut new_port = None;
        for (k, v) in &changes {
            if k == port_key {
                let p: u16 = v
                    .trim()
                    .parse()
                    .map_err(|_| Error::InvalidInput("The port must be a number.".into()))?;
                if p != inst.port {
                    self.check_port_reserved(provider.as_ref(), p, Some(id))?;
                    new_port = Some(p);
                }
                continue;
            }
            rest.push((k.clone(), v.clone()));
        }
        provider.write_settings(&self.paths.server_dir(id), &rest)?;
        if let Some(p) = new_port {
            provider.apply_port(&self.paths.server_dir(id), p)?;
            inst.port = p;
            self.store.save(&inst)?;
        }
        if self.is_active(id) {
            self.console(id)
                .push(LogStream::System, "Settings saved. Restart the server to apply them.");
        }
        self.changed();
        Ok(())
    }

    pub fn update_instance(&self, id: &str, name: Option<String>, memory_mb: Option<u32>) -> Result<Instance> {
        let mut inst = self.store.get(id)?;
        if let Some(n) = name {
            let n = n.trim();
            if n.is_empty() || n.chars().count() > 48 {
                return Err(Error::InvalidInput("Names must be 1-48 characters.".into()));
            }
            inst.name = n.to_owned();
        }
        if let Some(m) = memory_mb {
            validate_memory(m)?;
            inst.memory_mb = Some(m);
        }
        self.store.save(&inst)?;
        self.changed();
        Ok(inst)
    }

    // ---- modpacks -----------------------------------------------------------------------------

    /// Reads a pack file and reports what it contains, without creating anything.
    pub async fn inspect_pack(&self, file: &std::path::Path) -> Result<PackInfo> {
        if !file.is_file() {
            return Err(Error::InvalidInput("That file doesn't exist anymore.".into()));
        }
        for info in self.registry.infos() {
            if let Some(r) = self.registry.get(info.id)?.inspect_pack(&self.ctx, file).await {
                return r;
            }
        }
        Err(Error::InvalidInput(
            "Hearth can import Modrinth .mrpack files and server pack .zip files.".into(),
        ))
    }

    /// Creates a server from a modpack and starts installing it.
    pub async fn import_pack(self: &Arc<Self>, req: ImportPack) -> Result<Instance> {
        let info = self.inspect_pack(&req.file).await?;
        let flavor = req
            .loader
            .clone()
            .or(info.loader.clone())
            .ok_or_else(|| Error::InvalidInput("Choose the mod loader this pack uses.".into()))?;
        let minecraft = req
            .minecraft
            .clone()
            .filter(|v| !v.trim().is_empty())
            .or(info.minecraft.clone())
            .ok_or_else(|| Error::InvalidInput("Choose the Minecraft version this pack is for.".into()))?;
        // A pinned loader build only applies if the loader wasn't overridden.
        let loader_version = if req.loader.as_ref().is_none_or(|l| Some(l) == info.loader.as_ref()) {
            info.loader_version.clone()
        } else {
            None
        };

        let inst = self.create_instance(CreateServer {
            name: req.name,
            game: info.game.clone(),
            flavor,
            version: minecraft,
            port: req.port,
            memory_mb: req.memory_mb,
            accept_eula: req.accept_eula,
        })?;
        // Keep a copy of the pack so a failed install can be retried.
        let ext = req
            .file
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "zip".into());
        let stored = format!("pack.{ext}");
        let dest = self.paths.instance_dir(&inst.id).join(&stored);
        let copied = std::fs::copy(&req.file, &dest).map_err(|e| Error::Io {
            path: req.file.clone(),
            source: e,
        });
        if let Err(e) = copied {
            let _ = self.store.delete(&inst.id);
            return Err(e);
        }
        let mut inst = inst;
        inst.pack = Some(PackSource {
            name: info.name,
            version: info.pack_version,
            file: Some(stored),
            loader_version,
        });
        self.store.save(&inst)?;
        self.changed();
        self.spawn_install(inst.id.clone(), true)?;
        Ok(inst)
    }

    // ---- icons --------------------------------------------------------------------------------

    /// Uses the image at `file` as the server's icon (and the game's own icon file, if any).
    pub fn set_icon(&self, id: &str, file: &std::path::Path) -> Result<()> {
        let inst = self.store.get(id)?;
        let game_icon = self.registry.get(&inst.game)?.server_icon_file();
        crate::icons::set_icon(
            file,
            &self.paths.instance_dir(id),
            &self.paths.server_dir(id),
            game_icon,
        )?;
        if game_icon.is_some() && self.is_active(id) {
            self.console(id)
                .push(LogStream::System, "Icon saved. Restart the server so players see it.");
        }
        self.changed();
        Ok(())
    }

    pub fn clear_icon(&self, id: &str) -> Result<()> {
        let inst = self.store.get(id)?;
        let game_icon = self.registry.get(&inst.game)?.server_icon_file();
        crate::icons::clear_icon(&self.paths.instance_dir(id), &self.paths.server_dir(id), game_icon)?;
        self.changed();
        Ok(())
    }

    pub fn icon_data_url(&self, id: &str) -> Result<Option<String>> {
        self.store.get(id)?;
        crate::icons::icon_data_url(&self.paths.instance_dir(id))
    }

    // ---- mods / plugins -----------------------------------------------------------------------

    fn content_ctx(&self, id: &str) -> Result<(Arc<dyn GameProvider>, ContentFolder, PathBuf)> {
        let inst = self.store.get(id)?;
        let provider = self.registry.get(&inst.game)?;
        let folder = provider
            .content_folder(&inst)
            .ok_or_else(|| Error::InvalidInput("This server type doesn't use mods or plugins.".into()))?;
        Ok((provider, folder, self.paths.server_dir(id)))
    }

    pub fn list_content(&self, id: &str) -> Result<Vec<ContentEntry>> {
        let (provider, folder, dir) = self.content_ctx(id)?;
        content::list(&dir, &folder, |p| provider.describe_content(p))
    }

    pub fn add_content(&self, id: &str, files: &[PathBuf]) -> Result<Vec<String>> {
        let (_, folder, dir) = self.content_ctx(id)?;
        let added = content::add(&dir, &folder, files)?;
        self.note_restart(id, &folder);
        Ok(added)
    }

    pub fn set_content_enabled(&self, id: &str, file: &str, enabled: bool) -> Result<()> {
        let (_, folder, dir) = self.content_ctx(id)?;
        content::set_enabled(&dir, &folder, file, enabled)?;
        self.note_restart(id, &folder);
        Ok(())
    }

    pub fn delete_content(&self, id: &str, file: &str) -> Result<()> {
        let (_, folder, dir) = self.content_ctx(id)?;
        content::delete(&dir, &folder, file)?;
        self.note_restart(id, &folder);
        Ok(())
    }

    fn note_restart(&self, id: &str, folder: &ContentFolder) {
        if self.is_active(id) {
            self.console(id).push(
                LogStream::System,
                format!("{} changed. Restart the server to apply.", folder.label),
            );
        }
    }

    // ---- players ------------------------------------------------------------------------------

    fn player_book(&self, id: &str) -> PlayerBook {
        PlayerBook::new(self.paths.instance_dir(id).join("players.json"))
    }

    pub fn players(&self, id: &str) -> Result<PlayersView> {
        let inst = self.store.get(id)?;
        let provider = self.registry.get(&inst.game)?;
        let admin = provider
            .players()
            .ok_or_else(|| Error::InvalidInput("Player management isn't available for this game yet.".into()))?;
        let dir = self.paths.server_dir(id);
        let proc = self.process(id);
        let online = proc.as_ref().map(|p| p.players()).unwrap_or_default();
        Ok(PlayersView {
            caps: admin.caps(),
            whitelist_enabled: admin.whitelist_enabled(&dir),
            running: proc.is_some_and(|p| p.status() == ServerStatus::Running),
            players: admin.list(&dir, &online, &self.player_book(id).load())?,
        })
    }

    /// Changes a player's role, whitelist or ban state, or kicks / sets their game mode.
    /// Uses console commands while the server runs and edits its files while it's stopped.
    pub async fn player_action(&self, id: &str, name: &str, action: PlayerAction) -> Result<()> {
        let _g = self.lifecycle.lock().await;
        let inst = self.store.get(id)?;
        if inst.install_state != InstallState::Installed {
            return Err(Error::Conflict("This server isn't installed yet.".into()));
        }
        let provider = self.registry.get(&inst.game)?;
        let admin = provider
            .players()
            .ok_or_else(|| Error::InvalidInput("Player management isn't available for this game yet.".into()))?;
        let name = name.trim();
        let dir = self.paths.server_dir(id);
        let seen = self.player_book(id).load();

        match self.process(id).map(|p| (p.status(), p)) {
            Some((ServerStatus::Running, proc)) => {
                if let Some(r) = admin.apply_live(&dir, name, &seen, &action).await {
                    return r;
                }
                // Validate before touching any file.
                let commands = admin.commands(name, &action)?;
                if admin.live_needs_file_write(&action) {
                    admin.apply_offline(&self.ctx, &dir, name, &seen, &action).await?;
                }
                let console = self.console(id);
                for c in commands {
                    proc.send_command(&c).await?;
                    console.push(LogStream::System, format!("> {c}"));
                }
            }
            Some((ServerStatus::Starting | ServerStatus::Stopping, _)) => {
                return Err(Error::Conflict(
                    "Wait until the server has finished starting or stopping.".into(),
                ));
            }
            _ => {
                if action.needs_running() {
                    return Err(Error::Conflict(
                        "Start the server first: the player has to be online for that.".into(),
                    ));
                }
                admin.apply_offline(&self.ctx, &dir, name, &seen, &action).await?;
                if let PlayerAction::Whitelist { on: true } = action {
                    let _ = self.player_book(id).record(name, None);
                }
            }
        }
        Ok(())
    }

    // ---- sharing (playit) -------------------------------------------------------------------

    pub async fn share_info(&self, id: &str) -> Result<Option<TunnelInfo>> {
        let inst = self.store.get(id)?;
        if !self.playit.is_linked() {
            return Ok(None);
        }
        let proto = match self.registry.get(&inst.game)?.info().protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        };
        self.playit.tunnel_for_port(inst.port, proto).await
    }

    pub async fn share(&self, id: &str) -> Result<()> {
        let _guard = self.tunnels.lock().await;
        let mut inst = self.store.get(id)?;
        if self.share_info(id).await?.is_some() {
            return Ok(());
        }
        let just_created = self
            .fresh_tunnels
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(120));
        if just_created && inst.playit_tunnel_id.is_some() {
            return Ok(());
        }
        let info = self.registry.get(&inst.game)?.info();
        let tunnel_type = info.playit_tunnel_type;
        let port_type = match info.protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        };
        let tid = self
            .playit
            .create_tunnel(tunnel_type, port_type, inst.port, &format!("Hearth: {}", inst.name))
            .await?;
        self.playit.start().await?;
        inst.playit_tunnel_id = Some(tid);
        self.store.save(&inst)?;
        self.fresh_tunnels
            .lock()
            .unwrap()
            .insert(id.to_owned(), std::time::Instant::now());
        if inst.voice.is_some() {
            if let Err(e) = self.ensure_voice_tunnel(&inst).await {
                self.console(id)
                    .push(LogStream::System, format!("Couldn't create the voice chat tunnel: {e}"));
            }
        }
        self.changed();
        Ok(())
    }

    pub async fn unshare(&self, id: &str) -> Result<()> {
        let _guard = self.tunnels.lock().await;
        let mut inst = self.store.get(id)?;
        let tid = match self.share_info(id).await? {
            Some(t) => Some(t.id),
            None => inst.playit_tunnel_id.clone(),
        };
        if let Some(t) = tid {
            self.playit.delete_tunnel(&t).await?;
        }
        self.remove_voice_tunnel(&inst).await;
        self.fresh_tunnels.lock().unwrap().remove(id);
        inst.playit_tunnel_id = None;
        self.store.save(&inst)?;
        self.changed();
        Ok(())
    }

    // ---- voice chat (Simple Voice Chat, Minecraft Java) -----------------------------------------

    /// Installs Simple Voice Chat on a Paper/Fabric/Forge/NeoForge server with its own UDP port,
    /// and opens a playit tunnel for it if the server is shared.
    pub async fn enable_voice(&self, id: &str) -> Result<()> {
        let mut inst = self.store.get(id)?;
        if inst.voice.is_some() {
            return Ok(());
        }
        if inst.install_state != InstallState::Installed {
            return Err(Error::Conflict("Install the server first.".into()));
        }
        let port = crate::net::next_free_port(mc_voicechat::DEFAULT_PORT, Protocol::Udp, &self.reserved_ports(None))
            .ok_or_else(|| Error::Conflict("No free port for voice chat.".into()))?;
        let (file, version) = self.install_voice_jar(&inst).await?;
        mc_voicechat::write_config(&self.paths.server_dir(id), &inst.flavor, port, None)?;
        inst.voice = Some(mc_voicechat::VoiceChat { port, file, version });
        self.store.save(&inst)?;
        if self.playit.is_linked() && self.share_info(id).await.ok().flatten().is_some() {
            let _guard = self.tunnels.lock().await;
            if let Err(e) = self.ensure_voice_tunnel(&inst).await {
                self.console(id)
                    .push(LogStream::System, format!("Couldn't create the voice chat tunnel: {e}"));
            }
        }
        if self.is_active(id) {
            self.console(id).push(
                LogStream::System,
                "Voice chat installed. Restart the server to turn it on.",
            );
        }
        self.changed();
        Ok(())
    }

    pub async fn disable_voice(&self, id: &str) -> Result<()> {
        let mut inst = self.store.get(id)?;
        let Some(v) = inst.voice.take() else { return Ok(()) };
        if let Ok((_, folder, dir)) = self.content_ctx(id) {
            for name in [v.file.clone(), format!("{}{}", v.file, crate::content::DISABLED_SUFFIX)] {
                let _ = content::delete(&dir, &folder, &name);
            }
        }
        let with_voice = Instance {
            voice: Some(v),
            ..inst.clone()
        };
        self.remove_voice_tunnel(&with_voice).await;
        self.store.save(&inst)?;
        if self.is_active(id) {
            self.console(id)
                .push(LogStream::System, "Voice chat removed. Restart the server to apply.");
        }
        self.changed();
        Ok(())
    }

    /// Downloads the Simple Voice Chat build for the server's type and Minecraft version into
    /// its mods/plugins folder, replacing an older copy. Returns (file name, version).
    async fn install_voice_jar(&self, inst: &Instance) -> Result<(String, String)> {
        let loader = mc_voicechat::modrinth_loader(&inst.flavor)
            .ok_or_else(|| Error::InvalidInput("Voice chat needs a Paper, Fabric, Forge or NeoForge server.".into()))?;
        let (_, folder, dir) = self.content_ctx(&inst.id)?;
        let (url, sha512, file, version) = mc_voicechat::find_build(&self.ctx.client, loader, &inst.version).await?;
        crate::content::check_file_name(&file)?;
        if let Some(old) = &inst.voice {
            if old.file != file {
                let _ = content::delete(&dir, &folder, &old.file);
            }
        }
        crate::http::download(
            &self.ctx.client,
            &url,
            &dir.join(folder.folder).join(&file),
            &crate::http::Checksum::Sha512(sha512),
            |_| {},
        )
        .await?;
        Ok((file, version))
    }

    /// Points the mod at the voice tunnel's public address (or clears it when not shared), so
    /// players' clients connect to the right place.
    /// Points the mod at the voice tunnel's public address (or clears it when not shared), so
    /// players' clients connect to the right place. Logs what players will connect to.
    async fn sync_voice_host(&self, inst: &Instance) {
        let Some(v) = &inst.voice else { return };
        let linked = self.playit.is_linked();
        let lookup = if linked {
            self.playit
                .tunnel_for_port(v.port, "udp")
                .await
                .map(|t| t.map(|t| t.address))
                .map_err(|e| tracing::warn!("voice tunnel lookup failed: {e}"))
        } else {
            Ok(None)
        };
        let dir = self.paths.server_dir(&inst.id);
        let before = mc_voicechat::read_voice_host(&dir, &inst.flavor).unwrap_or_default();
        let host = match mc_voicechat::voice_host_update(linked, lookup) {
            mc_voicechat::HostUpdate::Set(a) => a,
            mc_voicechat::HostUpdate::Clear => String::new(),
            mc_voicechat::HostUpdate::Keep => before.clone(),
        };
        if let Err(e) = mc_voicechat::write_config(&dir, &inst.flavor, v.port, Some(&host)) {
            tracing::warn!("could not update voice chat config: {e}");
            return;
        }
        let console = self.console(&inst.id);
        if host.is_empty() {
            console.push(
                LogStream::System,
                format!("Voice chat: players connect on port {} (not shared online).", v.port),
            );
        } else {
            console.push(LogStream::System, format!("Voice chat: players connect to {host}"));
        }
        if host != before && self.is_active(&inst.id) {
            console.push(
                LogStream::System,
                "Voice chat address changed. Restart the server to apply.",
            );
        }
    }

    async fn ensure_voice_tunnel(&self, inst: &Instance) -> Result<()> {
        let Some(v) = &inst.voice else { return Ok(()) };
        if self.playit.tunnel_for_port(v.port, "udp").await?.is_none() {
            self.playit
                .create_tunnel(None, "udp", v.port, &format!("Hearth voice: {}", inst.name))
                .await?;
        }
        // Write the address right away rather than waiting for the next start.
        self.sync_voice_host(inst).await;
        Ok(())
    }

    async fn remove_voice_tunnel(&self, inst: &Instance) {
        let Some(v) = &inst.voice else { return };
        if !self.playit.is_linked() {
            return;
        }
        if let Ok(Some(t)) = self.playit.tunnel_for_port(v.port, "udp").await {
            if let Err(e) = self.playit.delete_tunnel(&t.id).await {
                tracing::warn!("could not delete voice tunnel: {e}");
            }
        }
    }
}

/// A file whose presence means the server binaries were installed before.
fn provider_marker(inst: &Instance) -> &'static str {
    match inst.game.as_str() {
        // Written at the end of every successful Java install.
        "minecraft-java" => "eula.txt",
        _ => "server.properties",
    }
}

fn validate_memory(m: u32) -> Result<()> {
    if m < MIN_MEMORY_MB {
        return Err(Error::InvalidInput(format!(
            "Give the server at least {MIN_MEMORY_MB} MB of memory."
        )));
    }
    if m > 64 * 1024 {
        return Err(Error::InvalidInput(
            "That's more memory than Hearth allows (64 GB).".into(),
        ));
    }
    Ok(())
}
