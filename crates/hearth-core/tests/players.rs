//! Player management through `ServerManager` on a stopped server (file edits).

use std::sync::Arc;

use chrono::Utc;
use hearth_core::events::NullSink;
use hearth_core::instance::{InstallState, Instance, InstanceStore};
use hearth_core::manager::ServerManager;
use hearth_core::paths::AppPaths;
use hearth_core::players::{PlayerAction, Role};

fn installed(root: &std::path::Path, id: &str, game: &str) {
    let inst = Instance {
        id: id.into(),
        name: "Test".into(),
        game: game.into(),
        flavor: if game == "minecraft-java" {
            "paper".into()
        } else {
            "bedrock".into()
        },
        version: "26.3".into(),
        installed_build: None,
        port: 25565,
        memory_mb: Some(1024),
        java_major: Some(25),
        jvm_args: vec![],
        install_state: InstallState::Installed,
        created_at: Utc::now(),
        playit_tunnel_id: None,
        launch: None,
        pack: None,
        voice: None,
    };
    InstanceStore::new(AppPaths::new(root)).save(&inst).unwrap();
    std::fs::create_dir_all(AppPaths::new(root).server_dir(id)).unwrap();
}

#[tokio::test]
async fn java_offline_actions_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    installed(tmp.path(), "java-0001", "minecraft-java");
    let dir = AppPaths::new(tmp.path()).server_dir("java-0001");
    std::fs::write(dir.join("server.properties"), "online-mode=true\nwhite-list=true\n").unwrap();
    std::fs::write(
        dir.join("usercache.json"),
        r#"[{"name":"Bacon_Dude117","uuid":"33f7864d-a02c-4e21-ae28-3ecf6f0fd767","expiresOn":"2026-12-09 22:52:49 -0600"}]"#,
    )
    .unwrap();

    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    let view = m.players("java-0001").unwrap();
    assert!(!view.running);
    assert!(view.whitelist_enabled);
    assert_eq!(view.players.len(), 1);
    assert_eq!(view.players[0].role, Role::Player);

    m.player_action(
        "java-0001",
        "Bacon_Dude117",
        PlayerAction::SetRole { role: Role::Operator },
    )
    .await
    .unwrap();
    m.player_action("java-0001", "Bacon_Dude117", PlayerAction::Whitelist { on: true })
        .await
        .unwrap();
    let p = &m.players("java-0001").unwrap().players[0];
    assert_eq!(p.role, Role::Operator);
    assert!(p.whitelisted);

    let err = m
        .player_action("java-0001", "Bacon_Dude117", PlayerAction::Kick { reason: None })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Start the server first"), "{err}");
}

#[tokio::test]
async fn bedrock_role_needs_a_known_xuid() {
    let tmp = tempfile::tempdir().unwrap();
    installed(tmp.path(), "bed-0001", "minecraft-bedrock");
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));

    m.player_action("bed-0001", "Cool Guy 22", PlayerAction::Whitelist { on: true })
        .await
        .unwrap();
    let view = m.players("bed-0001").unwrap();
    assert_eq!(view.caps.whitelist_label, "Allow list");
    assert!(!view.caps.can_ban);
    assert!(view
        .players
        .iter()
        .any(|p| p.name == "Cool Guy 22" && p.whitelisted && p.id.is_none()));

    let err = m
        .player_action(
            "bed-0001",
            "Cool Guy 22",
            PlayerAction::SetRole { role: Role::Operator },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("join the server once"), "{err}");
}

#[tokio::test]
async fn custom_icons_reach_the_game_and_the_summary() {
    let tmp = tempfile::tempdir().unwrap();
    installed(tmp.path(), "java-0002", "minecraft-java");
    installed(tmp.path(), "bed-0002", "minecraft-bedrock");
    let src = tmp.path().join("me.png");
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(200, 100))
        .save(&src)
        .unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));

    assert_eq!(m.get("java-0002").unwrap().icon_version, None);
    m.set_icon("java-0002", &src).unwrap();
    assert!(m.get("java-0002").unwrap().icon_version.is_some());
    assert!(AppPaths::new(tmp.path())
        .server_dir("java-0002")
        .join("server-icon.png")
        .exists());
    assert!(m.icon_data_url("java-0002").unwrap().is_some());

    m.set_icon("bed-0002", &src).unwrap();
    assert!(!AppPaths::new(tmp.path())
        .server_dir("bed-0002")
        .join("server-icon.png")
        .exists());

    m.clear_icon("java-0002").unwrap();
    assert_eq!(m.get("java-0002").unwrap().icon_version, None);
    assert!(!AppPaths::new(tmp.path())
        .server_dir("java-0002")
        .join("server-icon.png")
        .exists());
}

#[tokio::test]
async fn voice_chat_ports_are_reserved() {
    let tmp = tempfile::tempdir().unwrap();
    installed(tmp.path(), "java-0003", "minecraft-java");
    installed(tmp.path(), "java-0004", "minecraft-java");
    let store = InstanceStore::new(AppPaths::new(tmp.path()));
    let mut a = store.get("java-0003").unwrap();
    a.voice = Some(hearth_core::games::mc_voicechat::VoiceChat {
        port: 24454,
        file: "voicechat-fabric.jar".into(),
        version: "2.6".into(),
    });
    store.save(&a).unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    let err = m
        .set_properties("java-0004", [("server-port".to_string(), "24454".to_string())].into())
        .unwrap_err();
    assert!(err.to_string().contains("24454"), "{err}");
}
