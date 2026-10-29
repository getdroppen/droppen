//! The aggregator gateway (`droppen_cloud::gateway`) as the server serves it.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gotcha::axum::extract::FromRef;
use gotcha::State;
use serde_json::{json, Value};

use droppen_aggregator::live::Key;
use droppen_aggregator::store::Kind;
use droppen_auth::MaybeIdentity;
use droppen_cloud::CloudState;

/// The `bye` a page that may no longer see other devices is sent, byte
/// for byte.
const NO_REMOTE_ACCESS_BYE: &str = r#"{"t":"bye","code":"no_remote_access","message":"this local account may not see your other devices here: the local account that signed this device in to Droppen can allow it in Settings → Droppen"}"#;

/// The gateway end to end, in process: refused with no local account,
/// to a local account that may not see other devices, and closed (`bye`)
/// the moment the owner takes that back; to the owner, the directory and
/// the copies as they change.
#[tokio::test]
async fn the_gateway_serves_only_the_local_accounts_that_may_see_other_devices() {
    use tokio_tungstenite::tungstenite::{self, Message as WsMessage};
    type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
    async fn next_text(ws: &mut Ws) -> String {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("a frame in time") {
            Some(Ok(WsMessage::Text(t))) => t.as_str().to_string(),
            other => panic!("{other:?}"),
        }
    }
    async fn next(ws: &mut Ws) -> Value {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("a frame in time") {
            Some(Ok(WsMessage::Text(t))) => serde_json::from_str::<Value>(t.as_str()).unwrap(),
            Some(Ok(WsMessage::Close(frame))) => json!({"closed": frame.map(|f| u16::from(f.code))}),
            other => panic!("{other:?}"),
        }
    }
    let home = tempfile::tempdir().unwrap();
    let (state, addr) = crate::api::test_support::served(home.path()).await;
    let connect = |ticket: Option<String>| async move {
        let url = match ticket {
            Some(t) => format!("ws://{addr}/api/agg/ws?ticket={t}"),
            None => format!("ws://{addr}/api/agg/ws"),
        };
        tokio_tungstenite::connect_async(url).await.map(|(ws, _)| ws)
    };
    let status_of = |r: Result<Ws, tungstenite::Error>| match r {
        Err(tungstenite::Error::Http(res)) => res.status().as_u16(),
        Err(other) => panic!("not refused over HTTP: {other}"),
        Ok(_) => panic!("let in"),
    };
    // No local account and no session: the setup state, and nobody gets
    // in for connecting from this device.
    assert_eq!(status_of(connect(None).await), 401);
    let auth = state.drive.auth.clone();
    for user in ["alice", "bob"] {
        auth.users.add(user, "correct horse battery staple").await.unwrap();
    }
    // A login and the identity it gives, and a `ws` ticket minted by it (as `POST /api/auth/ticket` does).
    let login = |user: &'static str| {
        let auth = auth.clone();
        async move {
            let digest = droppen_auth::sessions::digest_of(&auth.sessions.issue(user, None).await.unwrap().token);
            let who = MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
                username: user.into(),
                token_sha256: Some(digest.clone()),
            }));
            (digest, who)
        }
    };
    let ticket_for = |user: &str, digest: &str| Some(auth.tickets.issue(user, digest, droppen_auth::tickets::TicketScope::Ws).unwrap().0);
    let (alice_login, alice) = login("alice").await;
    let (bob_login, _) = login("bob").await;
    let ticket = |user: &str| ticket_for(user, if user == "alice" { &alice_login } else { &bob_login });
    // alice signed this device in: she sees the other devices, and says who else may.
    let created: std::collections::BTreeMap<String, String> = auth.users.list().await.into_iter().map(|u| (u.name, u.created_at)).collect();
    std::fs::write(
        droppen_cloud::LinkFile::of(&state.paths).path(),
        serde_json::json!({"owner": "alice", "aggregate": true, "linkedAt": 1, "accountsSince": {"alice": created["alice"]}}).to_string(),
    )
    .unwrap();
    // A ticket whose login is not one (nothing live to check it against): refused.
    let unbound = Some(
        auth.tickets
            .issue("alice", "no-such-login", droppen_auth::tickets::TicketScope::Ws)
            .unwrap()
            .0,
    );
    assert_eq!(status_of(connect(unbound).await), 401);
    // Without a ticket, with auth on: nobody.
    assert_eq!(status_of(connect(None).await), 401);
    let mut a = connect(ticket("alice")).await.expect("alice is let in");
    let dir = next(&mut a).await;
    assert_eq!((dir["t"].as_str(), dir["machines"].as_array().map(Vec::len)), (Some("dir"), Some(0)));
    // A device appears, with its index: the directory, then the copy.
    let live = state.cloud.control.live().clone();
    live.set_hosts(&["m-2".into()]);
    let dir = next(&mut a).await;
    assert_eq!(dir["machines"][0]["id"], "m-2");
    let mut index = rh_client::streams::IndexCopy::default();
    index
        .rows
        .insert("claude/q".into(), json!({"source": "claude", "id": "q", "asking": true}));
    let slot = droppen_aggregator::live::index_slot(index, 4, 1_700_000_000_000);
    live.publish(&Key::new("m-2", Kind::Index, ""), 0, &slot);
    let dir = next(&mut a).await;
    assert_eq!(dir["machines"][0]["copyAsOfMs"], 1_700_000_000_000u64);
    a.send(WsMessage::Text(r#"{"t":"sub","s":1,"machine":"m-2","name":{"kind":"index"}}"#.into()))
        .await
        .unwrap();
    let copy = next(&mut a).await;
    assert_eq!((copy["t"].as_str(), copy["reset"].as_bool()), (Some("copy"), Some(true)));
    assert_eq!(copy["bodies"][0]["upserts"][0]["asking"], true);
    a.send(WsMessage::Text(
        r#"{"t":"open","p":1,"machine":"m-2","kind":"rpc","path":"/api/sessions"}"#.into(),
    ))
    .await
    .unwrap();
    assert_eq!(next(&mut a).await["code"], "unsupported");
    // bob may not see other devices.
    assert_eq!(status_of(connect(ticket("bob")).await), 403);
    // alice lets him, then takes it back: he is told, and closed.
    let set = |remote: bool| {
        droppen_cloud::put_access(
            State(CloudState::from_ref(&state)),
            alice.clone(),
            droppen_http::extract::ApiJson(droppen_cloud::AccessRequest { user: "bob".into(), remote }),
        )
    };
    let _ = set(true).await.unwrap();
    let mut b = connect(ticket("bob")).await.expect("bob is let in now");
    assert_eq!(next(&mut b).await["machines"][0]["id"], "m-2");
    let t0 = std::time::Instant::now();
    let _ = set(false).await.unwrap();
    assert_eq!(next_text(&mut b).await, NO_REMOTE_ACCESS_BYE);
    assert_eq!(next(&mut b).await["closed"], 4403);
    assert!(t0.elapsed() < Duration::from_secs(1), "closed {:?} after", t0.elapsed());
    // alice stays.
    a.send(WsMessage::Text(r#"{"t":"ping"}"#.into())).await.unwrap();
    assert_eq!(next(&mut a).await["t"], "pong");
    // Review of S1-7: a page opened with a login that is then signed out
    // is closed with it.
    let (digest, _) = login("alice").await;
    let mut signed_in = connect(ticket_for("alice", &digest)).await.expect("alice's login is let in");
    assert_eq!(next(&mut signed_in).await["t"], "dir");
    assert!(auth.sessions.revoke(&digest).await);
    assert_eq!(
        next_text(&mut signed_in).await,
        r#"{"t":"bye","code":"unauthorized","message":"this login was signed out"}"#
    );
    assert_eq!(next(&mut signed_in).await["closed"], 4401);
    // The page opened by a login still signed in stays.
    a.send(WsMessage::Text(r#"{"t":"ping"}"#.into())).await.unwrap();
    assert_eq!(next(&mut a).await["t"], "pong");
    // No owner on file (a device signed in with the CLI): nobody sees the
    // other devices, not even who may manage the link, until one claims it.
    std::fs::write(
        droppen_cloud::LinkFile::of(&state.paths).path(),
        serde_json::json!({"aggregate": true, "linkedAt": 1}).to_string(),
    )
    .unwrap();
    // (Written behind the server's back: the next recheck sees it.)
    assert_eq!(next_text(&mut a).await, NO_REMOTE_ACCESS_BYE);
    assert_eq!(status_of(connect(ticket("alice")).await), 403);
    // A file that does not read: nobody either.
    std::fs::write(droppen_cloud::LinkFile::of(&state.paths).path(), "{not json").unwrap();
    assert_eq!(status_of(connect(ticket("alice")).await), 403);
}
