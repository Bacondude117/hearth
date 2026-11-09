//! Minecraft modpacks: Modrinth `.mrpack` files and pack authors' server-pack `.zip`s.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::StreamExt;
use serde::Deserialize;

use super::mc_loaders::{self, Loader};
use super::{GameContext, Progress};
use crate::archive::{extract_zip, extract_zip_prefix, ExtractOptions};
use crate::error::{Error, IoContext, Result};
use crate::http::{self, Checksum};
use crate::packs::{PackInfo, PackKind};

/// Hosts a `.mrpack` may download from (Modrinth's format rules).
const MRPACK_HOSTS: &[&str] = &[
    "cdn.modrinth.com",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
];
const MODRINTH_API: &str = "https://api.modrinth.com/v2";
const PARALLEL_DOWNLOADS: usize = 6;
/// How many bundled mod jars to open when guessing the loader from metadata.
const MOD_SNIFF_LIMIT: usize = 40;

// ---- .mrpack --------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MrIndex {
    pub name: String,
    #[serde(default)]
    pub version_id: Option<String>,
    pub dependencies: HashMap<String, String>,
    pub files: Vec<MrFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MrFile {
    pub path: String,
    pub hashes: HashMap<String, String>,
    #[serde(default)]
    pub env: Option<MrEnv>,
    pub downloads: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MrEnv {
    #[serde(default)]
    pub server: Option<String>,
}

impl MrFile {
    fn on_server(&self) -> bool {
        self.env.as_ref().and_then(|e| e.server.as_deref()) != Some("unsupported")
    }
}

fn mr_loader(deps: &HashMap<String, String>) -> Result<(Option<Loader>, Option<String>)> {
    if deps.contains_key("quilt-loader") {
        return Err(Error::InvalidInput(
            "This is a Quilt modpack. Hearth supports Fabric, Forge and NeoForge packs.".into(),
        ));
    }
    for (key, loader) in [
        ("fabric-loader", Loader::Fabric),
        ("neoforge", Loader::NeoForge),
        ("forge", Loader::Forge),
    ] {
        if let Some(v) = deps.get(key) {
            return Ok((Some(loader), Some(v.clone())));
        }
    }
    Ok((None, None))
}

pub(crate) fn inspect_mrpack_index(index: &MrIndex) -> Result<PackInfo> {
    let (loader, loader_version) = mr_loader(&index.dependencies)?;
    let server: Vec<&MrFile> = index.files.iter().filter(|f| f.on_server()).collect();
    let skipped = index.files.len() - server.len();
    let mut notes = vec![];
    if skipped > 0 {
        notes.push(format!(
            "{skipped} client-only file{} will be skipped.",
            if skipped == 1 { "" } else { "s" }
        ));
    }
    let mut warnings = vec![];
    for f in &server {
        if let Err(e) = validate_mr_file(f) {
            warnings.push(e.to_string());
        }
    }
    let minecraft = index.dependencies.get("minecraft").cloned();
    Ok(PackInfo {
        kind: PackKind::Mrpack,
        game: "minecraft-java".into(),
        name: index.name.clone(),
        pack_version: index.version_id.clone(),
        needs_input: loader.is_none() || minecraft.is_none(),
        minecraft,
        loader: loader.map(|l| l.flavor().to_owned()),
        loader_version,
        mod_count: server.iter().filter(|f| f.path.starts_with("mods/")).count(),
        notes,
        warnings,
    })
}

// ---- Modrinth metadata ------------------------------------------------------------------------
//
// A pack's own `env.server` flags are unreliable (client packs often mark every file "required").
// Modrinth's per-project `server_side` is the better source, so look files up by hash.

#[derive(Deserialize)]
struct VersionRef {
    project_id: String,
}

#[derive(Deserialize)]
struct ProjectRef {
    id: String,
    #[serde(default)]
    title: String,
    server_side: String,
}

/// sha512 -> the file's Modrinth project `server_side` ("required", "optional", "unsupported",
/// "unknown"). Files Modrinth doesn't know are absent.
pub(crate) async fn modrinth_server_sides(
    client: &reqwest::Client,
    files: &[&MrFile],
) -> Result<HashMap<String, String>> {
    let hashes: Vec<&String> = files.iter().filter_map(|f| f.hashes.get("sha512")).collect();
    if hashes.is_empty() {
        return Ok(HashMap::new());
    }
    let versions: HashMap<String, VersionRef> = client
        .post(format!("{MODRINTH_API}/version_files"))
        .json(&serde_json::json!({ "hashes": hashes, "algorithm": "sha512" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut ids: Vec<&str> = versions.values().map(|v| v.project_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut side_of_project: HashMap<String, String> = HashMap::new();
    for chunk in ids.chunks(100) {
        let projects: Vec<ProjectRef> = client
            .get(format!("{MODRINTH_API}/projects"))
            .query(&[("ids", serde_json::to_string(chunk).expect("serializes"))])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        side_of_project.extend(projects.into_iter().map(|p| (p.id, p.server_side)));
    }
    Ok(versions
        .into_iter()
        .filter_map(|(hash, v)| side_of_project.get(&v.project_id).map(|s| (hash, s.clone())))
        .collect())
}

/// Splits files into those to install and the names of those Modrinth says can't run on a server.
pub(crate) fn split_by_server_support<'a>(
    files: Vec<&'a MrFile>,
    sides: &HashMap<String, String>,
) -> (Vec<&'a MrFile>, Vec<String>) {
    let mut skipped = vec![];
    let kept = files
        .into_iter()
        .filter(|f| {
            let unsupported = f
                .hashes
                .get("sha512")
                .and_then(|h| sides.get(h))
                .is_some_and(|s| s == "unsupported");
            if unsupported {
                skipped.push(
                    Path::new(&f.path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| f.path.clone()),
                );
            }
            !unsupported
        })
        .collect();
    (kept, skipped)
}

/// The pack's own Modrinth project, found by the file's hash: (title, server_side).
async fn modrinth_pack_project(client: &reqwest::Client, file: &Path) -> Option<(String, String)> {
    use sha2::Digest;
    let path = file.to_path_buf();
    let sha = tokio::task::spawn_blocking(move || -> Option<String> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut h = sha2::Sha512::new();
        std::io::copy(&mut f, &mut h).ok()?;
        Some(hex::encode(h.finalize()))
    })
    .await
    .ok()??;
    let v: VersionRef = client
        .get(format!("{MODRINTH_API}/version_file/{sha}"))
        .query(&[("algorithm", "sha512")])
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    let p: ProjectRef = client
        .get(format!("{MODRINTH_API}/project/{}", v.project_id))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    Some((p.title, p.server_side))
}

/// Adds Modrinth's view of which mods (and whether the pack itself) can run on a server.
async fn enrich_mrpack_info(client: &reqwest::Client, file: &Path, index: &MrIndex, info: &mut PackInfo) {
    let server: Vec<&MrFile> = index.files.iter().filter(|f| f.on_server()).collect();
    match modrinth_server_sides(client, &server).await {
        Ok(sides) => {
            let (kept, skipped) = split_by_server_support(server, &sides);
            if !skipped.is_empty() {
                let shown: Vec<&str> = skipped.iter().take(5).map(String::as_str).collect();
                info.notes.push(format!(
                    "{} more mod{} that Modrinth lists as client-only will be skipped ({}{}).",
                    skipped.len(),
                    if skipped.len() == 1 { "" } else { "s" },
                    shown.join(", "),
                    if skipped.len() > shown.len() { ", …" } else { "" }
                ));
            }
            info.mod_count = kept.iter().filter(|f| f.path.starts_with("mods/")).count();
        }
        Err(e) => {
            tracing::warn!("Modrinth lookup failed: {e}");
            info.notes.push(
                "Couldn't reach Modrinth to double-check which mods are client-only; using the pack's own labels."
                    .into(),
            );
        }
    }
    if let Some((title, side)) = modrinth_pack_project(client, file).await {
        if side == "unsupported" {
            info.warnings.insert(
                0,
                format!(
                    "{title} is a client-side modpack: it's made for players' PCs, not servers, and its mods may crash the server. Look for the pack's server pack, or a modpack that supports servers."
                ),
            );
        }
    }
}

fn read_mrpack_index(file: &Path) -> Result<MrIndex> {
    let f = std::fs::File::open(file).at(file)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Archive(e.to_string()))?;
    let mut s = String::new();
    z.by_name("modrinth.index.json")
        .map_err(|_| Error::InvalidInput("This .mrpack has no modrinth.index.json.".into()))?
        .read_to_string(&mut s)
        .map_err(|e| Error::Archive(e.to_string()))?;
    serde_json::from_str(&s).map_err(|e| Error::InvalidInput(format!("This .mrpack's index is invalid: {e}")))
}

fn safe_rel_path(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty() && !p.contains('\\') && path.components().all(|c| matches!(c, Component::Normal(_)))
}

fn validate_mr_file(f: &MrFile) -> Result<()> {
    if !safe_rel_path(&f.path) {
        return Err(Error::InvalidInput(format!(
            "Refusing unsafe file path in pack: {}",
            f.path
        )));
    }
    let url = f
        .downloads
        .first()
        .ok_or_else(|| Error::InvalidInput(format!("{} has no download link.", f.path)))?;
    let host = reqwest::Url::parse(url).ok().and_then(|u| {
        (u.scheme() == "https")
            .then(|| u.host_str().map(str::to_owned))
            .flatten()
    });
    if !host.as_deref().is_some_and(|h| MRPACK_HOSTS.contains(&h)) {
        return Err(Error::InvalidInput(format!(
            "Refusing download from an unapproved site for {}.",
            f.path
        )));
    }
    if !f.hashes.contains_key("sha512") && !f.hashes.contains_key("sha1") {
        return Err(Error::InvalidInput(format!("{} has no checksum.", f.path)));
    }
    Ok(())
}

async fn unpack_mrpack(ctx: &GameContext, file: &Path, dir: &Path, progress: Progress<'_>) -> Result<()> {
    let index = read_mrpack_index(file)?;
    let server: Vec<&MrFile> = index.files.iter().filter(|f| f.on_server()).collect();
    let files: Vec<&MrFile> = match modrinth_server_sides(&ctx.client, &server).await {
        Ok(sides) => split_by_server_support(server, &sides).0,
        Err(e) => {
            tracing::warn!("Modrinth lookup failed, using the pack's own server flags: {e}");
            server
        }
    };
    for f in &files {
        validate_mr_file(f)?;
    }
    let total = files.len().max(1);
    let done = AtomicUsize::new(0);
    progress("Downloading mods", Some(0.0));
    type Job<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;
    let jobs: Vec<Job<'_>> = files
        .iter()
        .map(|f| -> Job<'_> {
            let done = &done;
            Box::pin(async move {
                let checksum = match (f.hashes.get("sha512"), f.hashes.get("sha1")) {
                    (Some(h), _) => Checksum::Sha512(h.clone()),
                    (None, Some(h)) => Checksum::Sha1(h.clone()),
                    _ => Checksum::None,
                };
                let r = http::download(&ctx.client, &f.downloads[0], &dir.join(&f.path), &checksum, |_| {}).await;
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                progress("Downloading mods", Some(n as f64 / total as f64));
                r.map_err(|e| Error::InvalidInput(format!("Couldn't download {}: {e}", f.path)))
            })
        })
        .collect();
    let results: Vec<Result<()>> = futures_util::stream::iter(jobs)
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .collect()
        .await;
    results.into_iter().collect::<Result<Vec<()>>>()?;

    progress("Copying pack files", None);
    let (file, dir) = (file.to_path_buf(), dir.to_path_buf());
    tokio::task::spawn_blocking(move || -> Result<()> {
        // Server overrides win over shared overrides.
        extract_zip_prefix(&file, "overrides", &dir)?;
        extract_zip_prefix(&file, "server-overrides", &dir)?;
        Ok(())
    })
    .await
    .expect("extract task panicked")
}

// ---- server pack .zip -----------------------------------------------------------------------

/// Entry names with the zip's single top-level folder (if any) removed.
fn entry_names(z: &mut zip::ZipArchive<std::fs::File>) -> Vec<(usize, String)> {
    let raw: Vec<(usize, String)> = (0..z.len())
        .filter_map(|i| {
            z.by_index(i)
                .ok()
                .and_then(|e| e.enclosed_name().map(|n| (i, n.to_string_lossy().replace('\\', "/"))))
        })
        .collect();
    let first = |s: &str| s.split('/').next().unwrap_or("").to_owned();
    let root = raw.first().map(|(_, n)| first(n));
    let single_root = root
        .as_ref()
        .is_some_and(|r| raw.iter().all(|(_, n)| first(n) == *r && n.len() > r.len()));
    raw.into_iter()
        .map(|(i, n)| match (&root, single_root) {
            (Some(r), true) => (i, n[r.len()..].trim_start_matches('/').to_owned()),
            _ => (i, n),
        })
        .filter(|(_, n)| !n.is_empty())
        .collect()
}

fn read_entry(z: &mut zip::ZipArchive<std::fs::File>, i: usize, limit: u64) -> Option<Vec<u8>> {
    let e = z.by_index(i).ok()?;
    if e.size() > limit {
        return None;
    }
    let mut buf = Vec::with_capacity(e.size() as usize);
    e.take(limit).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Loader declared by a mod jar's metadata.
fn loader_of_mod_jar(bytes: &[u8]) -> Option<Loader> {
    let z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let has = |n: &str| z.index_for_name(n).is_some();
    if has("META-INF/neoforge.mods.toml") {
        Some(Loader::NeoForge)
    } else if has("META-INF/mods.toml") {
        Some(Loader::Forge)
    } else if has("fabric.mod.json") {
        Some(Loader::Fabric)
    } else {
        None
    }
}

pub(crate) fn parse_variables(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().trim_matches('"').to_owned()))
        .collect()
}

pub(crate) fn inspect_server_zip(file: &Path) -> Result<PackInfo> {
    let f = std::fs::File::open(file).at(file)?;
    let mut z = zip::ZipArchive::new(f).map_err(|_| Error::InvalidInput("That file isn't a valid .zip.".into()))?;
    let names = entry_names(&mut z);
    let top = |n: &str| !n.contains('/');

    let mut loader: Option<Loader> = None;
    let mut mc: Option<String> = None;
    let mut loader_version: Option<String> = None;
    let mut fill = |l: Option<Loader>, m: Option<String>, v: Option<String>| {
        if loader.is_none() {
            loader = l;
        }
        if mc.is_none() {
            mc = m;
        }
        if loader_version.is_none() && loader == l {
            loader_version = v;
        }
    };

    // 1. Forge/NeoForge already installed.
    if let Some((_, n)) = names
        .iter()
        .find(|(_, n)| n.starts_with("libraries/") && (n.ends_with("/win_args.txt") || n.ends_with("/unix_args.txt")))
    {
        if let Some((l, m, v)) = mc_loaders::parse_args_path(n) {
            fill(Some(l), m, Some(v));
        }
    }
    // 2. A bundled installer.
    let installers: Vec<usize> = names
        .iter()
        .filter(|(_, n)| top(n) && n.ends_with(".jar") && n.to_lowercase().contains("installer"))
        .map(|(i, _)| *i)
        .collect();
    for i in installers {
        if let Some((l, m, v)) =
            read_entry(&mut z, i, 50 << 20).and_then(|b| mc_loaders::read_install_profile_from_jar(&b))
        {
            fill(Some(l), Some(m), Some(v));
        }
    }
    // 3. Fabric launcher.
    for (_, n) in names.iter().filter(|(_, n)| top(n)) {
        if let Some((m, v)) = mc_loaders::parse_fabric_launcher_name(n) {
            fill(Some(Loader::Fabric), Some(m), Some(v));
        } else if n == mc_loaders::FABRIC_LAUNCHER || n == "fabric-server-launcher.properties" {
            fill(Some(Loader::Fabric), None, None);
        }
    }
    let quilt = names.iter().any(|(_, n)| top(n) && n.starts_with("quilt-server"));
    // 4. ServerPackCreator's variables.txt.
    if let Some((i, _)) = names.iter().find(|(_, n)| n == "variables.txt") {
        if let Some(text) = read_entry(&mut z, *i, 1 << 20).map(|b| String::from_utf8_lossy(&b).into_owned()) {
            let vars = parse_variables(&text);
            let l = match vars.get("MODLOADER").map(|s| s.to_lowercase()).as_deref() {
                Some("fabric") => Some(Loader::Fabric),
                Some("forge") => Some(Loader::Forge),
                Some("neoforge") => Some(Loader::NeoForge),
                Some("quilt") => {
                    return Err(Error::InvalidInput(
                        "This is a Quilt server pack. Hearth supports Fabric, Forge and NeoForge.".into(),
                    ))
                }
                _ => None,
            };
            fill(
                l,
                vars.get("MINECRAFT_VERSION").cloned(),
                vars.get("MODLOADER_VERSION").cloned(),
            );
        }
    }
    if quilt && loader.is_none() {
        return Err(Error::InvalidInput(
            "This is a Quilt server pack. Hearth supports Fabric, Forge and NeoForge.".into(),
        ));
    }

    let mods: Vec<usize> = names
        .iter()
        .filter(|(_, n)| n.starts_with("mods/") && n.ends_with(".jar") && n.matches('/').count() == 1)
        .map(|(i, _)| *i)
        .collect();
    // 5. Guess from mod metadata.
    if loader.is_none() {
        let mut votes: HashMap<Loader, usize> = HashMap::new();
        for i in mods.iter().take(MOD_SNIFF_LIMIT) {
            if let Some(l) = read_entry(&mut z, *i, 64 << 20).and_then(|b| loader_of_mod_jar(&b)) {
                *votes.entry(l).or_default() += 1;
            }
        }
        loader = votes.into_iter().max_by_key(|(_, n)| *n).map(|(l, _)| l);
    }

    if loader.is_none() && mods.is_empty() {
        return Err(Error::InvalidInput(
            "This zip doesn't look like a Minecraft server pack (no mods folder or loader found). Look for a \"Server Pack\" download on the pack's page.".into(),
        ));
    }
    let mut warnings = vec![];
    if names.iter().any(|(_, n)| n == "manifest.json") && mods.is_empty() {
        warnings.push(
            "This looks like a CurseForge export, not a server pack. Use the pack's \"Server Pack\" download instead."
                .into(),
        );
    }
    let name = file
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['_', '-'], " "))
        .unwrap_or_else(|| "Modpack".into());
    Ok(PackInfo {
        kind: PackKind::ServerPack,
        game: "minecraft-java".into(),
        name: name.trim().to_owned(),
        pack_version: None,
        needs_input: loader.is_none() || mc.is_none(),
        minecraft: mc,
        loader: loader.map(|l| l.flavor().to_owned()),
        loader_version,
        mod_count: mods.len(),
        notes: vec![],
        warnings,
    })
}

// ---- entry points ---------------------------------------------------------------------------

pub fn is_mrpack(file: &Path) -> bool {
    file.extension().is_some_and(|e| e.eq_ignore_ascii_case("mrpack"))
}

pub async fn inspect(client: &reqwest::Client, file: &Path) -> Result<PackInfo> {
    if is_mrpack(file) {
        let index = read_mrpack_index(file)?;
        let mut info = inspect_mrpack_index(&index)?;
        enrich_mrpack_info(client, file, &index, &mut info).await;
        Ok(info)
    } else {
        let file = file.to_path_buf();
        // Sniffing bundled jars can read a lot of data.
        tokio::task::spawn_blocking(move || inspect_server_zip(&file))
            .await
            .expect("inspect task panicked")
    }
}

/// Puts the pack's contents into the server folder.
pub async fn unpack(ctx: &GameContext, file: &Path, dir: &Path, progress: Progress<'_>) -> Result<()> {
    tokio::fs::create_dir_all(dir).await.at(dir)?;
    if is_mrpack(file) {
        unpack_mrpack(ctx, file, dir, progress).await
    } else {
        progress("Unpacking server pack", None);
        let (file, dir) = (file.to_path_buf(), dir.to_path_buf());
        tokio::task::spawn_blocking(move || {
            extract_zip(
                &file,
                &dir,
                &ExtractOptions {
                    strip_single_root: true,
                    ..Default::default()
                },
            )
        })
        .await
        .expect("extract task panicked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (n, b) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        std::fs::write(path, zip_bytes(entries)).unwrap();
    }

    #[test]
    fn real_cobblemon_index() {
        let idx: MrIndex = serde_json::from_str(include_str!("../../tests/fixtures/cobblemon_index.json")).unwrap();
        let info = inspect_mrpack_index(&idx).unwrap();
        assert_eq!(info.name, "Cobblemon Official Modpack [Fabric]");
        assert_eq!(info.minecraft.as_deref(), Some("1.21.1"));
        assert_eq!(info.loader.as_deref(), Some("fabric"));
        assert_eq!(info.loader_version.as_deref(), Some("0.20.1"));
        assert_eq!(info.mod_count, 3);
        assert_eq!(info.notes, vec!["2 client-only files will be skipped."]);
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        assert!(!info.needs_input);
    }

    #[test]
    fn modrinth_server_support_overrides_pack_flags() {
        let file = |path: &str, sha: &str| MrFile {
            path: path.into(),
            hashes: [("sha512".to_string(), sha.to_string())].into(),
            env: Some(MrEnv {
                server: Some("required".into()),
            }),
            downloads: vec!["https://cdn.modrinth.com/a.jar".into()],
        };
        let files = [
            file("mods/iris.jar", "aa"),
            file("mods/lithium.jar", "bb"),
            file("mods/unknown.jar", "cc"),
        ];
        let sides: HashMap<String, String> = [
            ("aa".to_string(), "unsupported".to_string()),
            ("bb".to_string(), "optional".to_string()),
        ]
        .into();
        let (kept, skipped) = split_by_server_support(files.iter().collect(), &sides);
        let kept: Vec<&str> = kept.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            kept,
            vec!["mods/lithium.jar", "mods/unknown.jar"],
            "unknown projects are kept"
        );
        assert_eq!(skipped, vec!["iris"]);
    }

    #[test]
    fn mrpack_rejects_unsafe_paths_and_hosts() {
        let f = |path: &str, url: &str| MrFile {
            path: path.into(),
            hashes: [("sha512".to_string(), "00".to_string())].into(),
            env: None,
            downloads: vec![url.into()],
        };
        assert!(validate_mr_file(&f("mods/a.jar", "https://cdn.modrinth.com/data/x/a.jar")).is_ok());
        assert!(validate_mr_file(&f("../evil.jar", "https://cdn.modrinth.com/a.jar")).is_err());
        assert!(validate_mr_file(&f("/etc/evil", "https://cdn.modrinth.com/a.jar")).is_err());
        assert!(validate_mr_file(&f("mods\\..\\evil.jar", "https://cdn.modrinth.com/a.jar")).is_err());
        assert!(validate_mr_file(&f("mods/a.jar", "https://evil.example.com/a.jar")).is_err());
        assert!(validate_mr_file(&f("mods/a.jar", "http://cdn.modrinth.com/a.jar")).is_err());
    }

    #[test]
    fn quilt_packs_are_refused_clearly() {
        let deps: HashMap<String, String> = [
            ("minecraft".into(), "1.21.1".into()),
            ("quilt-loader".into(), "0.26".into()),
        ]
        .into();
        assert!(mr_loader(&deps).unwrap_err().to_string().contains("Quilt"));
    }

    #[test]
    fn server_pack_with_forge_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("All_The_Mods-Server.zip");
        write_zip(
            &p,
            &[
                (
                    "ATM/libraries/net/minecraftforge/forge/1.20.1-47.3.0/win_args.txt",
                    b"-jar x",
                ),
                ("ATM/mods/a.jar", b""),
                ("ATM/mods/b.jar", b""),
                ("ATM/run.bat", b""),
            ],
        );
        let i = inspect_server_zip(&p).unwrap();
        assert_eq!(
            (i.loader.as_deref(), i.minecraft.as_deref(), i.loader_version.as_deref()),
            (Some("forge"), Some("1.20.1"), Some("47.3.0"))
        );
        assert_eq!(i.mod_count, 2);
        assert_eq!(i.name, "All The Mods Server");
        assert!(!i.needs_input);
    }

    #[test]
    fn server_pack_with_bundled_installer() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pack.zip");
        let installer = zip_bytes(&[(
            "install_profile.json",
            include_bytes!("../../tests/fixtures/neoforge_install_profile.json"),
        )]);
        write_zip(
            &p,
            &[("neoforge-26.3.0.101-installer.jar", &installer), ("mods/x.jar", b"")],
        );
        let i = inspect_server_zip(&p).unwrap();
        assert_eq!(
            (i.loader.as_deref(), i.minecraft.as_deref(), i.loader_version.as_deref()),
            (Some("neoforge"), Some("26.3"), Some("26.3.0.101"))
        );
    }

    #[test]
    fn server_pack_with_fabric_launcher_and_variables() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("fabric.zip");
        write_zip(
            &p,
            &[
                ("fabric-server-mc.1.21.1-loader.0.16.10-launcher.1.0.1.jar", b""),
                ("mods/x.jar", b""),
            ],
        );
        let i = inspect_server_zip(&p).unwrap();
        assert_eq!(
            (i.loader.as_deref(), i.minecraft.as_deref(), i.loader_version.as_deref()),
            (Some("fabric"), Some("1.21.1"), Some("0.16.10"))
        );

        let p = tmp.path().join("spc.zip");
        write_zip(
            &p,
            &[
                (
                    "variables.txt",
                    b"MINECRAFT_VERSION=1.20.1\nMODLOADER=Forge\nMODLOADER_VERSION=47.2.0\nJAVA_ARGS=\"-Xmx4G\"\n",
                ),
                ("mods/x.jar", b""),
            ],
        );
        let i = inspect_server_zip(&p).unwrap();
        assert_eq!(
            (i.loader.as_deref(), i.minecraft.as_deref(), i.loader_version.as_deref()),
            (Some("forge"), Some("1.20.1"), Some("47.2.0"))
        );
    }

    #[test]
    fn server_pack_guesses_loader_from_mods_and_asks_for_version() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("mystery.zip");
        let neo_mod = zip_bytes(&[("META-INF/neoforge.mods.toml", b"modLoader=\"javafml\"")]);
        let fabric_mod = zip_bytes(&[("fabric.mod.json", b"{}")]);
        write_zip(
            &p,
            &[
                ("mods/a.jar", &neo_mod),
                ("mods/b.jar", &neo_mod),
                ("mods/c.jar", &fabric_mod),
                ("config/x.toml", b""),
            ],
        );
        let i = inspect_server_zip(&p).unwrap();
        assert_eq!(i.loader.as_deref(), Some("neoforge"));
        assert_eq!(i.minecraft, None);
        assert!(i.needs_input);
    }

    #[test]
    fn non_pack_zips_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("photos.zip");
        write_zip(&p, &[("cat.jpg", b"")]);
        assert!(inspect_server_zip(&p)
            .unwrap_err()
            .to_string()
            .contains("doesn't look like"));
        let p = tmp.path().join("not-a-zip.zip");
        std::fs::write(&p, b"hello").unwrap();
        assert!(inspect_server_zip(&p).is_err());
    }

    #[tokio::test]
    async fn unpacks_server_pack_stripping_root() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pack.zip");
        write_zip(&p, &[("Pack/mods/a.jar", b"a"), ("Pack/config/c.toml", b"c")]);
        let dir = tmp.path().join("server");
        let paths = crate::paths::AppPaths::new(tmp.path());
        let client = crate::http::client();
        let ctx = GameContext::new(paths, client);
        unpack(&ctx, &p, &dir, &|_, _| {}).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("mods/a.jar")).unwrap(), "a");
        assert!(dir.join("config/c.toml").exists());
    }
}
