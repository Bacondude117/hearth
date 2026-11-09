use serde::Serialize;

use crate::process::ServerStatus;

/// Everything the core pushes to the UI. Serialized as `{ "type": ..., ...fields }`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    #[serde(rename_all = "camelCase")]
    InstallProgress {
        instance_id: String,
        stage: String,
        /// 0.0..=1.0, or `None` when the size is unknown.
        fraction: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    InstallFinished {
        instance_id: String,
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ServerLog {
        instance_id: String,
        line: LogLine,
    },
    #[serde(rename_all = "camelCase")]
    ServerStatus {
        instance_id: String,
        status: ServerStatus,
    },
    #[serde(rename_all = "camelCase")]
    ServerPlayers {
        instance_id: String,
        players: Vec<String>,
    },
    /// The server list or a server's saved settings changed; refetch.
    InstancesChanged,
    PlayitChanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Monotonic per-process sequence number so the UI can de-duplicate backlog vs live lines.
    pub seq: u64,
    pub text: String,
    pub stream: LogStream,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LogStream {
    Stdout,
    Stderr,
    /// Messages from Hearth itself (e.g. "Starting server...").
    System,
}

/// Implemented by the shell (Tauri) to deliver events to the UI.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: Event);
}

/// A sink that drops everything; handy for tests and CLI use.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: Event) {}
}

/// Collects events in memory (tests).
#[derive(Default)]
pub struct VecSink(pub std::sync::Mutex<Vec<Event>>);

impl EventSink for VecSink {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

impl<T: EventSink> EventSink for std::sync::Arc<T> {
    fn emit(&self, event: Event) {
        (**self).emit(event)
    }
}
