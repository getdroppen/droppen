//! Archive must use the live writer, not start a competing Codex process.
mod support;

use remote_hand_server::paths::Paths;
use remote_hand_server::sessions::{owned_sessions, Source};
use serde_json::json;
use support::Probe;

const ID: &str = "0199c1a1-2b3c-4d5e-8f60-112233445566";
const INSTANCE: &str = "archive-test";

async fn fixture() -> Probe {
    let probe = Probe::start_named(Some(INSTANCE)).await;
    let dir = probe.home.join(".codex/sessions/2026/09/14");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("rollout-2026-09-14T10-00-00-{ID}.jsonl")),
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","timestamp":"2026-09-14T10:00:00Z","payload":{"id":ID,"cwd":probe.workspace()}}),
            json!({"type":"event_msg","timestamp":"2026-09-14T10:00:01Z","payload":{"type":"agent_message","message":"ready"}}),
        ),
    )
    .unwrap();
    let paths = Paths::for_instance(probe.home.clone(), INSTANCE);
    owned_sessions::record(&paths, Source::Codex, ID).await.unwrap();
    probe
}

fn route(action: &str) -> String {
    format!("/api/sessions/codex/{ID}/{action}")
}

async fn start_resident(probe: &Probe) {
    probe.send_ok("POST", &route("message"), json!({"text":"test"}), 200).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let detail = probe.get_json(&format!("/api/sessions/codex/{ID}")).await;
        if detail["injection"]["state"] == "ok" && detail["summary"]["resident"]["phase"] == "idle" && detail["summary"]["resident"]["queued"] == 0 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "resident never became idle: {detail}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn assert_archived(probe: &Probe) {
    assert_eq!(probe.get_json("/api/sessions").await["total"], 0);
    assert_eq!(probe.get(&format!("/api/sessions/codex/{ID}")).await.0, 404);
    assert_eq!(probe.send("POST", &route("message"), json!({"text":"stale send"})).await.0, 404);
    assert!(probe
        .home
        .join(format!(".codex/archived_sessions/rollout-2026-09-14T10-00-00-{ID}.jsonl"))
        .is_file());
}

#[tokio::test]
async fn archive_routes_through_the_resident_and_removes_its_run() {
    let probe = fixture().await;
    start_resident(&probe).await;
    probe.send_ok("POST", &route("archive"), json!({}), 200).await;
    assert_archived(&probe).await;
    let requests = std::fs::read_to_string(probe.workspace().join("codex-stdin.jsonl")).unwrap();
    assert!(requests.contains("thread/archive"));
    assert!(!probe.home.join("codex-argv.bin").exists(), "no independent archive CLI was spawned");
    assert_eq!(
        std::fs::read_to_string(probe.workspace().join("codex-calls.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(support::common::rows(&probe.instance_db(), "SELECT run_id FROM runs", ()).is_empty());
    probe.stop().await;
}

#[tokio::test]
async fn archive_without_a_resident_uses_the_cli() {
    let probe = fixture().await;
    probe.send_ok("POST", &route("archive"), json!({}), 200).await;
    assert_archived(&probe).await;
    let argv = std::fs::read(probe.home.join("codex-argv.bin")).unwrap();
    assert_eq!(argv, format!("archive\0{ID}\0").as_bytes());
    probe.stop().await;
}

#[tokio::test]
async fn rejected_archive_keeps_the_resident_usable_and_can_be_retried() {
    let probe = fixture().await;
    start_resident(&probe).await;
    let reject = probe.workspace().join("archive-error.txt");
    std::fs::write(&reject, "reject").unwrap();
    let (status, body) = probe.send("POST", &route("archive"), json!({})).await;
    assert_eq!(status, 409);
    assert!(body["error"].as_str().unwrap().contains("archive fixture refused"));
    assert_eq!(probe.get_json("/api/sessions").await["total"], 1);
    start_resident(&probe).await;
    std::fs::remove_file(reject).unwrap();
    probe.send_ok("POST", &route("archive"), json!({}), 200).await;
    assert_archived(&probe).await;
    assert_eq!(
        std::fs::read_to_string(probe.workspace().join("codex-calls.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    probe.stop().await;
}

#[tokio::test]
async fn running_or_queued_work_is_not_archived() {
    let probe = fixture().await;
    support::plan_slow_cli(&probe.workspace(), 120.0);
    probe.send_ok("POST", &route("message"), json!({"text":"hold this turn"}), 200).await;
    assert_eq!(probe.send("POST", &route("archive"), json!({})).await.0, 409);
    probe.send_ok("POST", &route("message"), json!({"text":"keep queued"}), 200).await;
    let (status, body) = probe.send("POST", &route("archive"), json!({})).await;
    assert_eq!(status, 409);
    assert!(body["error"].as_str().unwrap().contains("queued messages"));
    let detail = probe.get_json(&format!("/api/sessions/codex/{ID}")).await;
    assert!(detail["summary"]["resident"]["queued"].as_u64().unwrap() >= 1);
    let requests = std::fs::read_to_string(probe.workspace().join("codex-stdin.jsonl")).unwrap();
    assert!(!requests.contains("thread/archive"));
    probe.stop().await;
}
