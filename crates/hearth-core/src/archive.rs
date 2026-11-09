use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, IoContext, Result};

#[derive(Debug, Default, Clone)]
pub struct ExtractOptions {
    /// Drop the single top-level folder if every entry lives under one (JDK zips do this).
    pub strip_single_root: bool,
    /// Top-level names (files or folders) that must not be overwritten if they already exist.
    pub preserve: Vec<String>,
}

/// Extracts a zip into `dest`, refusing any entry that would escape `dest`.
pub fn extract_zip(zip_path: &Path, dest: &Path, opts: &ExtractOptions) -> Result<()> {
    let file = fs::File::open(zip_path).at(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Archive(e.to_string()))?;

    let root = if opts.strip_single_root { single_root(&mut zip)? } else { None };

    fs::create_dir_all(dest).at(dest)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| Error::Archive(e.to_string()))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(Error::Archive(format!("unsafe path in archive: {}", entry.name())));
        };
        let rel = match &root {
            Some(r) => match rel.strip_prefix(r) {
                Ok(p) if p.as_os_str().is_empty() => continue,
                Ok(p) => p.to_path_buf(),
                Err(_) => rel,
            },
            None => rel,
        };
        if !is_safe_relative(&rel) {
            return Err(Error::Archive(format!("unsafe path in archive: {}", entry.name())));
        }
        if let Some(top) = rel.components().next() {
            let top = top.as_os_str().to_string_lossy();
            if opts.preserve.iter().any(|p| p.eq_ignore_ascii_case(&top)) && dest.join(&*top).exists() {
                continue;
            }
        }

        let out = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).at(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).at(parent)?;
        }
        let mut f = fs::File::create(&out).at(&out)?;
        io::copy(&mut entry, &mut f).at(&out)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// Extracts only the entries under top-level folder `prefix` (e.g. `overrides`), placing them
/// directly in `dest`. Returns how many files were written.
pub fn extract_zip_prefix(zip_path: &Path, prefix: &str, dest: &Path) -> Result<usize> {
    let file = fs::File::open(zip_path).at(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Archive(e.to_string()))?;
    let mut written = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| Error::Archive(e.to_string()))?;
        let Some(name) = entry.enclosed_name() else {
            return Err(Error::Archive(format!("unsafe path in archive: {}", entry.name())));
        };
        let Ok(rel) = name.strip_prefix(prefix) else { continue };
        if rel.as_os_str().is_empty() || !is_safe_relative(rel) {
            continue;
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).at(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).at(parent)?;
        }
        let mut f = fs::File::create(&out).at(&out)?;
        io::copy(&mut entry, &mut f).at(&out)?;
        written += 1;
    }
    Ok(written)
}

fn single_root(zip: &mut zip::ZipArchive<fs::File>) -> Result<Option<PathBuf>> {
    let mut root: Option<PathBuf> = None;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| Error::Archive(e.to_string()))?;
        let Some(name) = entry.enclosed_name() else { continue };
        let mut comps = name.components();
        let Some(first) = comps.next() else { continue };
        // A file sitting at the top level means there is no single root folder.
        if comps.next().is_none() && !entry.is_dir() {
            return Ok(None);
        }
        let first = PathBuf::from(first.as_os_str());
        match &root {
            None => root = Some(first),
            Some(r) if *r == first => {}
            Some(_) => return Ok(None),
        }
    }
    Ok(root)
}

fn is_safe_relative(p: &Path) -> bool {
    p.components().all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, entries: &[(&str, &str)]) {
        let f = fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        for (name, body) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, zip::write::SimpleFileOptions::default()).unwrap();
            } else {
                w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
        }
        w.finish().unwrap();
    }

    #[test]
    fn strips_single_root() {
        let tmp = tempfile::tempdir().unwrap();
        let z = tmp.path().join("a.zip");
        make_zip(&z, &[("jdk-21/", ""), ("jdk-21/bin/java", "x"), ("jdk-21/release", "r")]);
        let out = tmp.path().join("out");
        extract_zip(&z, &out, &ExtractOptions { strip_single_root: true, ..Default::default() }).unwrap();
        assert!(out.join("bin/java").exists());
        assert!(out.join("release").exists());
    }

    #[test]
    fn keeps_layout_when_multiple_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let z = tmp.path().join("a.zip");
        make_zip(&z, &[("bedrock_server.exe", "x"), ("worlds/", ""), ("server.properties", "a=1")]);
        let out = tmp.path().join("out");
        extract_zip(&z, &out, &ExtractOptions { strip_single_root: true, ..Default::default() }).unwrap();
        assert!(out.join("bedrock_server.exe").exists());
        assert!(out.join("server.properties").exists());
    }

    #[test]
    fn preserves_listed_files() {
        let tmp = tempfile::tempdir().unwrap();
        let z = tmp.path().join("a.zip");
        make_zip(&z, &[("server.properties", "new"), ("bedrock_server.exe", "v2")]);
        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("server.properties"), "mine").unwrap();
        fs::write(out.join("bedrock_server.exe"), "v1").unwrap();
        let opts = ExtractOptions { preserve: vec!["server.properties".into()], ..Default::default() };
        extract_zip(&z, &out, &opts).unwrap();
        assert_eq!(fs::read_to_string(out.join("server.properties")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(out.join("bedrock_server.exe")).unwrap(), "v2");
    }

    #[test]
    fn extracts_only_a_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let z = tmp.path().join("p.zip");
        make_zip(&z, &[("modrinth.index.json", "{}"), ("overrides/config/a.toml", "a"), ("server-overrides/config/a.toml", "b")]);
        let out = tmp.path().join("out");
        assert_eq!(extract_zip_prefix(&z, "overrides", &out).unwrap(), 1);
        assert_eq!(fs::read_to_string(out.join("config/a.toml")).unwrap(), "a");
        extract_zip_prefix(&z, "server-overrides", &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("config/a.toml")).unwrap(), "b");
        assert!(!out.join("modrinth.index.json").exists());
    }

    #[test]
    fn rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let z = tmp.path().join("evil.zip");
        make_zip(&z, &[("../escape.txt", "x")]);
        let out = tmp.path().join("out");
        assert!(extract_zip(&z, &out, &ExtractOptions::default()).is_err());
        assert!(!tmp.path().join("escape.txt").exists());
    }
}
