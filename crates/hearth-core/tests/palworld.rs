//! Palworld control flow through `ServerManager`, against a fake server process and a fake
//! REST API (Unix only: the fake server is a shell script).
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use hearth_core::events::NullSink;
use hearth_core::games::palworld::settings_path;
use hearth_core::games::palworld_settings::PalSettings;
use hearth_core::instance::{InstallState, Instance, InstanceStore};
use hearth_core::manager::ServerManager;
use hearth_core::paths::AppPaths;
use hearth_core::players::PlayerAction;
use hearth_core::process::ServerStatus;

/// Answers Palworld REST calls and records them. `shutdown` makes the fake server exit.
fn fake_api(server_dir: std::path::PathBuf) -> (u16, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(vec![]));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            s.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = vec![0u8; 16384];
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
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if body.len() >= len {
                        break;
                    }
                }
            }
            let first = req.lines().next().unwrap_or("").to_owned();
            let authed = req.to_lowercase().contains("authorization: basic ywrtaw46cdrsc3dvcmq="); // admin:p4lsword
            let body = req
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_owned())
                .unwrap_or_default();
            log2.lock().unwrap().push(format!("{first} {body}"));
            let reply = if !authed {
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_owned()
            } else if first.starts_with("GET /v1/api/players") {
                let json = r#"{"players":[{"name":"Bacon_Dude117","accountName":"Ryker","playerId":"P1","userId":"steam_76561198000000001","ping":10}]}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{json}",
                    json.len()
                )
            } else {
                if first.starts_with("POST /v1/api/shutdown") {
                    std::fs::write(server_dir.join("stop-flag"), "").unwrap();
                }
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_owned()
            };
            let _ = s.write_all(reply.as_bytes());
        }
    });
    (port, log)
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..150 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn start_poll_players_broadcast_kick_and_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path());
    let id = "pals-0001";
    let dir = paths.server_dir(id);

    // A fake PalServer that runs until the API's shutdown creates stop-flag.
    let exe = dir.join("Pal/Binaries/Linux/PalServer-Linux-Shipping");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(
        &exe,
        "#!/bin/bash\necho \"fake palworld $@\"\nwhile [ ! -f stop-flag ]; do sleep 0.1; done\necho bye\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let (api_port, log) = fake_api(dir.clone());
    let mut s = PalSettings::default();
    s.set("ServerName", "Pals");
    s.set("AdminPassword", "p4lsword");
    s.set("RESTAPIEnabled", "True");
    s.set("RESTAPIPort", &api_port.to_string());
    s.set("PublicPort", "28211");
    s.save(&settings_path(&dir)).unwrap();

    InstanceStore::new(paths.clone())
        .save(&Instance {
            id: id.into(),
            name: "Pals".into(),
            game: "palworld".into(),
            flavor: "steam".into(),
            version: "latest".into(),
            installed_build: Some("build 1".into()),
            port: 28211,
            memory_mb: None,
            java_major: None,
            jvm_args: vec![],
            install_state: InstallState::Installed,
            created_at: Utc::now(),
            playit_tunnel_id: None,
            launch: None,
            pack: None,
        })
        .unwrap();

    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    m.start(id).await.unwrap();
    assert_eq!(m.get(id).unwrap().status, ServerStatus::Starting);
    eventually("Online via REST poll", || {
        m.get(id).unwrap().status == ServerStatus::Running
    })
    .await;
    eventually("players from REST", || {
        m.get(id).unwrap().players == vec!["Bacon_Dude117".to_string()]
    })
    .await;

    // The players tab knows the Steam ID from the poll.
    let view = m.players(id).unwrap();
    assert!(!view.caps.has_whitelist);
    assert!(view.caps.roles.is_empty());
    assert_eq!(view.players[0].id.as_deref(), Some("steam_76561198000000001"));

    m.command(id, "Server restarts in 5 minutes").await.unwrap();
    m.player_action(id, "Bacon_Dude117", PlayerAction::Kick { reason: None })
        .await
        .unwrap();

    m.stop(id).await.unwrap();
    assert_eq!(m.get(id).unwrap().status, ServerStatus::Stopped);

    let calls = log.lock().unwrap().clone();
    let has = |p: &str| calls.iter().any(|c| c.starts_with(p));
    assert!(has("GET /v1/api/players"), "{calls:#?}");
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("POST /v1/api/announce") && c.contains("Server restarts in 5 minutes")),
        "{calls:#?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("POST /v1/api/kick") && c.contains("steam_76561198000000001")),
        "{calls:#?}"
    );
    let save = calls
        .iter()
        .position(|c| c.starts_with("POST /v1/api/save"))
        .expect("save before stop");
    let shutdown = calls
        .iter()
        .position(|c| c.starts_with("POST /v1/api/shutdown"))
        .expect("shutdown");
    assert!(save < shutdown, "world saved before shutdown");
    assert!(m
        .console_lines(id)
        .iter()
        .any(|l| l.text.contains("fake palworld Pal -port=28211")));
}
