//! A server's add-on folder (Minecraft `mods/` or `plugins/`): list, add, enable/disable, delete.
//! Reading a file's display name/version is game-specific and comes from the provider.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, IoContext, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentFolder {
    /// Tab label: "Mods" or "Plugins".
    pub label: &'static str,
    /// Folder inside the server directory.
    pub folder: &'static str,
    /// File extension of add-ons, without the dot.
    pub extension: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentEntry {
    /// File name on disk (including `.disabled` when disabled).
    pub file: String,
    pub name: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub size: u64,
}

pub const DISABLED_SUFFIX: &str = ".disabled";

/// Rejects anything that isn't a plain file name inside the folder.
pub fn check_file_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && name != "."
        && name != ".."
        && Path::new(name).file_name().is_some_and(|f| f == name);
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!("Invalid file name: {name}")))
    }
}

fn is_addon(name: &str, ext: &str) -> bool {
    let base = name.strip_suffix(DISABLED_SUFFIX).unwrap_or(name);
    base.rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case(ext))
}

pub fn list(
    dir: &Path,
    folder: &ContentFolder,
    describe: impl Fn(&Path) -> Option<(String, Option<String>)>,
) -> Result<Vec<ContentEntry>> {
    let root = dir.join(folder.folder);
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for e in std::fs::read_dir(&root).at(&root)? {
        let e = e.at(&root)?;
        let file = e.file_name().to_string_lossy().into_owned();
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) || !is_addon(&file, folder.extension) {
            continue;
        }
        let path = e.path();
        let fallback = file.strip_suffix(DISABLED_SUFFIX).unwrap_or(&file);
        let fallback = fallback.rsplit_once('.').map(|(b, _)| b).unwrap_or(fallback).to_owned();
        let (name, version) = describe(&path).unwrap_or((fallback, None));
        out.push(ContentEntry {
            enabled: !file.ends_with(DISABLED_SUFFIX),
            size: e.metadata().map(|m| m.len()).unwrap_or(0),
            file,
            name,
            version,
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

/// Copies files into the folder. Returns the names added; existing files are replaced.
pub fn add(dir: &Path, folder: &ContentFolder, files: &[PathBuf]) -> Result<Vec<String>> {
    let root = dir.join(folder.folder);
    std::fs::create_dir_all(&root).at(&root)?;
    let mut added = vec![];
    for src in files {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        check_file_name(&name)?;
        if !is_addon(&name, folder.extension) || name.ends_with(DISABLED_SUFFIX) {
            return Err(Error::InvalidInput(format!(
                "{name} isn't a .{} file.",
                folder.extension
            )));
        }
        // Replace a disabled copy of the same file, too.
        let _ = std::fs::remove_file(root.join(format!("{name}{DISABLED_SUFFIX}")));
        std::fs::copy(src, root.join(&name)).at(src)?;
        added.push(name);
    }
    Ok(added)
}

pub fn set_enabled(dir: &Path, folder: &ContentFolder, file: &str, enabled: bool) -> Result<String> {
    check_file_name(file)?;
    let root = dir.join(folder.folder);
    let base = file.strip_suffix(DISABLED_SUFFIX).unwrap_or(file);
    let (from, to) = if enabled {
        (format!("{base}{DISABLED_SUFFIX}"), base.to_owned())
    } else {
        (base.to_owned(), format!("{base}{DISABLED_SUFFIX}"))
    };
    if !root.join(&from).exists() {
        return Ok(to); // already in the requested state
    }
    std::fs::rename(root.join(&from), root.join(&to)).at(root.join(&from))?;
    Ok(to)
}

pub fn delete(dir: &Path, folder: &ContentFolder, file: &str) -> Result<()> {
    check_file_name(file)?;
    let path = dir.join(folder.folder).join(file);
    if path.exists() {
        std::fs::remove_file(&path).at(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODS: ContentFolder = ContentFolder {
        label: "Mods",
        folder: "mods",
        extension: "jar",
    };

    #[test]
    fn add_list_toggle_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("sodium-0.6.jar");
        std::fs::write(&src, b"jar").unwrap();
        let server = tmp.path().join("server");
        assert!(list(&server, &MODS, |_| None).unwrap().is_empty());

        assert_eq!(
            add(&server, &MODS, std::slice::from_ref(&src)).unwrap(),
            vec!["sodium-0.6.jar"]
        );
        std::fs::write(server.join("mods/readme.txt"), b"").unwrap();
        let l = list(&server, &MODS, |_| None).unwrap();
        assert_eq!(l.len(), 1, "non-jar files are ignored");
        assert_eq!(l[0].name, "sodium-0.6");
        assert!(l[0].enabled);

        assert_eq!(
            set_enabled(&server, &MODS, "sodium-0.6.jar", false).unwrap(),
            "sodium-0.6.jar.disabled"
        );
        let l = list(&server, &MODS, |_| Some(("Sodium".into(), Some("0.6".into())))).unwrap();
        assert!(!l[0].enabled);
        assert_eq!(l[0].name, "Sodium");
        set_enabled(&server, &MODS, "sodium-0.6.jar.disabled", true).unwrap();
        assert!(server.join("mods/sodium-0.6.jar").exists());

        delete(&server, &MODS, "sodium-0.6.jar").unwrap();
        assert!(list(&server, &MODS, |_| None).unwrap().is_empty());
    }

    #[test]
    fn rejects_paths_and_wrong_types() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in ["../x.jar", "a/b.jar", "..", "", "C:evil.jar"] {
            assert!(check_file_name(bad).is_err(), "{bad}");
            assert!(delete(tmp.path(), &MODS, bad).is_err());
        }
        let src = tmp.path().join("virus.exe");
        std::fs::write(&src, b"").unwrap();
        assert!(add(tmp.path(), &MODS, &[src]).is_err());
    }
}
