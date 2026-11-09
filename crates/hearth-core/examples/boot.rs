//! Dev helper: starts an installed server, waits until it's ready, then stops it.
//!     cargo run -p hearth-core --example boot -- <data-dir> <server-id>
use std::sync::Arc;
use std::time::Duration;

use hearth_core::events::NullSink;
use hearth_core::manager::ServerManager;
use hearth_core::process::ServerStatus;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (root, id) = (args.next().expect("data dir"), args.next().expect("server id"));
    let m = ServerManager::new(root, Arc::new(NullSink));
    m.start(&id).await.unwrap();
    for _ in 0..600 {
        match m.get(&id).unwrap().status {
            ServerStatus::Running => break,
            ServerStatus::Crashed { .. } => panic!("crashed"),
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    m.stop(&id).await.unwrap();
    println!("status after stop: {:?}", m.get(&id).unwrap().status);
}
