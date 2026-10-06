//! Noticing writes made through another connection (DECISIONS.md 0057).

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use common::at;
use tm_store::{ChangeWatch, Heartbeat, Store};

fn beat(minute: u32) -> Heartbeat {
    Heartbeat {
        pid: 1,
        updated_at: at(6, 12) + chrono::Duration::minutes(i64::from(minute)),
        state: "running".into(),
        next_check: None,
        next_check_at: None,
        check: None,
        loads_today: 0,
        budget: 300,
    }
}

#[tokio::test]
async fn a_write_on_another_connection_is_seen() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("tm.db");
    // The daemon's side: the pool that writes.
    let daemon = Store::open(&path).await.expect("opens");
    daemon.seed_defaults().await.expect("seeds");
    // The app's side: a connection of its own.
    let mut watch = ChangeWatch::open(&path).await.expect("opens");

    assert!(!watch.changed().await.expect("reads"), "the first look");
    assert!(!watch.changed().await.expect("reads"), "nothing written");

    daemon.write_heartbeat(&beat(0)).await.expect("writes");
    assert!(watch.changed().await.expect("reads"), "a commit elsewhere");
    assert!(!watch.changed().await.expect("reads"), "seen once");

    // Reads change nothing.
    daemon.heartbeat().await.expect("reads");
    daemon.list_routes(true).await.expect("reads");
    assert!(!watch.changed().await.expect("reads"));

    // A burst of writes reads as one change.
    for minute in 1..=5 {
        daemon.write_heartbeat(&beat(minute)).await.expect("writes");
    }
    assert!(watch.changed().await.expect("reads"));
    assert!(!watch.changed().await.expect("reads"));

    // A second program's own pool is "another connection" too.
    let app = Store::open(&path).await.expect("opens again");
    app.write_heartbeat(&beat(9)).await.expect("writes");
    assert!(watch.changed().await.expect("reads"));
    daemon.close().await;
    app.close().await;
}

#[tokio::test]
async fn a_missing_file_is_not_created() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("missing.db");
    assert!(ChangeWatch::open(&path).await.is_err());
    assert!(!path.exists());
}
