//! Process supervisor tests against a scripted fake server (Unix only: the fake is bash).
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hearth_core::events::{Event, VecSink};
use hearth_core::games::{minecraft_java::MinecraftJava, GameProvider, LaunchSpec};
use hearth_core::process::{Console, LineParser, ServerProcess, ServerStatus};

fn spawn(mode: &str) -> (Arc<ServerProcess>, Arc<Console>, Arc<VecSink>) {
    let sink = Arc::new(VecSink::default());
    let console = Arc::new(Console::new("t-0001".into(), sink.clone()));
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_server.sh");
    let spec = LaunchSpec {
        program: "bash".into(),
        args: vec![script.to_string_lossy().into(), mode.into()],
        cwd: std::env::temp_dir(),
        env: vec![],
    };
    let parser: LineParser = Arc::new(|l: &str| MinecraftJava.parse_line(l));
    let p = ServerProcess::spawn("t-0001".into(), &spec, parser, "stop", console.clone(), sink.clone()).unwrap();
    (p, console, sink)
}

async fn eventually(mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test]
async fn becomes_running_and_stops_gracefully() {
    let (p, console, sink) = spawn("normal");
    eventually(|| p.status() == ServerStatus::Running).await;

    p.send_command("hello").await.unwrap();
    eventually(|| console.snapshot().iter().any(|l| l.text == "echo: hello")).await;

    p.stop(Duration::from_secs(5)).await;
    assert_eq!(p.status(), ServerStatus::Stopped);
    assert!(console.snapshot().iter().any(|l| l.text == "Stopping the server"));

    let statuses: Vec<_> = sink
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::ServerStatus { status, .. } => Some(status.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(statuses.first(), Some(&ServerStatus::Starting));
    assert!(statuses.contains(&ServerStatus::Running));
    assert!(statuses.contains(&ServerStatus::Stopping));
    assert_eq!(statuses.last(), Some(&ServerStatus::Stopped));
}

#[tokio::test]
async fn tracks_players() {
    let (p, _console, _sink) = spawn("normal");
    eventually(|| p.status() == ServerStatus::Running).await;
    p.send_command("join Alex").await.unwrap();
    p.send_command("join Steve").await.unwrap();
    eventually(|| p.players() == vec!["Alex".to_string(), "Steve".to_string()]).await;
    p.send_command("leave Alex").await.unwrap();
    eventually(|| p.players() == vec!["Steve".to_string()]).await;
    p.stop(Duration::from_secs(5)).await;
    assert!(p.players().is_empty(), "players cleared on exit");
}

#[tokio::test]
async fn reports_crash_with_exit_code_and_keeps_stderr() {
    let (p, console, _sink) = spawn("crash");
    eventually(|| p.status() == ServerStatus::Running).await;
    p.send_command("crash").await.unwrap();
    p.wait().await;
    assert_eq!(p.status(), ServerStatus::Crashed { code: Some(3) });
    assert!(console.snapshot().iter().any(|l| l.text == "boom"));
    assert!(p.send_command("x").await.is_err(), "stdin closed after exit");
}

#[tokio::test]
async fn force_kills_after_timeout() {
    let (p, console, _sink) = spawn("stubborn");
    eventually(|| p.status() == ServerStatus::Running).await;
    let started = std::time::Instant::now();
    p.stop(Duration::from_millis(500)).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        p.status(),
        ServerStatus::Stopped,
        "a requested stop is never reported as a crash"
    );
    assert!(console
        .snapshot()
        .iter()
        .any(|l| l.text.contains("forcing it to close")));
}

#[tokio::test]
async fn spawn_failure_is_a_readable_error() {
    let sink = Arc::new(VecSink::default());
    let console = Arc::new(Console::new("t".into(), sink.clone()));
    let spec = LaunchSpec {
        program: "/definitely/not/here".into(),
        args: vec![],
        cwd: std::env::temp_dir(),
        env: vec![],
    };
    let parser: LineParser = Arc::new(|_: &str| None);
    let err = ServerProcess::spawn("t".into(), &spec, parser, "stop", console, sink)
        .err()
        .unwrap();
    assert!(err.to_string().contains("Could not start the server"), "{err}");
}

#[tokio::test]
async fn crash_report_counts_as_crash_even_with_exit_code_zero() {
    let (p, _console, _sink) = spawn("normal");
    eventually(|| p.status() == ServerStatus::Running).await;
    p.send_command("selfcrash").await.unwrap();
    p.wait().await;
    assert_eq!(p.status(), ServerStatus::Crashed { code: Some(0) });
}
