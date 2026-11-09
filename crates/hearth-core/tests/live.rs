//! Live tests against the real download services. Ignored by default:
//!     cargo test -p hearth-core --test live -- --ignored --nocapture
//! They download ~150 MB (Java + server) into a temp dir.

use std::sync::Arc;
use std::time::Duration;

use hearth_core::events::NullSink;
use hearth_core::instance::InstallState;
use hearth_core::manager::{CreateServer, ServerManager};
use hearth_core::process::ServerStatus;

async fn wait_for(mut f: impl FnMut() -> bool, secs: u64, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
#[ignore]
async fn lists_versions_for_every_game() {
    let tmp = tempfile::tempdir().unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    for g in m.games() {
        for f in &g.flavors {
            let v = m.versions(g.id, f.id).await.unwrap();
            println!("{} / {}: {} versions, newest {:?}", g.id, f.id, v.len(), v.first());
            assert!(!v.is_empty());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn installs_starts_and_stops_paper() {
    install_start_stop("paper").await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn installs_starts_and_stops_vanilla() {
    install_start_stop("vanilla").await
}

async fn install_start_stop(flavor: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    let version = m.versions("minecraft-java", flavor).await.unwrap().into_iter().find(|v| v.stable).unwrap().id;
    let inst = m
        .create(CreateServer {
            name: "Live Test".into(),
            game: "minecraft-java".into(),
            flavor: flavor.into(),
            version,
            port: None,
            memory_mb: Some(1024),
            accept_eula: true,
        })
        .unwrap();
    let id = inst.id.clone();

    wait_for(
        || {
            let s = m.get(&id).unwrap().instance.install_state;
            if let InstallState::Failed { message } = &s {
                panic!("install failed: {message}");
            }
            s == InstallState::Installed
        },
        900,
        "install",
    )
    .await;
    let installed = m.get(&id).unwrap().instance;
    println!("installed {:?} java {:?}", installed.installed_build, installed.java_major);

    m.start(&id).await.unwrap();
    wait_for(
        || {
            let s = m.get(&id).unwrap().status;
            if let ServerStatus::Crashed { .. } = s {
                for l in m.console_lines(&id) {
                    println!("{}", l.text);
                }
                panic!("server crashed");
            }
            s == ServerStatus::Running
        },
        300,
        "server ready",
    )
    .await;

    m.command(&id, "list").await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(m.console_lines(&id).iter().any(|l| l.text.contains("players online")));

    m.stop(&id).await.unwrap();
    assert_eq!(m.get(&id).unwrap().status, ServerStatus::Stopped);
    let props = m.properties(&id).unwrap();
    assert!(props.iter().any(|(k, v)| k == "motd" && v == "Live Test"));
}
