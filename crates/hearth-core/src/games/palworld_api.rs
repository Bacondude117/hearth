//! Palworld's REST admin API (`RESTAPIEnabled=True`), reached on 127.0.0.1 with HTTP Basic auth
//! `admin:<AdminPassword>`. Endpoints as used by thijsvanloef/palworld-server-docker:
//! GET players/info, POST save, shutdown {waittime,message}, kick/ban {userid,message},
//! unban {userid}, announce {message}.

use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};

pub struct PalApi {
    client: reqwest::Client,
    base: String,
    password: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PalPlayer {
    pub name: String,
    #[serde(default, rename = "userId")]
    pub user_id: String,
    #[serde(default, rename = "playerId")]
    pub player_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PlayersResponse {
    #[serde(default)]
    pub players: Vec<PalPlayer>,
}

impl PalApi {
    pub fn new(port: u16, password: &str) -> Self {
        // Local-only, short timeouts: this is polled while the server runs.
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(2))
            .build()
            .expect("static reqwest config is valid");
        Self { client, base: format!("http://127.0.0.1:{port}/v1/api"), password: password.to_owned() }
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let resp = req.basic_auth("admin", Some(&self.password)).send().await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::InvalidInput(
                "Palworld rejected Hearth's admin password. Check AdminPassword in Settings.".into(),
            ));
        }
        Ok(resp.error_for_status()?)
    }

    pub async fn players(&self) -> Result<Vec<PalPlayer>> {
        let r: PlayersResponse = self.send(self.client.get(format!("{}/players", self.base))).await?.json().await?;
        Ok(r.players)
    }

    async fn post(&self, path: &str, body: serde_json::Value) -> Result<()> {
        self.send(self.client.post(format!("{}/{path}", self.base)).json(&body)).await?;
        Ok(())
    }

    pub async fn save(&self) -> Result<()> {
        self.post("save", serde_json::json!({})).await
    }

    pub async fn shutdown(&self, wait_secs: u32, message: &str) -> Result<()> {
        self.post("shutdown", serde_json::json!({ "waittime": wait_secs, "message": message })).await
    }

    pub async fn announce(&self, message: &str) -> Result<()> {
        self.post("announce", serde_json::json!({ "message": message })).await
    }

    pub async fn kick(&self, user_id: &str, message: &str) -> Result<()> {
        self.post("kick", serde_json::json!({ "userid": user_id, "message": message })).await
    }

    pub async fn ban(&self, user_id: &str, message: &str) -> Result<()> {
        self.post("ban", serde_json::json!({ "userid": user_id, "message": message })).await
    }

    pub async fn unban(&self, user_id: &str) -> Result<()> {
        self.post("unban", serde_json::json!({ "userid": user_id })).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn parses_players_response() {
        let r: PlayersResponse = serde_json::from_str(
            r#"{"players":[{"name":"Bacon_Dude117","accountName":"Ryker","playerId":"A1B2C3D4","userId":"steam_76561198000000001","ip":"127.0.0.1","ping":12.5,"location_x":1.0,"location_y":2.0,"level":12,"building_count":40}]}"#,
        )
        .unwrap();
        assert_eq!(r.players[0].name, "Bacon_Dude117");
        assert_eq!(r.players[0].user_id, "steam_76561198000000001");
    }

    /// One-shot HTTP server that records the request and replies with `reply`.
    fn fake_server(reply: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buf = vec![0u8; 8192];
            let mut req = String::new();
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                req.push_str(&String::from_utf8_lossy(&buf[..n]));
                if let Some((head, body)) = req.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if body.len() >= len {
                        break;
                    }
                }
            }
            s.write_all(reply.as_bytes()).unwrap();
            req
        });
        (port, h)
    }

    #[tokio::test]
    async fn sends_basic_auth_and_json_bodies() {
        let (port, h) = fake_server("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        PalApi::new(port, "s3cret").kick("steam_1", "bye").await.unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /v1/api/kick "), "{req}");
        // base64("admin:s3cret")
        assert!(req.contains("authorization: Basic YWRtaW46czNjcmV0") || req.contains("Authorization: Basic YWRtaW46czNjcmV0"), "{req}");
        assert!(req.ends_with(r#"{"message":"bye","userid":"steam_1"}"#) || req.ends_with(r#"{"userid":"steam_1","message":"bye"}"#), "{req}");
    }

    #[tokio::test]
    async fn wrong_password_is_a_readable_error() {
        let (port, _h) = fake_server("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
        let e = PalApi::new(port, "nope").players().await.unwrap_err();
        assert!(e.to_string().contains("admin password"), "{e}");
    }
}
