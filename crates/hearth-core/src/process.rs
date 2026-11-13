//! Running one server process: spawn, stream logs, send commands, stop gracefully.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

/// How long console lines are collected before being sent to the UI as one event.
pub const LOG_BATCH_INTERVAL: Duration = Duration::from_millis(100);
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{oneshot, watch};

use crate::error::{Error, Result};
use crate::events::{Event, EventSink, LogLine, LogStream};
use crate::games::{LaunchSpec, LineEvent};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ServerStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed { code: Option<i32> },
}

impl ServerStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

pub const CONSOLE_CAPACITY: usize = 2000;

/// Per-server console history. Outlives individual processes so a crash log stays visible.
pub struct Console {
    instance_id: String,
    lines: Mutex<(u64, VecDeque<LogLine>)>,
    sink: Arc<dyn EventSink>,
    /// Lines waiting to be sent to the UI, and whether a flush is already scheduled.
    pending: Arc<Mutex<Vec<LogLine>>>,
    flush_scheduled: Arc<AtomicBool>,
}

impl Console {
    pub fn new(instance_id: String, sink: Arc<dyn EventSink>) -> Self {
        Self {
            instance_id,
            lines: Mutex::new((0, VecDeque::new())),
            sink,
            pending: Arc::new(Mutex::new(Vec::new())),
            flush_scheduled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn push(&self, stream: LogStream, text: impl Into<String>) {
        let line = {
            let mut g = self.lines.lock().unwrap();
            g.0 += 1;
            let line = LogLine {
                seq: g.0,
                text: text.into(),
                stream,
            };
            if g.1.len() >= CONSOLE_CAPACITY {
                g.1.pop_front();
            }
            g.1.push_back(line.clone());
            line
        };
        {
            let mut p = self.pending.lock().unwrap();
            p.push(line);
            // Never let an unread backlog grow past what the UI keeps anyway.
            if p.len() > CONSOLE_CAPACITY {
                let excess = p.len() - CONSOLE_CAPACITY;
                p.drain(..excess);
            }
        }
        self.schedule_flush();
    }

    /// Sends pending lines as one event after `LOG_BATCH_INTERVAL`. Without a Tokio runtime
    /// (plain unit tests) lines are sent immediately.
    fn schedule_flush(&self) {
        if self.flush_scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        let (pending, scheduled, sink, id) = (
            self.pending.clone(),
            self.flush_scheduled.clone(),
            self.sink.clone(),
            self.instance_id.clone(),
        );
        let flush = move || {
            scheduled.store(false, Ordering::Release);
            let lines = std::mem::take(&mut *pending.lock().unwrap());
            if !lines.is_empty() {
                sink.emit(Event::ServerLog { instance_id: id, lines });
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
                    tokio::time::sleep(LOG_BATCH_INTERVAL).await;
                    flush();
                });
            }
            Err(_) => flush(),
        }
    }

    pub fn snapshot(&self) -> Vec<LogLine> {
        self.lines.lock().unwrap().1.iter().cloned().collect()
    }
}

pub type LineParser = Arc<dyn Fn(&str) -> Option<LineEvent> + Send + Sync>;

pub struct ServerProcess {
    instance_id: String,
    status: Mutex<ServerStatus>,
    players: Mutex<BTreeSet<String>>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    stop_requested: AtomicBool,
    /// Set when the log reports a fatal error, so a following exit code 0 isn't "Stopped".
    crash_seen: AtomicBool,
    kill_tx: Mutex<Option<oneshot::Sender<()>>>,
    exited: watch::Receiver<bool>,
    console: Arc<Console>,
    sink: Arc<dyn EventSink>,
    stop_command: String,
    console_encoding: crate::games::ConsoleEncoding,
    /// Keeps the server's own job (memory limit) open while it runs.
    _job: Option<crate::winjob::JobGuard>,
}

impl ServerProcess {
    pub fn spawn(
        instance_id: String,
        spec: &LaunchSpec,
        parser: LineParser,
        stop_command: &str,
        console: Arc<Console>,
        sink: Arc<dyn EventSink>,
    ) -> Result<Arc<Self>> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .envs(spec.env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(crate::system::CHILD_PROCESS_FLAGS);
        }

        if let Some(log) = &spec.log_file {
            // Old output would otherwise be replayed into the console.
            let _ = std::fs::remove_file(log);
        }
        console.push(LogStream::System, format!("Starting {}", spec.program.display()));
        let mut child = cmd.spawn().map_err(|e| {
            Error::InvalidInput(format!("Could not start the server ({}): {e}", spec.program.display()))
        })?;
        let job = match spec.memory_limit_mb {
            Some(mb) => {
                let job = crate::winjob::adopt_limited(&child, mb);
                if job.is_some() {
                    console.push(LogStream::System, format!("Memory limit: {mb} MB"));
                } else {
                    crate::winjob::adopt(&child);
                }
                job
            }
            None => {
                crate::winjob::adopt(&child);
                None
            }
        };

        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let stdin = child.stdin.take();
        let (kill_tx, kill_rx) = oneshot::channel();
        let (exit_tx, exit_rx) = watch::channel(false);

        let proc = Arc::new(Self {
            instance_id,
            status: Mutex::new(ServerStatus::Starting),
            players: Mutex::new(BTreeSet::new()),
            stdin: tokio::sync::Mutex::new(stdin),
            stop_requested: AtomicBool::new(false),
            crash_seen: AtomicBool::new(false),
            kill_tx: Mutex::new(Some(kill_tx)),
            exited: exit_rx,
            console,
            sink,
            stop_command: stop_command.to_owned(),
            console_encoding: spec.console_encoding,
            _job: job,
        });
        proc.emit_status();

        let out_task = tokio::spawn(Self::pump(
            proc.clone(),
            stdout,
            LogStream::Stdout,
            Some(parser.clone()),
        ));
        let err_task = tokio::spawn(Self::pump(proc.clone(), stderr, LogStream::Stderr, None));
        if let Some(log) = spec.log_file.clone() {
            tokio::spawn(Self::follow_log(proc.clone(), log, parser.clone()));
        }

        let p = proc.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                s = child.wait() => s,
                _ = kill_rx => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            // Drain remaining output so the last lines (often the crash reason) are kept.
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                let _ = out_task.await;
                let _ = err_task.await;
            })
            .await;

            let code = status.ok().and_then(|s| s.code());
            let requested = p.stop_requested.load(Ordering::SeqCst);
            let crashed = p.crash_seen.load(Ordering::SeqCst);
            let final_status = if !crashed && (requested || code == Some(0)) {
                ServerStatus::Stopped
            } else {
                ServerStatus::Crashed { code }
            };
            p.console.push(
                LogStream::System,
                match &final_status {
                    ServerStatus::Crashed { code: Some(c) } => format!("Server stopped unexpectedly (exit code {c})"),
                    ServerStatus::Crashed { code: None } => "Server stopped unexpectedly".to_owned(),
                    _ => "Server stopped".to_owned(),
                },
            );
            *p.stdin.lock().await = None;
            p.players.lock().unwrap().clear();
            p.emit_players();
            p.set_status(final_status);
            let _ = exit_tx.send(true);
        });

        Ok(proc)
    }

    /// Shows a log file's new lines in the console until the server exits. Handles the file
    /// appearing late, partial last lines, and the file being truncated.
    async fn follow_log(self: Arc<Self>, path: PathBuf, parser: LineParser) {
        use tokio::io::AsyncSeekExt;
        let mut offset: u64 = 0;
        let mut partial: Vec<u8> = vec![];
        loop {
            let exited = self.has_exited();
            if let Ok(mut f) = tokio::fs::File::open(&path).await {
                let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
                if len < offset {
                    offset = 0;
                    partial.clear();
                }
                if len > offset && f.seek(std::io::SeekFrom::Start(offset)).await.is_ok() {
                    let mut buf = vec![];
                    if let Ok(n) = (&mut f).take(len - offset).read_to_end(&mut buf).await {
                        offset += n as u64;
                        partial.extend_from_slice(&buf);
                        while let Some(i) = partial.iter().position(|&b| b == b'\n') {
                            let raw: Vec<u8> = partial.drain(..=i).collect();
                            let text = String::from_utf8_lossy(&raw);
                            let line = strip_ansi(text.trim_end_matches(['\r', '\n']));
                            if line.is_empty() {
                                continue;
                            }
                            if let Some(ev) = parser(&line) {
                                self.on_line_event(ev);
                            }
                            self.console.push(LogStream::Stdout, line);
                        }
                    }
                }
            }
            if exited {
                break;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    async fn pump(self: Arc<Self>, stream: impl AsyncRead + Unpin, kind: LogStream, parser: Option<LineParser>) {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::with_capacity(256);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            // Windows consoles may not emit UTF-8; never fail on bad bytes.
            let raw = String::from_utf8_lossy(&buf);
            let line = strip_ansi(raw.trim_end_matches(['\r', '\n']));
            if let Some(ev) = parser.as_ref().and_then(|p| p(&line)) {
                self.on_line_event(ev);
            }
            self.console.push(kind, line);
        }
    }

    fn on_line_event(&self, ev: LineEvent) {
        match ev {
            LineEvent::Ready => {
                let mut s = self.status.lock().unwrap();
                if *s == ServerStatus::Starting {
                    *s = ServerStatus::Running;
                    drop(s);
                    self.emit_status();
                }
            }
            LineEvent::PlayerJoined(n) => {
                self.players.lock().unwrap().insert(n);
                self.emit_players();
            }
            LineEvent::PlayerLeft(n) => {
                self.players.lock().unwrap().remove(&n);
                self.emit_players();
            }
            LineEvent::Crashed => self.crash_seen.store(true, Ordering::SeqCst),
        }
    }

    /// Replaces the online player list (games that report players through an API).
    pub fn set_players(&self, names: Vec<String>) {
        let changed = {
            let mut p = self.players.lock().unwrap();
            let new: BTreeSet<String> = names.into_iter().collect();
            let changed = *p != new;
            *p = new;
            changed
        };
        if changed {
            self.emit_players();
        }
    }

    /// For processes without a readiness banner.
    pub fn force_running(&self) {
        self.on_line_event(LineEvent::Ready);
    }

    pub fn status(&self) -> ServerStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn players(&self) -> Vec<String> {
        self.players.lock().unwrap().iter().cloned().collect()
    }

    pub fn has_exited(&self) -> bool {
        *self.exited.borrow()
    }

    fn set_status(&self, s: ServerStatus) {
        *self.status.lock().unwrap() = s;
        self.emit_status();
    }

    fn emit_status(&self) {
        self.sink.emit(Event::ServerStatus {
            instance_id: self.instance_id.clone(),
            status: self.status(),
        });
    }

    fn emit_players(&self) {
        self.sink.emit(Event::ServerPlayers {
            instance_id: self.instance_id.clone(),
            players: self.players(),
        });
    }

    /// Types a line into the server console.
    pub async fn send_command(&self, line: &str) -> Result<()> {
        let line = line.trim_end_matches(['\r', '\n']);
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| Error::Conflict("The server is not running.".into()))?;
        stdin
            .write_all(&self.console_encoding.encode(&format!("{line}\n")))
            .await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Asks the server to stop; force-kills it if it hasn't exited after `timeout`.
    pub async fn stop(&self, timeout: Duration) {
        if self.has_exited() {
            return;
        }
        self.stop_requested.store(true, Ordering::SeqCst);
        self.set_status(ServerStatus::Stopping);
        self.console.push(LogStream::System, "Stopping server...");
        // Games without a console (stop_command "") are stopped through their API beforehand.
        if !self.stop_command.is_empty() && self.send_command(&self.stop_command).await.is_err() {
            self.kill();
        }
        if tokio::time::timeout(timeout, self.wait()).await.is_err() {
            self.console
                .push(LogStream::System, "Server did not stop in time; forcing it to close.");
            self.kill();
            self.wait().await;
        }
    }

    pub fn kill(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        if let Some(tx) = self.kill_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }

    pub async fn wait(&self) {
        let mut rx = self.exited.clone();
        let _ = rx.wait_for(|e| *e).await;
    }
}

/// Removes ANSI color/escape sequences (`ESC [ ... letter`).
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\u{1b}') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi() {
        assert_eq!(strip_ansi("\u{1b}[32mDone\u{1b}[0m!"), "Done!");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[tokio::test]
    async fn console_batches_bursts_into_few_events() {
        let sink = Arc::new(crate::events::VecSink::default());
        let c = Console::new("x".into(), sink.clone());
        for i in 0..100 {
            c.push(LogStream::Stdout, i.to_string());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let events = sink.0.lock().unwrap();
        let batches: Vec<&Vec<LogLine>> = events
            .iter()
            .filter_map(|e| match e {
                Event::ServerLog { lines, .. } => Some(lines),
                _ => None,
            })
            .collect();
        assert!(
            batches.len() <= 2,
            "100 lines should arrive in 1-2 events, got {}",
            batches.len()
        );
        let texts: Vec<String> = batches.iter().flat_map(|b| b.iter().map(|l| l.text.clone())).collect();
        assert_eq!(
            texts,
            (0..100).map(|i| i.to_string()).collect::<Vec<_>>(),
            "every line, in order"
        );
    }

    #[test]
    fn console_is_bounded() {
        let c = Console::new("x".into(), Arc::new(crate::events::NullSink));
        for i in 0..(CONSOLE_CAPACITY + 10) {
            c.push(LogStream::Stdout, i.to_string());
        }
        let snap = c.snapshot();
        assert_eq!(snap.len(), CONSOLE_CAPACITY);
        assert_eq!(snap.last().unwrap().seq as usize, CONSOLE_CAPACITY + 10);
    }
}
