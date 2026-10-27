//! The two storage subsystems (prefs and screens), end to end against a
//! throwaway home directory.
//!
//! This file used to be about bytes. Every store wrote a JSON file that the
//! TypeScript server read out of the same `~/.config/remote-hand`, so the key
//! order and the trailing newline were a compatibility contract and the tests
//! pinned them character by character. Both halves of that premise are gone: the
//! TypeScript server was deleted, and the stores write only to SQLite.
//!
//! So what is proved here now is the thing the byte assertions were a proxy for
//! — that a setting a client sends is *still there* when it asks again, and that
//! it is there because it was written down rather than because one process
//! remembered it. That is asked through the API wherever the API can answer it,
//! and through the database where it cannot. The JSON files keep exactly one
//! job, and it is tested below too: an install upgrading from an older build is
//! seeded from the file it already had, once.

mod common;

use remote_hand_server::db::scope;
use serde_json::{json, Value};

#[tokio::test]
async fn prefs_round_trip_and_leave_no_file_behind() {
    let server = common::TestServer::start().await;

    // Nothing has been written yet, so every field reads as its default.
    let defaults = server.get_json("/api/prefs").await;
    assert_eq!(
        defaults,
        json!({
            "sessionGrouping": "flat",
            "composerSend": "enter",
            "fontSize": "default",
            "language": "auto",
            "transcriptStyle": "flat",
            "math": "rendered",
        })
    );

    let wanted = json!({
        "sessionGrouping": "directory",
        "composerSend": "modifier",
        "fontSize": "xlarge",
        "language": "zh-CN",
        "transcriptStyle": "bubble",
        "math": "source",
    });
    let (status, body) = server.json("PUT", "/api/prefs", &wanted).await;
    assert_eq!(status, 200, "PUT /api/prefs -> {status}: {body}");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json"), wanted);

    assert_eq!(server.get_json("/api/prefs").await, wanted, "the write is what the next read sees");

    // One row per setting, under the name the wire uses. Read from the database
    // rather than from the handler a second time, because "the write reached the
    // store" and "the process remembers what it was told" are different claims
    // and only the first one survives a restart.
    let stored = common::rows(
        &server.paths.config_db(),
        "SELECT key || '=' || value FROM kv WHERE scope = ?1 ORDER BY key",
        (scope::PREFS,),
    );
    assert_eq!(
        stored,
        [
            "composerSend=modifier",
            "fontSize=xlarge",
            "language=zh-CN",
            "math=source",
            "sessionGrouping=directory",
            "transcriptStyle=bubble",
        ]
    );

    // And nothing wrote the file back. That is the inversion of the byte
    // assertion this test used to make, and it is worth keeping as an assertion
    // rather than a comment: `prefs.json` is an input to the one-time import, so
    // a save that rewrote it is a save a hand-restored file could not survive.
    assert!(!server.paths.prefs_file().exists(), "no file is created where there was none");

    server.stop().await;
}

#[tokio::test]
async fn an_unreadable_field_costs_only_itself() {
    let server = common::TestServer::start_with(|home| {
        let config = home.join(".config/remote-hand");
        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::write(
            config.join("prefs.json"),
            r#"{"sessionGrouping":"directory","fontSize":"gigantic","language":"kl"}"#,
        )
        .expect("seed prefs");
    })
    .await;

    let prefs = server.get_json("/api/prefs").await;
    assert_eq!(prefs["sessionGrouping"], "directory", "a readable field survives");
    assert_eq!(prefs["fontSize"], "default", "an unknown value falls back on its own");
    assert_eq!(prefs["language"], "auto");
    assert_eq!(prefs["composerSend"], "enter", "an absent field is a default");

    server.stop().await;
}

#[tokio::test]
async fn screens_are_created_patched_listed_and_deleted() {
    let server = common::TestServer::start().await;

    assert_eq!(server.get_json("/api/screens").await, json!({ "screens": [], "listRevision": 1 }));

    let (status, body) = server.json("POST", "/api/screens", &json!({})).await;
    assert_eq!(status, 200, "POST /api/screens -> {status}: {body}");
    let created: Value = serde_json::from_str(&body).expect("json");
    let id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["title"], "screen 1", "an unnamed screen is numbered");
    assert_eq!(created["layout"]["type"], "leaf", "a new screen holds one empty pane");
    assert_eq!(created["layout"]["content"], json!({ "kind": "picker" }));
    assert_eq!(created["createdAt"], created["updatedAt"]);

    // A layout the client could plausibly send, plus one node the server has
    // never heard of — the whole tree is rebuilt, so the stray node cannot land
    // in the file.
    let patch = json!({
        "baseRevision": created["revision"],
        "title": "  work  ",
        "layout": {
            "type": "split",
            "id": "root",
            "direction": "column",
            "ratio": 42,
            "children": [
                { "type": "leaf", "id": "left", "content": { "kind": "terminal", "id": "t-1" } },
                { "type": "leaf", "id": "right", "content": { "kind": "hologram", "id": "h-1" } },
            ],
        },
    });
    let (status, body) = server.json("PATCH", &format!("/api/screens/{id}"), &patch).await;
    assert_eq!(status, 200, "PATCH /api/screens/{id} -> {status}: {body}");
    let updated: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(updated["title"], "work", "the title is trimmed");
    assert_eq!(updated["layout"]["direction"], "column");
    assert_eq!(updated["layout"]["ratio"], 0.9, "an out-of-range ratio is clamped");
    assert_eq!(updated["layout"]["children"][0]["content"], json!({ "kind": "terminal", "id": "t-1" }));
    assert_eq!(
        updated["layout"]["children"][1]["content"],
        json!({ "kind": "picker" }),
        "an unknown pane kind degrades instead of being stored"
    );
    assert_eq!(updated["createdAt"], created["createdAt"], "creation time is not rewritten");

    let listed = server.get_json("/api/screens").await;
    assert_eq!(listed["screens"].as_array().expect("array").len(), 1);
    assert_eq!(listed["screens"][0], updated, "the list is what the patch returned");

    // The list above is this server answering; this is the layout being on disk
    // under the id the client was given. The `{"version": 1, "screens": [...]}`
    // envelope the file carried has no counterpart here and needs none — it
    // existed so a second program could tell which shape it was reading, and
    // there is no second program.
    assert_eq!(
        common::rows(&server.paths.instance_db(), "SELECT id FROM screens", ()),
        std::slice::from_ref(&id)
    );

    let (status, body) = server
        .json("PATCH", "/api/screens/does-not-exist", &json!({ "title": "x", "baseRevision": 1 }))
        .await;
    assert_eq!(status, 404);
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "screen not found");

    let (status, body) = server.request("DELETE", &format!("/api/screens/{id}"), &[], &[]).await;
    assert_eq!(status, 200, "DELETE /api/screens/{id} -> {status}: {body}");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json"), json!({ "accepted": true }));

    let (status, body) = server.request("DELETE", &format!("/api/screens/{id}"), &[], &[]).await;
    assert_eq!(status, 404, "a second delete has nothing left to remove");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "screen not found");

    assert_eq!(server.get_json("/api/screens").await, json!({ "screens": [], "listRevision": 3 }));

    server.stop().await;
}

#[tokio::test]
async fn screen_emoji_is_persistent_independent_metadata_and_can_be_reset() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = common::TestServer::start_at(home.path(), None).await;
    let (status, body) = server.json("POST", "/api/screens", &json!({ "title": "Work" })).await;
    assert_eq!(status, 200);
    let created: Value = serde_json::from_str(&body).expect("json");
    assert!(created.get("emoji").is_none(), "new screens use the grid icon");
    let path = format!("/api/screens/{}", created["id"].as_str().expect("id"));
    let (_, sibling_body) = server.json("POST", "/api/screens", &json!({ "title": "Personal" })).await;
    let sibling: Value = serde_json::from_str(&sibling_body).expect("json");

    let (status, body) = server
        .json("PATCH", &path, &json!({ "emoji": " 👩🏽‍💻 ", "baseRevision": created["revision"] }))
        .await;
    assert_eq!(status, 200, "{body}");
    let updated: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(updated["emoji"], "👩🏽‍💻");
    assert_eq!(updated["title"], created["title"]);
    assert_eq!(updated["layout"], created["layout"]);
    assert_eq!(updated["createdAt"], created["createdAt"]);

    let layout = json!({ "type": "leaf", "id": "files", "content": { "kind": "files", "path": "/tmp" } });
    let (status, body) = server
        .json(
            "PATCH",
            &path,
            &json!({ "title": "Renamed", "layout": layout, "baseRevision": updated["revision"] }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let updated: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(updated["emoji"], "👩🏽‍💻", "layout and title edits retain the icon");
    assert_eq!(updated["layout"], layout);
    assert_eq!(server.get_json("/api/screens").await["screens"], json!([updated, sibling]));

    server.stop().await;
    let server = common::TestServer::start_at(home.path(), None).await;
    assert_eq!(
        server.get_json("/api/screens").await["screens"],
        json!([updated, sibling]),
        "restart retains the icon"
    );

    let (status, body) = server
        .json("PATCH", &path, &json!({ "emoji": "", "baseRevision": updated["revision"] }))
        .await;
    assert_eq!(status, 200, "{body}");
    let reset: Value = serde_json::from_str(&body).expect("json");
    assert!(reset.get("emoji").is_none(), "blank restores the grid icon");
    assert_eq!(reset["title"], "Renamed");
    assert_eq!(reset["layout"], layout);
    assert_eq!(server.get_json("/api/screens").await["screens"], json!([reset, sibling]));
    server.stop().await;
}

#[tokio::test]
async fn invalid_screen_emoji_cannot_modify_the_screen() {
    let server = common::TestServer::start().await;
    let (_, body) = server.json("POST", "/api/screens", &json!({ "title": "Work" })).await;
    let created: Value = serde_json::from_str(&body).expect("json");
    let path = format!("/api/screens/{}", created["id"].as_str().expect("id"));
    let (status, body) = server
        .json("PATCH", &path, &json!({ "emoji": "🐙", "baseRevision": created["revision"] }))
        .await;
    assert_eq!(status, 200, "{body}");
    let before: Value = serde_json::from_str(&body).expect("json");
    for emoji in [
        json!("work"),
        json!("🐙🚀"),
        json!("🇯"),
        json!("x".repeat(1000)),
        json!(42),
        json!(["🐙"]),
    ] {
        let (status, body) = server
            .json(
                "PATCH",
                &path,
                &json!({ "emoji": emoji, "title": "Must not change", "baseRevision": before["revision"] }),
            )
            .await;
        assert_eq!(status, 400, "{emoji}: {body}");
        assert_eq!(server.get_json("/api/screens").await["screens"][0], before);
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn screen_emoji_and_layout_autosave_do_not_overwrite_each_other() {
    let server = common::TestServer::start().await;
    let (_, body) = server.json("POST", "/api/screens", &json!({})).await;
    let created: Value = serde_json::from_str(&body).expect("json");
    let path = format!("/api/screens/{}", created["id"].as_str().expect("id"));
    for round in 0..12 {
        let revision = server.get_json("/api/screens").await["screens"][0]["revision"].clone();
        let icon = json!({ "emoji": if round % 2 == 0 { "🐙" } else { "🚀" }, "baseRevision": revision });
        let layout = json!({ "type": "leaf", "id": format!("pane-{round}"), "content": { "kind": "picker" } });
        let autosave = json!({ "title": format!("Work {round}"), "layout": layout, "baseRevision": revision });
        let (emoji_result, layout_result) = tokio::join!(server.json("PATCH", &path, &icon), server.json("PATCH", &path, &autosave),);
        let (winner, conflict, mut retry) = match (emoji_result.0, layout_result.0) {
            (200, 409) => (emoji_result.1, layout_result.1, autosave.clone()),
            (409, 200) => (layout_result.1, emoji_result.1, icon.clone()),
            _ => panic!("one save must succeed and the stale save must conflict: {emoji_result:?}, {layout_result:?}"),
        };
        let winner: Value = serde_json::from_str(&winner).expect("winner");
        let conflict: Value = serde_json::from_str(&conflict).expect("conflict");
        assert_eq!(conflict["error"], "revision_conflict");
        assert_eq!(conflict["screen"], winner, "the retry uses the current screen, not stale metadata");
        retry["baseRevision"] = winner["revision"].clone();
        let (status, body) = server.json("PATCH", &path, &retry).await;
        assert_eq!(status, 200, "{body}");
        let screens = server.get_json("/api/screens").await;
        assert_eq!(screens["screens"][0]["emoji"], icon["emoji"]);
        assert_eq!(screens["screens"][0]["title"], autosave["title"]);
        assert_eq!(screens["screens"][0]["layout"], layout);
    }
    server.stop().await;
}

/// What replaced the test that pinned `screens.json`'s key order and its
/// trailing newline: the layout is a row, and the file is not written at all.
///
/// Worth an assertion rather than a deletion, because "no file" is the whole of
/// the new contract. A layout is a row and nothing writes `screens.json`; a save
/// that put one back would quietly re-establish two answers to one question —
/// with the file, as ever, being the one nothing reads and everything believes.
#[tokio::test]
async fn a_saved_layout_is_a_row_and_the_file_stays_unwritten() {
    let server = common::TestServer::start().await;
    let (status, body) = server.json("POST", "/api/screens", &json!({ "title": "layout" })).await;
    assert_eq!(status, 200, "POST /api/screens -> {status}: {body}");

    let stored = common::json_rows(&server.paths.instance_db(), "SELECT payload FROM screens", ());
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["title"], "layout");
    assert_eq!(
        stored[0]["layout"]["type"], "leaf",
        "the sanitized tree is what was stored, not the request"
    );

    let file = server.paths.instance_dir().join("screens.json");
    assert!(!file.exists(), "a save creates no file where there was none");
    assert!(!file.with_extension("json.tmp").exists());

    server.stop().await;
}

/// Layouts live in the instance database now. A database seeded before boot is
/// what `GET /api/screens` serves, and the `screens.json` an install older than
/// 0.2.125 keeps in the shared config directory is neither copied nor read.
#[tokio::test]
async fn boot_serves_the_instance_database_and_ignores_the_old_shared_file() {
    let payload = json!({
        "id": "seeded",
        "title": "seeded layout",
        "layout": { "type": "leaf", "id": "l1", "content": { "kind": "picker" } },
        "createdAt": "2026-01-01T00:00:00.000Z",
        "updatedAt": "2026-01-01T00:00:00.000Z",
    });
    let legacy = json!({
        "version": 1,
        "screens": [{
            "id": "screen-inherited",
            "title": "from the shared file",
            "layout": { "type": "leaf", "id": "l2", "content": { "kind": "picker" } },
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        }],
    });
    let body = format!("{}\n", serde_json::to_string_pretty(&legacy).expect("json"));
    let shared_body = body.clone();
    let server = common::TestServer::start_with(move |home| {
        let paths = remote_hand_server::sessions::Paths::new(home.to_path_buf());
        remote_hand_server::db::at(paths.instance_db())
            .write(|tx| {
                tx.execute(
                    "INSERT INTO screens (id, ordinal, payload) VALUES ('seeded', 0, ?1)",
                    (payload.to_string(),),
                )
            })
            .expect("seed the instance database");
        let config = home.join(".config").join("remote-hand");
        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::write(config.join("screens.json"), shared_body).expect("seed the old shared file");
    })
    .await;

    let listed = server.get_json("/api/screens").await;
    assert_eq!(listed["screens"].as_array().expect("array").len(), 1);
    assert_eq!(listed["screens"][0]["id"], "seeded");
    assert_eq!(listed["screens"][0]["title"], "seeded layout");

    // Deleting through the API touches the row and nothing else: the old file
    // is inert, because an install older than the floor is not migrated.
    let (status, _) = server.json("DELETE", "/api/screens/seeded", &json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(server.get_json("/api/screens").await, json!({ "screens": [], "listRevision": 2 }));
    let shared = server.home.join(".config").join("remote-hand").join("screens.json");
    assert_eq!(
        std::fs::read_to_string(&shared).expect("shared screens.json"),
        body,
        "the old shared file is left exactly as it was found"
    );

    server.stop().await;
}

#[tokio::test]
async fn the_spec_documents_the_storage_routes_and_their_statuses() {
    let server = common::TestServer::start().await;
    let spec = server.get_json("/openapi.json").await;

    let schemas = &spec["components"]["schemas"];
    for name in ["Prefs", "Screen", "ScreenList", "ScreenCreate", "ScreenUpdate", "SendResult"] {
        assert!(schemas[name].is_object(), "{name} is missing from components/schemas");
    }

    // `Screen.layout` is `z.any()`: always serialized, never required.
    let screen = &schemas["Screen"];
    let required: Vec<&str> = screen["required"]
        .as_array()
        .expect("required")
        .iter()
        .map(|v| v.as_str().expect("name"))
        .collect();
    assert_eq!(
        required,
        ["id", "title", "createdAt", "updatedAt", "revision"],
        "layout must stay out of `required`"
    );
    assert!(screen["properties"]["layout"].is_object(), "layout is still documented");

    let prefs_put = &spec["paths"]["/api/prefs"]["put"];
    assert_eq!(prefs_put["responses"]["200"]["description"], "Updated preferences");
    assert_eq!(prefs_put["responses"]["default"], Value::Null, "the derived `default` should be gone");

    assert!(spec["paths"]["/api/init-script"].is_null(), "the boot hook route is gone");

    let patch = &spec["paths"]["/api/screens/{id}"]["patch"];
    assert_eq!(patch["responses"]["400"]["description"], "Invalid screen emoji, or no baseRevision");
    assert_eq!(patch["responses"]["404"]["description"], "Screen not found");
    assert_eq!(patch["responses"]["default"], Value::Null);
    assert_eq!(
        spec["paths"]["/api/screens/{id}"]["delete"]["responses"]["200"]["description"],
        "Screen deleted; referenced resources are left untouched"
    );

    server.stop().await;
}
