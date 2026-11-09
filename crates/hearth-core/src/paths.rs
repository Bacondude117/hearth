use std::path::{Path, PathBuf};

/// On-disk layout. Everything lives under one root (normally `%APPDATA%\Hearth`).
#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn instance_dir(&self, id: &str) -> PathBuf {
        self.instances().join(id)
    }

    /// The folder the game server itself runs in.
    pub fn server_dir(&self, id: &str) -> PathBuf {
        self.instance_dir(id).join("server")
    }

    pub fn runtimes(&self) -> PathBuf {
        self.root.join("runtimes")
    }

    pub fn tools(&self) -> PathBuf {
        self.root.join("tools")
    }

    pub fn playit(&self) -> PathBuf {
        self.root.join("playit")
    }

    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }
}
