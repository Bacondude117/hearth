use std::path::Path;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use sha1::Digest;
use tokio::io::AsyncWriteExt;

use crate::error::{Error, IoContext, Result};

/// User-Agent sent with every request. PaperMC requires a descriptive one.
pub const USER_AGENT: &str = concat!(
    "Hearth/",
    env!("CARGO_PKG_VERSION"),
    " (self-hosted game server manager; https://github.com/hearth-app/hearth)"
);

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("static reqwest config is valid")
}

pub async fn get_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Result<T> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let text = resp.text().await?;
    serde_json::from_str(&text).map_err(|e| Error::BadResponse {
        source_name: host_of(url),
        detail: e.to_string(),
    })
}

pub(crate) fn host_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checksum {
    Sha1(String),
    Sha256(String),
    Sha512(String),
    None,
}

enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
    None,
}

impl Hasher {
    fn for_checksum(c: &Checksum) -> Self {
        match c {
            Checksum::Sha1(_) => Hasher::Sha1(sha1::Sha1::new()),
            Checksum::Sha256(_) => Hasher::Sha256(sha2::Sha256::new()),
            Checksum::Sha512(_) => Hasher::Sha512(sha2::Sha512::new()),
            Checksum::None => Hasher::None,
        }
    }
    fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Sha1(h) => h.update(data),
            Hasher::Sha256(h) => h.update(data),
            Hasher::Sha512(h) => h.update(data),
            Hasher::None => {}
        }
    }
    fn finish(self) -> Option<String> {
        match self {
            Hasher::Sha1(h) => Some(hex::encode(h.finalize())),
            Hasher::Sha256(h) => Some(hex::encode(h.finalize())),
            Hasher::Sha512(h) => Some(hex::encode(h.finalize())),
            Hasher::None => None,
        }
    }
}

/// Streams `url` to `dest`, verifying the checksum. Writes to `dest.part` first so a failed
/// or interrupted download never leaves a half-written file at `dest`.
///
/// `progress` receives a fraction in `0.0..=1.0` when the size is known.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    checksum: &Checksum,
    mut progress: impl FnMut(Option<f64>) + Send,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.at(parent)?;
    }
    let part = dest.with_extension(match dest.extension() {
        Some(ext) => format!("{}.part", ext.to_string_lossy()),
        None => "part".to_owned(),
    });

    let resp = client.get(url).send().await?.error_for_status()?;
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(&part).await.at(&part)?;
    let mut hasher = Hasher::for_checksum(checksum);
    let mut done: u64 = 0;
    let mut last_reported = -1.0f64;
    let mut stream = resp.bytes_stream();

    progress(total.map(|_| 0.0));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.at(&part)?;
        done += chunk.len() as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            let f = (done as f64 / total as f64).min(1.0);
            // Throttle: report each whole percent at most once.
            if f - last_reported >= 0.01 || f >= 1.0 {
                last_reported = f;
                progress(Some(f));
            }
        }
    }
    file.flush().await.at(&part)?;
    drop(file);

    let expected = match checksum {
        Checksum::Sha1(h) | Checksum::Sha256(h) | Checksum::Sha512(h) => Some(h.to_ascii_lowercase()),
        Checksum::None => None,
    };
    if let (Some(expected), Some(actual)) = (expected, hasher.finish()) {
        if expected != actual {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(Error::ChecksumMismatch {
                file: dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            });
        }
    }

    tokio::fs::rename(&part, dest).await.at(dest)?;
    Ok(())
}
