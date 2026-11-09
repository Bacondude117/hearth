use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::paths::AppPaths;

/// One managed server. Stored as `instances/<id>/instance.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub id: String,
    pub name: String,
    /// Provider id, e.g. `minecraft-java`.
    pub game: String,
    /// Provider-specific flavor, e.g. `paper`.
    pub flavor: String,
    /// Game version, e.g. `26.3`, or `latest` for providers that only offer the newest build.
    pub version: String,
    /// Concrete build installed (Paper build number, Bedrock version...), for display.
    #[serde(default)]
    pub installed_build: Option<String>,
    pub port: u16,
    /// Max heap for JVM-based games.
    #[serde(default)]
    pub memory_mb: Option<u32>,
    /// Java feature version this server needs (set by the installer).
    #[serde(default)]
    pub java_major: Option<u32>,
    /// Extra JVM flags (e.g. Paper's recommended flags).
    #[serde(default)]
    pub jvm_args: Vec<String>,
    pub install_state: InstallState,
    pub created_at: DateTime<Utc>,
    /// playit tunnel that exposes this server, if any.
    #[serde(default)]
    pub playit_tunnel_id: Option<String>,
    /// How to start a JVM server (set by the installer; `None` means `-jar server.jar`).
    #[serde(default)]
    pub launch: Option<JavaLaunch>,
    /// The modpack this server was created from, if any.
    #[serde(default)]
    pub pack: Option<PackSource>,
    /// Simple Voice Chat, when installed through Hearth.
    #[serde(default)]
    pub voice: Option<crate::games::mc_voicechat::VoiceChat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JavaLaunch {
    /// `java -jar <jar> nogui`
    Jar { jar: String },
    /// `java @<args_file> nogui` (Forge/NeoForge), paths relative to the server folder.
    ArgsFile { args_file: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackSource {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    /// The pack file copied into the instance folder, until the install succeeds.
    #[serde(default)]
    pub file: Option<String>,
    /// Loader build the pack asked for, if it named one.
    #[serde(default)]
    pub loader_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum InstallState {
    Pending,
    Installing,
    Installed,
    Failed { message: String },
}

#[derive(Debug, Clone)]
pub struct InstanceStore {
    paths: AppPaths,
}

impl InstanceStore {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    fn file(&self, id: &str) -> PathBuf {
        self.paths.instance_dir(id).join("instance.json")
    }

    pub fn list(&self) -> Result<Vec<Instance>> {
        let dir = self.paths.instances();
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(&dir).at(&dir)? {
            let entry = entry.at(&dir)?;
            let f = entry.path().join("instance.json");
            if !f.exists() {
                continue;
            }
            match std::fs::read_to_string(&f).at(&f).and_then(|s| {
                serde_json::from_str::<Instance>(&s).map_err(|e| Error::BadResponse {
                    source_name: f.display().to_string(),
                    detail: e.to_string(),
                })
            }) {
                Ok(i) => out.push(i),
                Err(e) => tracing::warn!("skipping unreadable instance {}: {e}", f.display()),
            }
        }
        out.sort_by_key(|a| a.created_at);
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Instance> {
        let f = self.file(id);
        if !f.exists() {
            return Err(Error::InstanceNotFound(id.to_owned()));
        }
        let s = std::fs::read_to_string(&f).at(&f)?;
        serde_json::from_str(&s).map_err(|e| Error::BadResponse {
            source_name: f.display().to_string(),
            detail: e.to_string(),
        })
    }

    /// Atomic write: temp file then rename, so a crash never leaves a truncated record.
    pub fn save(&self, inst: &Instance) -> Result<()> {
        let dir = self.paths.instance_dir(&inst.id);
        std::fs::create_dir_all(&dir).at(&dir)?;
        let f = self.file(&inst.id);
        let tmp = f.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(inst).expect("instance serializes");
        std::fs::write(&tmp, body).at(&tmp)?;
        std::fs::rename(&tmp, &f).at(&f)?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.paths.instance_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).at(&dir)?;
        }
        Ok(())
    }

    /// `"My Server!"` -> `my-server-1a2b`. Unique within the store.
    pub fn new_id(&self, name: &str) -> String {
        let mut slug: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        while slug.contains("--") {
            slug = slug.replace("--", "-");
        }
        let slug = slug.trim_matches('-');
        let slug: String = if slug.is_empty() {
            "server".into()
        } else {
            slug.chars().take(24).collect()
        };
        loop {
            let suffix: u16 = rand::random();
            let id = format!("{}-{:04x}", slug.trim_end_matches('-'), suffix);
            if !self.paths.instance_dir(&id).exists() {
                return id;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> Instance {
        Instance {
            id: id.into(),
            name: "Test".into(),
            game: "minecraft-java".into(),
            flavor: "paper".into(),
            version: "26.3".into(),
            installed_build: None,
            port: 25565,
            memory_mb: Some(2048),
            java_major: None,
            jvm_args: vec![],
            install_state: InstallState::Pending,
            created_at: Utc::now(),
            playit_tunnel_id: None,
            launch: None,
            pack: None,
            voice: None,
        }
    }

    #[test]
    fn save_list_get_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = InstanceStore::new(AppPaths::new(tmp.path()));
        assert!(store.list().unwrap().is_empty());
        let a = sample("a-0001");
        store.save(&a).unwrap();
        assert_eq!(store.get("a-0001").unwrap(), a);
        assert_eq!(store.list().unwrap().len(), 1);
        store.delete("a-0001").unwrap();
        assert!(matches!(store.get("a-0001"), Err(Error::InstanceNotFound(_))));
    }

    #[test]
    fn ids_are_slugged() {
        let tmp = tempfile::tempdir().unwrap();
        let store = InstanceStore::new(AppPaths::new(tmp.path()));
        let id = store.new_id("My  Cool Server!!");
        assert!(id.starts_with("my-cool-server-"), "{id}");
        let id = store.new_id("???");
        assert!(id.starts_with("server-"), "{id}");
    }

    #[test]
    fn install_state_serializes_tagged() {
        let s = serde_json::to_string(&InstallState::Failed { message: "x".into() }).unwrap();
        assert_eq!(s, r#"{"state":"failed","message":"x"}"#);
    }
}
