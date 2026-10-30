//! The session feeds' socket (`droppen_drive::feed::mux`) end to end, on the drive
//! the server builds (`feed_session`'s `test_drive`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use droppen_drive::Drive;
use droppen_drive::feed::mux::{parse_client_message, ClientMessage, Subscriptions, MAX_SUBSCRIPTIONS};
use droppen_drive::feed::FeedRegistry;

use super::feed_session::{claude_turns, test_drive};

const ID: &str = "55555555-5555-4555-8555-555555555555";

/// A drive with one ~1000-item claude session in its managed root.
fn session(dir: &std::path::Path) -> (Arc<Drive>, std::path::PathBuf) {
    let (paths, drive) = test_drive(dir);
    let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-w-mux");
    std::fs::create_dir_all(&project).expect("project");
    let file = project.join(format!("{ID}.jsonl"));
    std::fs::write(&file, claude_turns(ID, 0, 250)).expect("transcript");
    (drive, file)
}

fn sub(sub: u64, extra: Value) -> ClientMessage {
    let mut entry = serde_json::json!({ "sub": sub, "source": "claude", "id": ID });
    if let (Value::Object(entry), Value::Object(extra)) = (&mut entry, extra) {
        entry.extend(extra);
    }
    parse_client_message(&serde_json::json!({ "type": "sub", "feeds": [entry] }).to_string()).expect("a sub")
}

async fn next(rx: &mut mpsc::Receiver<String>) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a frame in time")
        .expect("open");
    serde_json::from_str(&frame).expect("json")
}

async fn settle(registry: &FeedRegistry) {
    for _ in 0..100 {
        if registry.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_subscription_opens_with_a_short_tagged_snapshot_and_resumes_with_a_delta() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (drive, _) = session(dir.path());
    let registry = FeedRegistry::new();
    let (mut subs, mut rx) = Subscriptions::new(registry.clone(), drive);

    assert!(subs.handle(sub(1, serde_json::json!({ "limit": 60 }))).is_empty());
    let snapshot = next(&mut rx).await;
    assert_eq!(snapshot["sub"], 1);
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["endIndex"], 1000);
    assert_eq!(snapshot["items"].as_array().map(Vec::len), Some(60), "the page the client asked for");
    assert_eq!(snapshot["startIndex"], 940);
    assert_eq!(snapshot["hasMoreBefore"], true);
    assert!(snapshot["startFp"].is_string(), "a short page is still provable for history");
    let state = next(&mut rx).await;
    assert_eq!((state["sub"].clone(), state["type"].clone()), (Value::from(1), Value::from("state")));

    // A second subscription to the same session, resuming: the overlap only.
    subs.handle(sub(2, serde_json::json!({ "since": 1000, "fp": snapshot["prefixFp"] })));
    let delta = next(&mut rx).await;
    assert_eq!(delta["sub"], 2);
    assert_eq!(delta["type"], "delta");
    assert_eq!(delta["fromIndex"], 1000 - droppen_sessions::loader::RESUME_OVERLAP);
    assert_eq!(registry.len(), 1, "two subscriptions, one watcher");
    assert_eq!(subs.len(), 2);
}

#[tokio::test]
async fn unsubscribing_and_dropping_detach_every_feed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (drive, _) = session(dir.path());
    let registry = FeedRegistry::new();
    let (mut subs, mut rx) = Subscriptions::new(registry.clone(), drive);
    subs.handle(sub(1, Value::Null));
    subs.handle(sub(2, Value::Null));
    next(&mut rx).await;
    assert_eq!(registry.len(), 1);

    subs.handle(ClientMessage::Unsub(vec![1]));
    assert_eq!(subs.len(), 1);
    drop(subs);
    settle(&registry).await;
    assert!(registry.is_empty(), "no watcher outlives the socket's subscriptions");
}

#[tokio::test]
async fn a_missing_session_is_a_tagged_closed_and_a_ping_is_a_pong() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (drive, _) = session(dir.path());
    let (mut subs, mut rx) = Subscriptions::new(FeedRegistry::new(), drive);
    let missing = parse_client_message(r#"{"type":"sub","feeds":[{"sub":9,"source":"claude","id":"00000000-0000-4000-8000-000000000000"}]}"#)
        .expect("a sub");
    subs.handle(missing);
    let closed = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a frame in time")
        .expect("open");
    assert_eq!(closed, r#"{"sub":9,"type":"closed"}"#, "byte for byte: pages of any version read it");
    assert_eq!(subs.handle(ClientMessage::Ping), vec![r#"{"type":"pong"}"#.to_string()]);
}

#[tokio::test]
async fn a_resync_reaches_its_own_subscription_and_live_changes_arrive_tagged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (drive, file) = session(dir.path());
    let (mut subs, mut rx) = Subscriptions::new(FeedRegistry::new(), drive);
    subs.handle(sub(4, Value::Null));
    let snapshot = next(&mut rx).await;
    assert_eq!(snapshot["type"], "snapshot");
    next(&mut rx).await; // state

    let resync = serde_json::json!({ "type": "resync", "sub": 4, "since": 1000, "fp": snapshot["prefixFp"] }).to_string();
    subs.handle(parse_client_message(&resync).expect("a resync"));
    let answer = next(&mut rx).await;
    assert_eq!((answer["sub"].clone(), answer["type"].clone()), (Value::from(4), Value::from("delta")));

    let mut handle = std::fs::OpenOptions::new().append(true).open(&file).expect("open");
    std::io::Write::write_all(&mut handle, claude_turns(ID, 250, 1).as_bytes()).expect("append");
    drop(handle);
    loop {
        let frame = next(&mut rx).await;
        assert_eq!(frame["sub"], 4, "{frame}");
        if frame["type"] == "delta" {
            assert_eq!(frame["endIndex"], 1004, "the new turn, live");
            break;
        }
    }
}

#[tokio::test]
async fn a_request_the_server_will_not_start_is_answered_rather_than_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (drive, _) = session(dir.path());
    let (mut subs, _rx) = Subscriptions::new(FeedRegistry::new(), drive);
    let refusal = |text: &str| -> Value { serde_json::from_str(text).expect("json") };

    let batch = parse_client_message(
        r#"{"type":"sub","feeds":[{"sub":5,"source":"claude","id":"../x"},{"sub":6,"source":"claude","id":"55555555-5555-4555-8555-555555555555"}]}"#,
    )
    .expect("a sub");
    let replies = subs.handle(batch);
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(
        refusal(&replies[0]),
        serde_json::json!({ "sub": 5, "type": "closed", "reason": "invalid" })
    );
    assert_eq!(subs.len(), 1, "the good entry of the batch still stands");

    // A number at or below the highest this socket has seen is never a
    // new subscription: frames of the one it named may still be queued.
    for again in [6, 3] {
        let replies = subs.handle(sub(again, Value::Null));
        assert_eq!(
            refusal(&replies[0]),
            serde_json::json!({ "sub": again, "type": "closed", "reason": "reused" })
        );
    }
    assert_eq!(subs.len(), 1);

    for n in 7..(7 + MAX_SUBSCRIPTIONS as u64 - 1) {
        assert!(subs.handle(sub(n, Value::Null)).is_empty());
    }
    let over = 7 + MAX_SUBSCRIPTIONS as u64;
    let replies = subs.handle(sub(over, Value::Null));
    assert_eq!(
        refusal(&replies[0]),
        serde_json::json!({ "sub": over, "type": "closed", "reason": "limit" })
    );
}
