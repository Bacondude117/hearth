use std::path::PathBuf;

/// Every failure the UI can see. Messages are written for end users.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Network error: {}", describe_http(.0))]
    Http(#[from] reqwest::Error),

    #[error("File error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("I/O error: {0}")]
    RawIo(#[from] std::io::Error),

    #[error("Unexpected data from {source_name}: {detail}")]
    BadResponse { source_name: String, detail: String },

    #[error("Download of {file} was corrupted (checksum mismatch). Please try again.")]
    ChecksumMismatch { file: String },

    #[error("Could not extract archive: {0}")]
    Archive(String),

    #[error("Server not found: {0}")]
    InstanceNotFound(String),

    #[error("{0}")]
    InvalidInput(String),

    #[error("{0}")]
    Conflict(String),

    #[error("playit: {0}")]
    Playit(String),

    #[error("{0} is not supported on this operating system")]
    Unsupported(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io { path: path.into(), source })
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// reqwest's top-level message ("error sending request") hides the useful part
/// (DNS failure, refused connection, TLS problem); include the whole cause chain.
fn describe_http(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        let part = s.to_string();
        if !msg.contains(&part) {
            msg.push_str(": ");
            msg.push_str(&part);
        }
        src = s.source();
    }
    msg
}
