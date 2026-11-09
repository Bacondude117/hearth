//! Display names and versions for Minecraft mod and plugin jars.

use std::io::Read;
use std::path::Path;

fn read(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.by_ref().take(1 << 20).read_to_string(&mut s).ok()?;
    Some(s)
}

/// `key = "value"` from a TOML-ish line (enough for mods.toml without a TOML parser).
fn toml_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
    })
}

/// `key: value` from a YAML-ish line (plugin.yml).
fn yaml_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key && !l.starts_with([' ', '\t']))
            .then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
    })
}

fn manifest_version(z: &mut zip::ZipArchive<std::fs::File>) -> Option<String> {
    read(z, "META-INF/MANIFEST.MF")?
        .lines()
        .find_map(|l| l.strip_prefix("Implementation-Version:").map(|v| v.trim().to_owned()))
}

pub fn describe_jar(path: &Path) -> Option<(String, Option<String>)> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
    let clean = |v: Option<String>| v.filter(|v| !v.is_empty() && !v.contains("${"));

    if let Some(j) = read(&mut z, "fabric.mod.json").or_else(|| read(&mut z, "quilt.mod.json")) {
        let v: serde_json::Value = serde_json::from_str(&j).ok()?;
        let meta = v
            .get("quilt_loader")
            .map(|q| q.get("metadata").unwrap_or(q))
            .unwrap_or(&v);
        let name = meta.get("name").or_else(|| v.get("id")).and_then(|n| n.as_str())?;
        let version = v
            .get("version")
            .or_else(|| v.pointer("/quilt_loader/version"))
            .and_then(|n| n.as_str());
        return Some((name.to_owned(), clean(version.map(str::to_owned))));
    }
    for toml in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Some(t) = read(&mut z, toml) {
            let name = toml_value(&t, "displayName").or_else(|| toml_value(&t, "modId"))?;
            let version = clean(toml_value(&t, "version")).or_else(|| manifest_version(&mut z));
            return Some((name, version));
        }
    }
    for yml in ["paper-plugin.yml", "plugin.yml"] {
        if let Some(t) = read(&mut z, yml) {
            return Some((yaml_value(&t, "name")?, clean(yaml_value(&t, "version"))));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn jar(dir: &Path, name: &str, entries: &[(&str, &str)]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        for (n, b) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        p
    }

    #[test]
    fn reads_each_metadata_format() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let f = jar(
            d,
            "a.jar",
            &[(
                "fabric.mod.json",
                r#"{"id":"sodium","name":"Sodium","version":"0.6.0+mc26.3"}"#,
            )],
        );
        assert_eq!(describe_jar(&f), Some(("Sodium".into(), Some("0.6.0+mc26.3".into()))));

        let f = jar(d, "b.jar", &[
            ("META-INF/neoforge.mods.toml", "modLoader=\"javafml\"\n[[mods]]\nmodId=\"create\"\nversion=\"${file.jarVersion}\"\ndisplayName=\"Create\"\n"),
            ("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\nImplementation-Version: 6.0.4\n"),
        ]);
        assert_eq!(describe_jar(&f), Some(("Create".into(), Some("6.0.4".into()))));

        let f = jar(
            d,
            "c.jar",
            &[(
                "plugin.yml",
                "name: EssentialsX\nversion: 2.21.0\nmain: com.earth2me.Essentials\ncommands:\n  name: nope\n",
            )],
        );
        assert_eq!(describe_jar(&f), Some(("EssentialsX".into(), Some("2.21.0".into()))));

        let f = jar(d, "d.jar", &[("readme.txt", "")]);
        assert_eq!(describe_jar(&f), None);
        std::fs::write(d.join("e.jar"), b"not a zip").unwrap();
        assert_eq!(describe_jar(&d.join("e.jar")), None);
    }
}
