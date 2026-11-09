//! Dev helper: creates and installs a Vanilla server in a data directory.
//!     cargo run -p hearth-core --example seed -- <data-dir> [name]
use std::sync::Arc;
use std::time::Duration;

use hearth_core::events::NullSink;
use hearth_core::instance::InstallState;
use hearth_core::manager::{CreateServer, ServerManager};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().expect("usage: seed <data-dir> [name]");
    let name = args.next().unwrap_or_else(|| "Seeded Server".into());
    let m = ServerManager::new(root, Arc::new(NullSink));
    let version = m.versions("minecraft-java", "vanilla").await.unwrap()[0].id.clone();
    let inst = m
        .create(CreateServer {
            name,
            game: "minecraft-java".into(),
            flavor: "vanilla".into(),
            version,
            port: None,
            memory_mb: Some(1024),
            accept_eula: true,
        })
        .unwrap();
    loop {
        match m.get(&inst.id).unwrap().instance.install_state {
            InstallState::Installed => break,
            InstallState::Failed { message } => panic!("{message}"),
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    println!("installed {}", inst.id);
}
