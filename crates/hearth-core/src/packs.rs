//! Modpack import: what the UI sees after inspecting a pack file. Game-specific inspection and
//! unpacking live in each provider (see `games::mc_packs`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PackKind {
    /// Modrinth `.mrpack`.
    Mrpack,
    /// A ready-made server folder zipped up by the pack author.
    ServerPack,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackInfo {
    pub kind: PackKind,
    /// The game the pack is for (provider id).
    pub game: String,
    pub name: String,
    pub pack_version: Option<String>,
    pub minecraft: Option<String>,
    /// Flavor id of the loader (`fabric`, `forge`, `neoforge`).
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    /// Server-side mods the pack will install.
    pub mod_count: usize,
    /// Informational lines ("41 client-only mods will be skipped").
    pub notes: Vec<String>,
    /// Problems the user should know about before importing.
    pub warnings: Vec<String>,
    /// Loader or Minecraft version couldn't be detected; the user must choose.
    pub needs_input: bool,
}
