//! Source RCON (Valve's remote console protocol, TCP). Used for Source-engine servers whose
//! Windows console doesn't read piped input (Left 4 Dead 2).
//!
//! Packets: `i32 size`, `i32 id`, `i32 type`, body, `\0\0` (little-endian). Auth is type 3;
//! the server answers with type 2 carrying our id (or -1 for a wrong password). Commands are
//! type 2; replies are type 0 and may span several packets, so an empty type-0 packet is sent
//! after each command and its echo marks the end of the reply.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::{Error, Result};

const AUTH: i32 = 3;
const EXEC: i32 = 2;
const AUTH_RESPONSE: i32 = 2;
const RESPONSE: i32 = 0;
const TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn encode(id: i32, kind: i32, body: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 14);
    out.extend_from_slice(&((body.len() + 10) as i32).to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Packet {
    pub id: i32,
    pub kind: i32,
    pub body: String,
}

/// Parses one packet's payload (everything after the size field).
pub(crate) fn decode(payload: &[u8]) -> Option<Packet> {
    if payload.len() < 10 {
        return None;
    }
    let id = i32::from_le_bytes(payload[0..4].try_into().ok()?);
    let kind = i32::from_le_bytes(payload[4..8].try_into().ok()?);
    let body = &payload[8..payload.len() - 2];
    Some(Packet {
        id,
        kind,
        body: String::from_utf8_lossy(body).into_owned(),
    })
}

pub struct Rcon {
    stream: TcpStream,
    next_id: i32,
}

fn unreachable_err(e: impl std::fmt::Display) -> Error {
    Error::Conflict(format!("Couldn't reach the server's remote console: {e}"))
}

impl Rcon {
    /// Connects to `127.0.0.1:port` and logs in.
    pub async fn connect(port: u16, password: &str) -> Result<Self> {
        let stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(("127.0.0.1", port)))
            .await
            .map_err(|_| unreachable_err("timed out"))?
            .map_err(unreachable_err)?;
        let _ = stream.set_nodelay(true);
        let mut r = Self { stream, next_id: 1 };
        let id = r.send(AUTH, password).await?;
        loop {
            let p = r.read().await?;
            if p.kind == AUTH_RESPONSE {
                if p.id == -1 {
                    return Err(Error::Conflict(
                        "The server refused Hearth's remote console password. Restart the server to apply it.".into(),
                    ));
                }
                if p.id == id {
                    return Ok(r);
                }
            }
        }
    }

    async fn send(&mut self, kind: i32, body: &str) -> Result<i32> {
        let id = self.next_id;
        self.next_id += 1;
        tokio::time::timeout(TIMEOUT, self.stream.write_all(&encode(id, kind, body)))
            .await
            .map_err(|_| unreachable_err("timed out"))?
            .map_err(unreachable_err)?;
        Ok(id)
    }

    async fn read(&mut self) -> Result<Packet> {
        let fut = async {
            let size = self.stream.read_i32_le().await?;
            if !(10..=1 << 20).contains(&size) {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad packet size"));
            }
            let mut buf = vec![0u8; size as usize];
            self.stream.read_exact(&mut buf).await?;
            Ok(buf)
        };
        let buf = tokio::time::timeout(TIMEOUT, fut)
            .await
            .map_err(|_| unreachable_err("timed out"))?
            .map_err(unreachable_err)?;
        decode(&buf).ok_or_else(|| unreachable_err("bad packet"))
    }

    /// Runs a console command and returns its full reply.
    pub async fn exec(&mut self, command: &str) -> Result<String> {
        let id = self.send(EXEC, command).await?;
        let end = self.send(RESPONSE, "").await?;
        let mut out = String::new();
        loop {
            let p = self.read().await?;
            if p.id == end {
                return Ok(out);
            }
            if p.id == id && p.kind == RESPONSE {
                out.push_str(&p.body);
            }
        }
    }
}

/// Connects, runs one command, and returns the reply.
pub async fn run(port: u16, password: &str, command: &str) -> Result<String> {
    Rcon::connect(port, password).await?.exec(command).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn packets_round_trip() {
        let bytes = encode(7, EXEC, "status");
        assert_eq!(
            i32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        assert_eq!(
            decode(&bytes[4..]),
            Some(Packet {
                id: 7,
                kind: EXEC,
                body: "status".into()
            })
        );
        assert_eq!(decode(&[0; 4]), None);
    }

    /// A tiny Source-style RCON server: checks the password, answers `status` in two packets,
    /// and mirrors the empty end-marker packet like srcds does.
    async fn fake_server(password: &'static str) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            loop {
                let Ok(size) = s.read_i32_le().await else { return };
                let mut buf = vec![0u8; size as usize];
                s.read_exact(&mut buf).await.unwrap();
                let p = decode(&buf).unwrap();
                let reply: Vec<Vec<u8>> = match p.kind {
                    AUTH => vec![
                        encode(p.id, RESPONSE, ""),
                        encode(if p.body == password { p.id } else { -1 }, AUTH_RESPONSE, ""),
                    ],
                    EXEC if p.body == "status" => vec![
                        encode(p.id, RESPONSE, "hostname: Test\n"),
                        encode(p.id, RESPONSE, "players : 1 humans\n"),
                    ],
                    EXEC => vec![encode(p.id, RESPONSE, "")],
                    _ => vec![encode(p.id, RESPONSE, "")],
                };
                for r in reply {
                    s.write_all(&r).await.unwrap();
                }
            }
        });
        port
    }

    #[tokio::test]
    async fn logs_in_and_collects_multi_packet_replies() {
        let port = fake_server("secret").await;
        let mut r = Rcon::connect(port, "secret").await.unwrap();
        assert_eq!(r.exec("status").await.unwrap(), "hostname: Test\nplayers : 1 humans\n");
        assert_eq!(r.exec("say hi").await.unwrap(), "");
    }

    #[tokio::test]
    async fn wrong_password_is_a_readable_error() {
        let port = fake_server("secret").await;
        let err = Rcon::connect(port, "nope").await.err().unwrap().to_string();
        assert!(err.contains("refused"), "{err}");
    }
}
