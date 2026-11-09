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
    let version = m
        .versions("minecraft-java", flavor)
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.stable)
        .unwrap()
        .id;
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
    println!(
        "installed {:?} java {:?}",
        installed.installed_build, installed.java_major
    );

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

async fn wait_installed(m: &ServerManager, id: &str) {
    wait_for(
        || {
            let s = m.get(id).unwrap().instance.install_state;
            if let InstallState::Failed { message } = &s {
                panic!("install failed: {message}");
            }
            s == InstallState::Installed
        },
        900,
        "install",
    )
    .await;
}

async fn boot_and_stop(m: &ServerManager, id: &str) {
    m.start(id).await.unwrap();
    wait_for(
        || {
            let s = m.get(id).unwrap().status;
            if let ServerStatus::Crashed { .. } = s {
                for l in m.console_lines(id) {
                    println!("{}", l.text);
                }
                panic!("server crashed");
            }
            s == ServerStatus::Running
        },
        600,
        "server ready",
    )
    .await;
    m.stop(id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn installs_and_boots_fabric() {
    let tmp = tempfile::tempdir().unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    let version = m.versions("minecraft-java", "fabric").await.unwrap()[0].id.clone();
    let inst = m
        .create(CreateServer {
            name: "Fabric Live".into(),
            game: "minecraft-java".into(),
            flavor: "fabric".into(),
            version,
            port: None,
            memory_mb: Some(1536),
            accept_eula: true,
        })
        .unwrap();
    wait_installed(&m, &inst.id).await;
    let s = m.get(&inst.id).unwrap();
    println!(
        "installed {:?} launch {:?}",
        s.instance.installed_build, s.instance.launch
    );
    let dir = m.server_dir(&inst.id).unwrap();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        println!(
            "  file: {} {}",
            e.file_name().to_string_lossy(),
            e.metadata().map(|m| m.len()).unwrap_or(0)
        );
    }
    for l in m.console_lines(&inst.id) {
        println!("  log: {}", l.text);
    }
    assert_eq!(s.content.map(|c| c.label), Some("Mods"));
    boot_and_stop(&m, &inst.id).await;
}

/// Builds a real .mrpack (Fabric API from Modrinth, plus overrides) and imports it.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn imports_and_boots_mrpack() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    let mc = m.versions("minecraft-java", "fabric").await.unwrap()[0].id.clone();

    let client = hearth_core::http::client();
    let url = format!(
        "https://api.modrinth.com/v2/project/fabric-api/version?loaders=%5B%22fabric%22%5D&game_versions=%5B%22{mc}%22%5D"
    );
    let versions: serde_json::Value = client.get(url).send().await.unwrap().json().await.unwrap();
    let file = &versions[0]["files"][0];
    let index = serde_json::json!({
        "formatVersion": 1, "game": "minecraft", "versionId": "1.0.0", "name": "Live Test Pack",
        "dependencies": { "minecraft": mc, "fabric-loader": serde_json::Value::Null },
        "files": [
            { "path": format!("mods/{}", file["filename"].as_str().unwrap()),
              "hashes": { "sha512": file["hashes"]["sha512"], "sha1": file["hashes"]["sha1"] },
              "env": { "client": "required", "server": "required" },
              "downloads": [file["url"]], "fileSize": file["size"] },
            { "path": "mods/client-only.jar", "hashes": { "sha512": "00" },
              "env": { "client": "required", "server": "unsupported" },
              "downloads": ["https://cdn.modrinth.com/data/nope/client-only.jar"], "fileSize": 1 }
        ]
    });
    let mut index = index;
    index["dependencies"].as_object_mut().unwrap().remove("fabric-loader");
    index["dependencies"]["fabric-loader"] = serde_json::json!(
        client
            .get("https://meta.fabricmc.net/v2/versions/loader")
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()[0]["version"]
    );

    let pack = tmp.path().join("live.mrpack");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&pack).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    w.start_file("modrinth.index.json", opts).unwrap();
    w.write_all(index.to_string().as_bytes()).unwrap();
    w.start_file("overrides/config/hearth-test.txt", opts).unwrap();
    w.write_all(b"shared").unwrap();
    w.start_file("server-overrides/config/hearth-test.txt", opts).unwrap();
    w.write_all(b"server").unwrap();
    w.finish().unwrap();

    let info = m.inspect_pack(&pack).await.unwrap();
    println!("{info:?}");
    assert_eq!(info.mod_count, 1);
    assert!(!info.needs_input);

    let inst = m
        .import_pack(hearth_core::manager::ImportPack {
            file: pack,
            name: "Pack Live".into(),
            port: None,
            memory_mb: Some(1536),
            accept_eula: true,
            minecraft: None,
            loader: None,
        })
        .await
        .unwrap();
    wait_installed(&m, &inst.id).await;
    let dir = m.server_dir(&inst.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("config/hearth-test.txt")).unwrap(),
        "server"
    );
    assert!(!dir.join("mods/client-only.jar").exists());
    let s = m.get(&inst.id).unwrap().instance;
    assert!(
        s.pack.as_ref().unwrap().file.is_none(),
        "pack copy removed after install"
    );
    let mods = m.list_content(&inst.id).unwrap();
    println!("mods: {mods:?}");
    assert_eq!(mods.len(), 1);
    assert_eq!(mods[0].name, "Fabric API");

    boot_and_stop(&m, &inst.id).await;
    assert!(m
        .console_lines(&inst.id)
        .iter()
        .any(|l| l.text.contains("fabric-api") || l.text.contains("Loading")));
}

/// The real Fabulously Optimized pack marks every file server-"required" but is a client pack.
#[tokio::test]
#[ignore]
async fn flags_client_only_modpacks() {
    let tmp = tempfile::tempdir().unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    let client = hearth_core::http::client();
    let versions: serde_json::Value = client
        .get("https://api.modrinth.com/v2/project/fabulously-optimized/version")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = versions[0]["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["primary"] == true)
        .unwrap()["url"]
        .as_str()
        .unwrap()
        .to_owned();
    let pack = tmp.path().join("fo.mrpack");
    std::fs::write(&pack, client.get(url).send().await.unwrap().bytes().await.unwrap()).unwrap();

    let info = m.inspect_pack(&pack).await.unwrap();
    println!("{info:#?}");
    assert!(
        info.warnings.iter().any(|w| w.contains("client-side modpack")),
        "{:?}",
        info.warnings
    );
    assert!(info.mod_count < 20, "client mods filtered out, got {}", info.mod_count);
    assert!(info.notes.iter().any(|n| n.contains("client-only")));
}

/// Simple Voice Chat on a real Fabric server: installs from Modrinth and its voice server starts.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn fabric_server_with_voice_chat() {
    let tmp = tempfile::tempdir().unwrap();
    let m = ServerManager::new(tmp.path(), Arc::new(NullSink));
    let version = m.versions("minecraft-java", "fabric").await.unwrap()[0].id.clone();
    let inst = m
        .create(CreateServer {
            name: "Voice Live".into(),
            game: "minecraft-java".into(),
            flavor: "fabric".into(),
            version,
            port: None,
            memory_mb: Some(1536),
            accept_eula: true,
        })
        .unwrap();
    wait_installed(&m, &inst.id).await;
    m.enable_voice(&inst.id).await.unwrap();
    let s = m.get(&inst.id).unwrap().instance;
    let v = s.voice.clone().unwrap();
    println!("voice: {v:?}");
    assert!(m.list_content(&inst.id).unwrap().iter().any(|c| c.file == v.file));

    boot_and_stop(&m, &inst.id).await;
    let log: Vec<String> = m.console_lines(&inst.id).into_iter().map(|l| l.text).collect();
    let voice_lines: Vec<&String> = log.iter().filter(|l| l.to_lowercase().contains("voice chat")).collect();
    println!("{voice_lines:#?}");
    assert!(
        voice_lines.iter().any(|l| l.contains(&v.port.to_string())),
        "voice server should start on {}",
        v.port
    );

    m.disable_voice(&inst.id).await.unwrap();
    assert!(m.get(&inst.id).unwrap().instance.voice.is_none());
    assert!(!m.list_content(&inst.id).unwrap().iter().any(|c| c.file == v.file));
}
