//! Minimal client for the playit.gg control API (https://api.playit.gg).
//!
//! Every endpoint is a JSON POST. Responses are wrapped as
//! `{"status": "success"|"fail"|"error", "data": ...}`.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const API_BASE: &str = "https://api.playit.gg";

#[derive(Debug, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "lowercase")]
pub(crate) enum Envelope<T> {
    Success(T),
    Fail(serde_json::Value),
    Error(serde_json::Value),
}

pub(crate) fn unwrap_envelope<T: DeserializeOwned>(path: &str, body: &str) -> Result<T> {
    let env: Envelope<T> = serde_json::from_str(body).map_err(|e| Error::BadResponse {
        source_name: format!("playit {path}"),
        detail: format!("{e}: {}", body.chars().take(200).collect::<String>()),
    })?;
    match env {
        Envelope::Success(v) => Ok(v),
        Envelope::Fail(v) => Err(Error::Playit(describe_failure(&v))),
        Envelope::Error(v) => Err(Error::Playit(describe_failure(&v))),
    }
}

/// Turns playit's error payloads (`"AgentNotFound"`, `{"type":"auth","message":...}`) into text.
fn describe_failure(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => humanize(s),
        serde_json::Value::Object(map) => {
            let kind = map.get("type").and_then(|t| t.as_str()).unwrap_or("error");
            let detail = map
                .get("message")
                .or_else(|| map.get("details"))
                .map(|d| match d {
                    serde_json::Value::String(s) => humanize(s),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            if detail.is_empty() {
                humanize(kind)
            } else {
                format!("{} ({})", humanize(kind), detail)
            }
        }
        other => other.to_string(),
    }
}

/// `RequiresVerifiedAccount` -> `requires verified account`
fn humanize(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push(' ');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

pub struct PlayitApi {
    client: reqwest::Client,
    base: String,
    secret: Option<String>,
}

impl PlayitApi {
    pub fn new(client: reqwest::Client, secret: Option<String>) -> Self {
        Self {
            client,
            base: API_BASE.into(),
            secret,
        }
    }

    async fn call<Req: Serialize, Res: DeserializeOwned>(&self, path: &str, req: &Req) -> Result<Res> {
        let mut b = self.client.post(format!("{}{}", self.base, path)).json(req);
        if let Some(s) = &self.secret {
            b = b.header(reqwest::header::AUTHORIZATION, format!("Agent-Key {}", s.trim()));
        }
        let resp = b.send().await?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::Playit("too many requests; try again in a minute".into()));
        }
        let body = resp.text().await?;
        unwrap_envelope(path, &body)
    }

    pub async fn claim_setup(&self, code: &str) -> Result<ClaimSetup> {
        self.call(
            "/claim/setup",
            &serde_json::json!({
                "code": code,
                "agent_type": "self-managed",
                "version": concat!("Hearth ", env!("CARGO_PKG_VERSION")),
            }),
        )
        .await
    }

    pub async fn claim_exchange(&self, code: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Key {
            secret_key: String,
        }
        let k: Key = self
            .call("/claim/exchange", &serde_json::json!({ "code": code }))
            .await?;
        Ok(k.secret_key)
    }

    pub async fn rundata(&self) -> Result<RunData> {
        self.call("/v1/agents/rundata", &serde_json::json!({})).await
    }

    pub async fn create_tunnel(
        &self,
        agent_id: &str,
        tunnel_type: Option<&str>,
        port_type: &str,
        local_port: u16,
        name: &str,
    ) -> Result<String> {
        #[derive(Deserialize)]
        struct Id {
            id: String,
        }
        let req = create_tunnel_request(agent_id, tunnel_type, port_type, local_port, name);
        let id: Id = self.call(CREATE_TUNNEL_PATH, &req).await?;
        Ok(id.id)
    }

    pub async fn delete_tunnel(&self, tunnel_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .call("/tunnels/delete", &serde_json::json!({ "tunnel_id": tunnel_id }))
            .await?;
        Ok(())
    }
}

/// playit rejects the newer `/v1/tunnels/create` body ("failed to parse body"); this endpoint
/// and shape were verified against the live API.
pub(crate) const CREATE_TUNNEL_PATH: &str = "/tunnels/create";

pub(crate) fn create_tunnel_request(
    agent_id: &str,
    tunnel_type: Option<&str>,
    port_type: &str,
    local_port: u16,
    name: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "tunnel_type": tunnel_type,
        "port_type": port_type,
        "port_count": 1,
        "origin": {
            "type": "agent",
            "data": { "agent_id": agent_id, "local_ip": "127.0.0.1", "local_port": local_port }
        },
        "enabled": true,
        "alloc": null,
        "firewall_id": null,
        "proxy_protocol": null,
    });
    // playit rejects custom (preset-less) tunnels without a description
    // ("TunnelTypeRequiresDescription").
    if tunnel_type.is_none() {
        body["tunnel_description"] = serde_json::json!(name);
    }
    body
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ClaimSetup {
    WaitingForUserVisit,
    WaitingForUser,
    UserAccepted,
    UserRejected,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunData {
    pub agent_id: String,
    #[serde(default)]
    pub tunnels: Vec<RunTunnel>,
    #[serde(default)]
    pub notices: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunTunnel {
    pub id: String,
    pub name: String,
    pub display_address: String,
    pub port_type: String,
    #[serde(default)]
    pub tunnel_type: Option<String>,
    #[serde(default)]
    pub tunnel_type_display: String,
    pub agent_config: AgentConfig,
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub fields: Vec<ConfigField>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigField {
    pub name: String,
    pub value: String,
}

impl RunTunnel {
    pub fn local_port(&self) -> Option<u16> {
        self.agent_config
            .fields
            .iter()
            .find(|f| f.name == "local_port")
            .and_then(|f| f.value.parse().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_success() {
        let v: ClaimSetup = unwrap_envelope("/claim/setup", r#"{"status":"success","data":"UserAccepted"}"#).unwrap();
        assert_eq!(v, ClaimSetup::UserAccepted);
    }

    #[test]
    fn fail_becomes_readable_error() {
        let e = unwrap_envelope::<ClaimSetup>("/x", r#"{"status":"fail","data":"RequiresVerifiedAccount"}"#)
            .unwrap_err()
            .to_string();
        assert_eq!(e, "playit: requires verified account");
        let e = unwrap_envelope::<ClaimSetup>(
            "/x",
            r#"{"status":"error","data":{"type":"auth","message":"InvalidAgentKey"}}"#,
        )
        .unwrap_err()
        .to_string();
        assert_eq!(e, "playit: auth (invalid agent key)");
    }

    #[test]
    fn parses_rundata() {
        let body = r#"{"status":"success","data":{
            "agent_id":"0b3a3b56-1f0e-4f43-8f52-6a6c9d3f8e11",
            "tunnels":[{"id":"7d3e0c6e-9a0b-4b8f-9a55-1b2c3d4e5f60","internal_id":12,"name":"Hearth: Survival",
              "display_address":"fun-dog.gl.joinmc.link","port_type":"tcp","port_count":1,
              "tunnel_type":"minecraft-java","tunnel_type_display":"Minecraft Java",
              "agent_config":{"fields":[{"name":"local_ip","value":"127.0.0.1"},{"name":"local_port","value":"25565"}]},
              "disabled_reason":null}],
            "pending":[],"notices":[],
            "permissions":{"is_self_managed":true,"has_premium":false,"account_status":"guest"}}}"#;
        let r: RunData = unwrap_envelope("/v1/agents/rundata", body).unwrap();
        assert_eq!(r.tunnels.len(), 1);
        assert_eq!(r.tunnels[0].local_port(), Some(25565));
        assert_eq!(r.tunnels[0].display_address, "fun-dog.gl.joinmc.link");
    }

    #[test]
    /// The exact body that playit accepted in a real test (2026-11-09). playit rejects the
    /// `/v1/tunnels/create` shape with "failed to parse body"; `/tunnels/create` works.
    fn create_request_matches_what_playit_accepts() {
        let r = create_tunnel_request(
            "e05f684d-d21f-424d-a272-6828d10e60d7",
            Some("minecraft-java"),
            "tcp",
            25599,
            "Hearth: S",
        );
        assert_eq!(
            r,
            serde_json::json!({
                "name": "Hearth: S",
                "tunnel_type": "minecraft-java",
                "port_type": "tcp",
                "port_count": 1,
                "origin": { "type": "agent", "data": {
                    "agent_id": "e05f684d-d21f-424d-a272-6828d10e60d7",
                    "local_ip": "127.0.0.1",
                    "local_port": 25599
                }},
                "enabled": true,
                "alloc": null,
                "firewall_id": null,
                "proxy_protocol": null
            })
        );
        assert_eq!(CREATE_TUNNEL_PATH, "/tunnels/create");
        // Games without a playit preset get a plain UDP/TCP tunnel, which playit only accepts
        // with a `tunnel_description` (verified against the live API).
        let r = create_tunnel_request("a", None, "udp", 8211, "Hearth: P");
        assert!(r["tunnel_type"].is_null());
        assert_eq!(r["port_type"], "udp");
        assert_eq!(r["tunnel_description"], "Hearth: P");
        // Preset tunnels don't send one.
        let r = create_tunnel_request("a", Some("palworld"), "udp", 8211, "Hearth: P");
        assert!(r.get("tunnel_description").is_none());
    }
}
