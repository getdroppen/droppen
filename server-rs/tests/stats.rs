//! `/api/stats/usage` and `/api/models`, against a synthetic home directory.
//!
//! The transcripts are stamped with *today's* UTC date rather than a fixed one:
//! the window ends today by construction, so a literal date would fall out of it
//! the day after it was written.

mod common;

use std::path::Path;
use std::time::SystemTime;

use common::{write_jsonl, TestServer};
use remote_hand_server::sessions::iso8601;
use serde_json::Value;

/// Today, UTC, as the aggregator computes it.
fn today() -> String {
    iso8601(SystemTime::now())[..10].to_string()
}

/// A JSON number, spelled out because the buckets are `number` in the contract
/// and serde renders them as floats — `body["x"] == 12` would compare against an
/// integer and fail.
fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| panic!("expected a number, got {value}"))
}

/// Two Claude assistant turns and one Codex `token_count`, all dated today.
fn seed_sessions(home: &Path) {
    let at = |time: &str| format!("{}T{time}.000Z", today());

    write_jsonl(
        &home.join(".claude/projects/proj/11111111-2222-3333-4444-555555555555.jsonl"),
        &[
            serde_json::json!({
                "type": "assistant",
                "timestamp": at("10:00:00"),
                "message": {
                    "model": "claude-fable-5",
                    "usage": { "input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 20 }
                }
            }),
            serde_json::json!({
                "type": "assistant",
                "timestamp": at("11:00:00"),
                "message": { "model": "claude-fable-5", "usage": { "input_tokens": 2, "output_tokens": 3 } }
            }),
            serde_json::json!({ "type": "user", "timestamp": at("09:00:00"), "message": { "content": "hi" } }),
        ],
    );

    let token_count = |time: &str| {
        serde_json::json!({
            "type": "event_msg",
            "timestamp": at(time),
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": { "total_tokens": 100 },
                    "last_token_usage": { "input_tokens": 60, "cached_input_tokens": 10, "output_tokens": 5 }
                }
            }
        })
    };
    write_jsonl(
        &home.join(".codex/sessions/2026/01/01/rollout-2026-01-01T12-00-00-66666666-7777-8888-9999-000000000000.jsonl"),
        &[
            serde_json::json!({ "type": "turn_context", "timestamp": at("12:00:00"), "payload": { "model": "gpt-5.6-sol" } }),
            token_count("12:01:00"),
            // A repeat of the same cumulative total must not be counted twice.
            token_count("12:02:00"),
        ],
    );
}

#[tokio::test]
async fn usage_stats_aggregate_both_clis_over_the_window() {
    let server = TestServer::start_with(seed_sessions).await;
    let body = server.get_json("/api/stats/usage").await;

    assert_eq!(body["windowDays"], 30);
    assert_eq!(body["since"], usage_since(30));
    assert_eq!(body["generatedAt"].as_str().expect("generatedAt").len(), 24);

    // claude: 10+2 fresh in, 5+3 out, 100 cache read, 20 cache write.
    // codex:  60-10 fresh in, 5 out, 10 cache read, no cache write.
    let totals = &body["totals"];
    assert_eq!(number(&totals["input"]), 62.0);
    assert_eq!(number(&totals["output"]), 13.0);
    assert_eq!(number(&totals["cacheRead"]), 110.0);
    assert_eq!(number(&totals["cacheWrite"]), 20.0);
    assert_eq!(number(&totals["total"]), 205.0);
    assert_eq!(totals["sessions"], 2, "one file per CLI contributed");
    // 12 in @ $10 + 8 out @ $50 + 100 read @ $1 + 20 write @ $12.5, per 1M.
    assert!((number(&totals["costUSD"]) - 0.000_87).abs() < 1e-9, "{totals}");

    let daily = body["daily"].as_array().expect("daily array");
    assert_eq!(daily.len(), 30, "every calendar day in the window is present");
    assert_eq!(daily[0]["date"], usage_since(30));
    let last = daily.last().expect("today's row");
    assert_eq!(last["date"], today());
    assert_eq!(number(&last["total"]), 205.0, "all of it landed today");

    let by_model = body["byModel"].as_array().expect("byModel array");
    assert_eq!(by_model.len(), 2);
    assert_eq!(by_model[0]["model"], "claude-fable-5", "largest total first");
    assert_eq!(by_model[0]["source"], "claude");
    assert_eq!(number(&by_model[0]["total"]), 140.0);
    assert_eq!(by_model[1]["model"], "gpt-5.6-sol");
    assert_eq!(by_model[1]["cost"], Value::Null, "Codex models are deliberately unpriced");

    let by_source = body["bySource"].as_array().expect("bySource array");
    assert_eq!(by_source.len(), 2);
    assert_eq!(by_source[0]["source"], "claude");
    assert_eq!(number(&by_source[0]["total"]), 140.0);
    assert_eq!(by_source[0]["sessions"], 1);
    assert_eq!(by_source[1]["source"], "codex");
    assert_eq!(number(&by_source[1]["total"]), 65.0);
    assert_eq!(number(&by_source[1]["cost"]), 0.0);

    assert_eq!(body["pricing"]["unpricedModels"], serde_json::json!(["codex:gpt-5.6-sol"]));
    assert_eq!(
        body["pricing"]["overridePath"],
        server.paths.price_override().to_string_lossy().into_owned(),
        "the override path is rooted at the server's home, not the user's"
    );

    server.stop().await;
}

/// The first day of an `n`-day window ending today, as the response spells it.
fn usage_since(days: i64) -> String {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("after the epoch");
    let midnight = (now.as_millis() as i64 / 86_400_000) * 86_400_000;
    let cutoff = midnight - (days - 1) * 86_400_000;
    iso8601(SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(cutoff as u64))[..10].to_string()
}

#[tokio::test]
async fn the_usage_window_is_clamped_and_honours_days() {
    let server = TestServer::start_with(seed_sessions).await;

    let week = server.get_json("/api/stats/usage?days=7").await;
    assert_eq!(week["windowDays"], 7);
    assert_eq!(week["daily"].as_array().expect("daily").len(), 7);

    let clamped = server.get_json("/api/stats/usage?days=9999").await;
    assert_eq!(clamped["windowDays"], 365);
    assert_eq!(clamped["daily"].as_array().expect("daily").len(), 365);

    server.stop().await;
}

#[tokio::test]
async fn an_empty_home_reports_an_empty_window() {
    let server = TestServer::start().await;
    let body = server.get_json("/api/stats/usage").await;

    assert_eq!(number(&body["totals"]["total"]), 0.0);
    assert_eq!(body["totals"]["sessions"], 0);
    assert_eq!(body["byModel"].as_array().expect("byModel").len(), 0);
    assert_eq!(body["bySource"].as_array().expect("bySource").len(), 0, "a silent source is dropped");
    assert_eq!(body["daily"].as_array().expect("daily").len(), 30, "the timeline still has no gaps");

    server.stop().await;
}

#[tokio::test]
async fn codex_models_fall_back_when_no_cache_exists() {
    let server = TestServer::start().await;
    let body = server.get_json("/api/models?source=codex").await;

    let models = body["models"].as_array().expect("models array");
    let ids: Vec<&str> = models.iter().map(|model| model["id"].as_str().expect("id")).collect();
    assert_eq!(ids, vec!["gpt-5.6-sol", "gpt-5.5", "gpt-5.4"]);
    assert_eq!(models[0]["label"], Value::Null, "the fallback publishes nothing but ids");
    assert_eq!(models[0]["efforts"], Value::Null);
    assert_eq!(models[0]["defaultEffort"], Value::Null);
    assert_eq!(body["source"], "fallback", "and the list says it is a guess");

    server.stop().await;
}

/// The Claude list is whatever the local CLI publishes, and is sendable either
/// way.
///
/// It cannot be pinned to one expected list here: the models are read from the
/// CLI installed on the machine running the test, which is a real answer on a
/// developer's laptop and no CLI at all in CI. Both shapes are asserted for what
/// they promise — the fallback is exactly the tier aliases, a real answer labels
/// every alias with the concrete model it resolves to — and both are held to the
/// rule that matters: every id offered here is an id the send routes accept,
/// which is what `claude-opus-5[1m]` and its bracketed tag exist to test.
#[tokio::test]
async fn the_claude_list_is_the_cli_s_own_and_every_id_in_it_is_sendable() {
    let server = TestServer::start().await;
    let body = server.get_json("/api/models?source=claude").await;

    let models = body["models"].as_array().expect("models array");
    assert!(!models.is_empty(), "a picker is never empty");

    if body["source"] == "fallback" {
        let ids: Vec<&str> = models.iter().map(|model| model["id"].as_str().expect("id")).collect();
        assert_eq!(ids, vec!["fable", "opus", "sonnet", "haiku"], "the static guess is the tier aliases");
        assert_eq!(models[0]["label"], "fable (latest)");
        assert_eq!(models[0]["defaultEffort"], "medium");
        assert_eq!(models[0]["efforts"], serde_json::json!(["low", "medium", "high", "xhigh", "max"]));
        let haiku = models.iter().find(|model| model["id"] == "haiku").expect("the haiku alias");
        assert_eq!(haiku["efforts"], serde_json::json!([]), "haiku takes no effort at all");
        assert_eq!(haiku["defaultEffort"], Value::Null, "and so has no rung to default to");
    } else {
        assert_eq!(body["source"], "cache", "there are only two answers");
        let mut labels: Vec<&str> = Vec::new();
        for model in models {
            let label = model["label"].as_str().expect("a CLI answer names the model each alias resolves to");
            assert!(label.starts_with("claude-"), "{label} is a concrete model id");
            assert!(
                !labels.contains(&label),
                "{label} is offered twice, which is two rows a user cannot tell apart"
            );
            labels.push(label);
        }
    }

    // The end of the line for an offered id: the route a composer pick is stored
    // through runs the same pattern the send routes do.
    for model in models {
        let id = model["id"].as_str().expect("id");
        let (status, reply) = server
            .json("PUT", "/api/composer-prefs/claude", &serde_json::json!({ "model": id }))
            .await;
        assert_eq!(status, 200, "the picker offers {id}, which the send routes refused: {reply}");
    }

    server.stop().await;
}

/// An `accountId` nothing can be asked of is a fallback, not a failure.
///
/// The list is per login now, so the query names one — and a picker that got a
/// `500` because the account was deleted between page load and model fetch would
/// be a worse answer than the static aliases. Both shapes of "not askable" go
/// the same way: an id that is not a uuid never reaches the store, and a uuid
/// that names nothing is simply absent from it.
#[tokio::test]
async fn an_account_that_cannot_be_asked_falls_back_rather_than_failing() {
    let server = TestServer::start().await;

    for account in ["not-a-uuid", "11111111-2222-3333-4444-555555555555"] {
        let body = server.get_json(&format!("/api/models?source=claude&accountId={account}")).await;
        assert_eq!(body["source"], "fallback", "{account} is not probeable, so the list says it is a guess");
        let ids: Vec<&str> = body["models"]
            .as_array()
            .expect("models array")
            .iter()
            .map(|model| model["id"].as_str().expect("id"))
            .collect();
        assert_eq!(ids, vec!["fable", "opus", "sonnet", "haiku"], "and it is the tier aliases");
    }

    // An empty value is the local login spelled the long way, which is the
    // answer the route gave before it took an account at all.
    let explicit = server.get_json("/api/models?source=claude&accountId=").await;
    let implicit = server.get_json("/api/models?source=claude").await;
    assert_eq!(explicit["models"], implicit["models"], "an empty accountId is the local login");
    assert_eq!(explicit["source"], implicit["source"]);

    server.stop().await;
}
