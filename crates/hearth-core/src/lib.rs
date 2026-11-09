//! Hearth core: everything the desktop app does, with no UI dependency.
//!
//! The Tauri shell constructs a [`manager::ServerManager`] and forwards
//! [`events::Event`]s to the web UI through an [`events::EventSink`].

pub mod archive;
pub mod content;
pub mod error;
pub mod events;
pub mod games;
pub mod http;
pub mod instance;
pub mod java;
pub mod manager;
pub mod net;
pub mod packs;
pub mod paths;
pub mod players;
pub mod playit;
pub mod process;
pub mod properties;
pub mod steamcmd;
pub mod system;
mod winjob;

pub use error::{Error, Result};
