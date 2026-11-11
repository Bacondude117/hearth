//! Terraria console flow through `ServerManager` against a fake server (Unix only).
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use hearth_core::events::NullSink;
use hearth_core::instance::{InstallState, Instance, InstanceStore};
use hearth_core::manager::ServerManager;
use hearth_core::paths::AppPaths;
use hearth_core::players::PlayerAction;
use hearth_core::process::ServerStatus;

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn console_server_lifecycle_and_players() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path());
    let id = "terra-0001";
    let dir = paths.server_dir(id);
    std::fs::create_dir_all(&dir).unwrap();
    // A fake TerrariaServer: world gen output, "Server started", then a console loop.
    let exe = dir.join("TerrariaServer.bin.x86_64");
    std::fs::write(
        &exe,
        r#"#!/bin/bash
echo "Terraria Server v1.4.5.8"
printf 'Generating world terrain: 50%%\rGenerating world terrain: 100%%\n'
echo "Listening on port 7790"
echo "Type 'help' for a list of commands."
printf ": Server started\n"
while IFS= read -r line; do
  case "$line" in
    join*) echo ": ${line#join } has joined." ;;
    "kick "*) echo "${line#kick } has left." ; echo "kicked" ;;
    "ban "*) printf '//%s\n10.0.0.9\n' "${line#ban }" >> banlist.txt ; echo "${line#ban } has left." ;;
    exit) echo "Saving world data: 100%"; echo "Backing up world file"; exit 0 ;;
    *) echo ": $line" ;;
  esac
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("hearth-serverconfig.txt"), "port=7790\nworldname=Test\n").unwrap();

    InstanceStore::new(paths.clone())
        .save(&Instance {
            id: id.into(),
            name: "Terra".into(),
            game: "terraria".into(),
            flavor: "vanilla".into(),
            version: "latest".into(),
            installed_build: Some("1.4.5.8".into()),
            port: 7790,
            memory_mb: None,
            java_major: None,
            jvm_args: vec![],
            install_state: InstallState::Installed,
            created_at: Utc::now(),
            playit_tunnel_id: None,
            launch: None,
            pack: None,
            voice: None,
        })
        .unwrap();

    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    m.start(id).await.unwrap();
    eventually("Online after 'Server started'", || {
        m.get(id).unwrap().status == ServerStatus::Running
    })
    .await;

    m.command(id, "join Cool Guy").await.unwrap();
    eventually("join tracked", || {
        m.get(id).unwrap().players == vec!["Cool Guy".to_string()]
    })
    .await;

    m.player_action(id, "Cool Guy", PlayerAction::Ban { reason: None })
        .await
        .unwrap();
    eventually("left after ban", || m.get(id).unwrap().players.is_empty()).await;
    let view = m.players(id).unwrap();
    assert!(view.caps.roles.is_empty() && !view.caps.has_whitelist);
    assert!(view.players.iter().any(|p| p.name == "Cool Guy" && p.banned));

    m.player_action(id, "Cool Guy", PlayerAction::Unban).await.unwrap();
    assert!(!m.players(id).unwrap().players.iter().any(|p| p.banned));

    m.stop(id).await.unwrap();
    assert_eq!(m.get(id).unwrap().status, ServerStatus::Stopped);
    let log: Vec<String> = m.console_lines(id).into_iter().map(|l| l.text).collect();
    assert!(
        log.iter().any(|l| l.contains("Saving world data")),
        "stopped with exit (saves): {log:#?}"
    );
    assert!(log.iter().any(|l| l.contains("-config")) || log.iter().any(|l| l.starts_with("Starting")));
}
