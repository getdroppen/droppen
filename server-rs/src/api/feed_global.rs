//! The session-list socket (`crate::feed::global`) as the server serves it:
//! what a cloud subscriber hears, when a login's sockets close, and a client
//! that never reads.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::time::Instant;

use crate::api::AppState;
use crate::feed::global::LOGIN_RECHECK;

/// A bridged subscriber is droppen.app, and plugins are this computer's
/// own (`docs/plugins/runtime.md` §5): its `settings` frame says there are
/// none, and the `plugins` / `plugin-doc` frames a local page hears never
/// reach it. A local page hears both, as it always did.
#[tokio::test]
async fn a_cloud_subscriber_hears_nothing_of_the_plugins() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    /// The next JSON frame of a feed socket, skipping the keepalive pings.
    async fn next_frame(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    ) -> serde_json::Value {
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), socket.next())
                .await
                .expect("a frame in time")
            {
                Some(Ok(WsMessage::Text(text))) => return serde_json::from_str(&text).expect("a JSON frame"),
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => continue,
                other => panic!("{other:?}"),
            }
        }
    }

    let home = tempfile::tempdir().unwrap();
    let (state, addr) = crate::api::test_support::served_with(home.path(), |state| {
        // One plugin this instance has, so `settings` differs visibly
        // between the two subscribers.
        state.plugins = Arc::new(droppen_plugins::testing::installed_host(
            home.path(),
            &["tasks"],
            droppen_http::config::PluginSettings::default(),
            None,
        ));
    })
    .await;

    let mut request = format!("ws://{addr}/api/events").into_client_request().expect("a request");
    request.headers_mut().insert(
        droppen_http::bridge::CREDENTIAL_HEADER,
        state
            .drive
            .auth
            .internal_credential()
            .to_string()
            .parse()
            .expect("a header value"),
    );
    request.headers_mut().insert(
        droppen_http::bridge::DEVICE_HEADER,
        "d-1".parse().expect("a header value"),
    );
    let (mut cloud, _) = tokio_tungstenite::connect_async(request).await.expect("a bridged socket");

    let ticket = crate::api::test_support::owner_ticket(&state).await;
    let (mut local, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/events?ticket={ticket}"))
        .await
        .expect("a local socket");

    let cloud_settings = next_frame(&mut cloud).await;
    assert_eq!(cloud_settings["type"], "settings");
    assert_eq!(cloud_settings["plugins"], serde_json::json!([]), "a cloud subscriber is told of no plugins");

    let local_settings = next_frame(&mut local).await;
    assert_eq!(local_settings["type"], "settings");
    assert_eq!(local_settings["plugins"][0]["id"], "tasks", "a local page still sees the list");

    // Both are attached now; what the plugins publish reaches every
    // subscriber through the same broadcast.
    state
        .global
        .publish(serde_json::json!({"type":"plugins","plugins":[{"id":"tasks"}]}).to_string());
    state
        .global
        .publish(serde_json::json!({"type":"plugin-doc","plugin":"tasks","name":"state","revision":1}).to_string());
    state
        .global
        .publish(serde_json::json!({"type":"screens:changed","listRevision":1}).to_string());

    /// The types of the frames up to `marker` — what `socket` heard, in
    /// order, so a test can say what it did not.
    async fn types_until_marker(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        marker: &str,
    ) -> Vec<String> {
        let mut types = Vec::new();
        for _ in 0..16 {
            let frame = next_frame(socket).await;
            let kind = frame["type"].as_str().unwrap_or_default().to_string();
            let done = kind == marker;
            types.push(kind);
            if done {
                break;
            }
        }
        assert_eq!(types.last().map(String::as_str), Some(marker), "no {marker} in {types:?}");
        types
    }

    let cloud_heard = types_until_marker(&mut cloud, "screens:changed").await;
    assert!(
        !cloud_heard.iter().any(|kind| kind == "plugins" || kind == "plugin-doc"),
        "a cloud subscriber heard {cloud_heard:?}"
    );
    let local_heard = types_until_marker(&mut local, "screens:changed").await;
    for kind in ["plugins", "plugin-doc"] {
        assert!(local_heard.iter().any(|heard| heard == kind), "a local page heard {local_heard:?}");
    }
}

/// Review of S1-7, round 3: a feed socket opened by a login is closed
/// (4401) once that login is signed out; one opened by another login stays.
#[tokio::test]
async fn signing_out_closes_the_feed_sockets_of_that_login() {
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    let home = tempfile::tempdir().unwrap();
    let (state, addr) = crate::api::test_support::served(home.path()).await;
    let auth = state.drive.auth.clone();
    auth.users.add("ada", "correct horse battery staple").await.unwrap();
    let open = |digest: String| {
        let auth = auth.clone();
        async move {
            let ticket = auth.tickets.issue("ada", &digest, droppen_auth::tickets::TicketScope::Ws).unwrap().0;
            tokio_tungstenite::connect_async(format!("ws://{addr}/api/events?ticket={ticket}"))
                .await
                .expect("let in")
                .0
        }
    };
    let digest = |token: &str| droppen_auth::sessions::digest_of(token);
    let gone = digest(&auth.sessions.issue("ada", None).await.unwrap().token);
    let kept = digest(&auth.sessions.issue("ada", None).await.unwrap().token);
    let mut signed_out = open(gone.clone()).await;
    let mut stays = open(kept).await;
    assert!(auth.sessions.revoke(&gone).await);
    let closed = tokio::time::timeout(LOGIN_RECHECK + Duration::from_secs(5), async {
        loop {
            match signed_out.next().await {
                Some(Ok(WsMessage::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => continue,
                other => panic!("{other:?}"),
            }
        }
    })
    .await
    .expect("closed in time");
    assert_eq!(closed, Some(4401));
    // The other login's socket is still served.
    stays.send(WsMessage::Ping(Default::default())).await.unwrap();
    let alive = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stays.next().await {
                Some(Ok(WsMessage::Pong(_))) => return true,
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return false,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .expect("an answer");
    assert!(alive);
}

/// A feed socket opened by the device's owner (`remote-hand-server open`)
/// closes (4401) once the device has its first account: from then on the
/// owner's login stands for nobody, whatever its row says.
#[tokio::test]
async fn the_first_account_closes_the_owners_feed_socket() {
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    let home = tempfile::tempdir().unwrap();
    let (state, addr) = crate::api::test_support::served(home.path()).await;
    let auth = state.drive.auth.clone();
    let owner = droppen_auth::sessions::digest_of(&auth.sessions.issue(droppen_auth::LOCAL_OWNER, None).await.unwrap().token);
    let ticket = auth
        .tickets
        .issue(droppen_auth::LOCAL_OWNER, &owner, droppen_auth::tickets::TicketScope::Ws)
        .unwrap()
        .0;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/events?ticket={ticket}"))
        .await
        .expect("the owner is let in while there is no account");
    auth.users.add("ada", "correct horse battery staple").await.unwrap();
    let closed = tokio::time::timeout(LOGIN_RECHECK + Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(WsMessage::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => continue,
                other => panic!("{other:?}"),
            }
        }
    })
    .await
    .expect("closed in time");
    assert_eq!(closed, Some(4401));
}

/// The list socket of `state`'s server at `addr`, opened by its owner and
/// then never read from; once the socket has attached to the list feed.
async fn never_reading_list(
    state: &AppState,
    addr: std::net::SocketAddr,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let ticket = crate::api::test_support::owner_ticket(state).await;
    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/events?ticket={ticket}"))
        .await
        .expect("let in");
    let attached = tokio::time::timeout(Duration::from_secs(30), async {
        while !state.global.is_running() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    attached.await.expect("the socket attaches to the list feed");
    client
}

/// A frame of a quarter of a megabyte, for a client that takes none.
fn wide_frame() -> String {
    format!(r#"{{"type":"screens:list","pad":"{}"}}"#, "x".repeat(256 * 1024))
}

/// A client that stops reading cannot hold the list socket's task for
/// good: a frame it makes no room for gives up at the server's limit, and
/// the task lets the list feed go before its short close.
///
/// Real sockets, and a client that never reads after connecting. Frames
/// are published until the kernel's buffers are full and a send has to
/// wait; the broadcast keeps at most [`CHANNEL_CAPACITY`] of them.
#[tokio::test]
async fn a_list_client_that_never_reads_is_given_up_on() {
    let home = tempfile::tempdir().unwrap();
    let (state, addr) = crate::api::test_support::served_with(home.path(), |state| state.send_limit = Duration::from_secs(1)).await;
    let _client = never_reading_list(&state, addr).await;
    let frame = wide_frame();
    let deadline = Instant::now() + Duration::from_secs(60);
    while state.global.is_running() {
        assert!(
            Instant::now() < deadline,
            "a frame that cannot leave gives up at the limit, and the list feed is let go"
        );
        state.global.publish(frame.clone());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
