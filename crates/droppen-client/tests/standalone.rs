//! A standalone device end to end, in this process: its channel to a host
//! through a real relay (`droppen-relay-core`), requests over `rpc` pipes
//! with their answers streamed, relay tickets renewed before they run out,
//! connections made again after the relay closed them, the chain kept and
//! pinned across restarts, and each failure sorted as the a2a proxy needs it.
//! The hub and the host's agent are the test ones of `support`.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use droppen_client::device::StandaloneDevice;
use droppen_client::failure::{Failure, Refuser};
use droppen_client::now_ms;
use droppen_client::rpc::{refusal_of, RpcRequest};
use droppen_client::verified::{VerifiedChain, CHAIN_FILE};
use droppen_proto::chain::{Accept, Action, ChainError};
use droppen_proto::ticket::TicketRole;
use serde_json::{json, Value};
use support::*;

const SECS: Duration = Duration::from_secs(10);

async fn first_piece(body: &mut droppen_client::rpc::RpcBody) -> Vec<u8> {
    tokio::time::timeout(SECS, body.chunk())
        .await
        .expect("a piece in time")
        .expect("a piece")
        .expect("not a failure")
}

/// The failure a running body ends with.
async fn body_failure(body: &mut droppen_client::rpc::RpcBody) -> Failure {
    let deadline = tokio::time::Instant::now() + SECS;
    loop {
        let piece = tokio::time::timeout_at(deadline, body.chunk()).await.expect("the body ends in time");
        match piece {
            Some(Ok(_)) => continue,
            Some(Err(failure)) => return failure,
            None => panic!("the body ended cleanly"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_goes_through_a_relay_and_its_answer_comes_back() {
    let stand = Stand::new(echo()).await;
    let response = stand
        .device
        .rpc(
            HOST,
            RpcRequest {
                method: "POST".into(),
                path: "/a2a/agents/main/jsonrpc?x=1".into(),
                headers: vec![("content-type".into(), "application/json".into()), ("a2a-version".into(), "1.0".into())],
                body: br#"{"jsonrpc":"2.0","id":1,"method":"GetTask"}"#.to_vec(),
            },
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-type"), Some("application/json"));
    let body: Value = serde_json::from_slice(&response.body.read_to_end(1 << 20).await.unwrap()).unwrap();
    assert_eq!(body["method"], "POST");
    assert_eq!(body["path"], "/a2a/agents/main/jsonrpc?x=1");
    assert_eq!(body["body"], r#"{"jsonrpc":"2.0","id":1,"method":"GetTask"}"#);
    assert_eq!(body["headers"], json!([["content-type", "application/json"], ["a2a-version", "1.0"]]));
    // The next request goes on the same channel; a large body goes up whole.
    let big = vec![b'y'; 3 * 1024 * 1024 + 17];
    let response = stand
        .device
        .rpc(
            HOST,
            RpcRequest {
                method: "PUT".into(),
                path: "/a2a/big".into(),
                headers: Vec::new(),
                body: big.clone(),
            },
        )
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&response.body.read_to_end(8 << 20).await.unwrap()).unwrap();
    assert_eq!(body["body"].as_str().unwrap().len(), big.len());
    assert_eq!(stand.agent.outcomes(), ["open"], "one channel for both");
    assert_eq!(stand.hub.tickets(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_answer_reaches_the_reader_as_it_is_sent() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let sent = Arc::new(AtomicUsize::new(0));
    let serve = {
        let (gate, sent) = (gate.clone(), sent.clone());
        handler(move |_request: Request, responder: Responder| {
            let (gate, sent) = (gate.clone(), sent.clone());
            async move {
                responder.head(200, &[("content-type", "text/event-stream")]).await;
                responder.chunk(b"data: first\n\n").await;
                sent.fetch_add(1, Ordering::SeqCst);
                // Nothing more until the reader saw the first piece.
                gate.notified().await;
                for _ in 0..128 {
                    responder.chunk(&[b'x'; 64 * 1024]).await;
                    sent.fetch_add(1, Ordering::SeqCst);
                }
                responder.close(0);
            }
        })
    };
    let stand = Stand::new(serve).await;
    let mut response = stand.device.rpc(HOST, Stand::get("/a2a/agents/main/rest/message:stream")).await.unwrap();
    assert_eq!((response.status, response.header("content-type")), (200, Some("text/event-stream")));
    assert_eq!(first_piece(&mut response.body).await, b"data: first\n\n");
    assert_eq!(
        sent.load(Ordering::SeqCst),
        1,
        "the head and the first piece came before the rest was sent"
    );
    gate.notify_one();
    // A reader that stops reading holds the sender back: only what the
    // windows take is sent meanwhile, not the 8 MiB.
    assert_eq!(first_piece(&mut response.body).await.len(), 64 * 1024);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let while_paused = sent.load(Ordering::SeqCst);
    assert!(while_paused < 40, "{while_paused} pieces sent to a reader that does not read");
    let mut total = 64 * 1024;
    while let Some(piece) = tokio::time::timeout(SECS, response.body.chunk()).await.unwrap() {
        total += piece.unwrap().len();
    }
    assert_eq!(total, 128 * 64 * 1024);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_stream_runs_on_and_a_reader_that_walks_away_ends_it_at_the_host() {
    let stand = Stand::new(events(Duration::from_millis(20))).await;
    let mut response = stand
        .device
        .rpc(HOST, Stand::get("/a2a/agents/main/rest/tasks/t1:subscribe"))
        .await
        .unwrap();
    let mut seen = Vec::new();
    while seen.len() < 20 {
        seen.push(String::from_utf8(first_piece(&mut response.body).await).unwrap());
    }
    assert_eq!(seen[0], "data: 0\n\n");
    assert_eq!(seen[19], "data: 19\n\n", "in order, none lost");
    drop(response);
    // The channel is still good for the next request.
    let echo_answer = stand.device.rpc(HOST, Stand::get("/a2a/ping")).await.unwrap();
    assert_eq!(echo_answer.status, 200);
    assert_eq!(stand.agent.outcomes(), ["open"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_ticket_is_renewed_before_it_runs_out() {
    // Two-second tickets, and a relay that closes a connection the moment
    // its ticket ran out.
    let mut config = StandConfig::new(echo());
    config.timing = timing(0);
    config.ticket_ttl_ms = 2_000;
    let stand = Stand::with(config).await;
    assert_eq!(stand.device.rpc(HOST, Stand::get("/a2a/one")).await.unwrap().status, 200);
    let channel = stand.device.channel(HOST).await.unwrap();
    let conn = stand.device.relays().connected().await.pop().unwrap();
    tokio::time::sleep(Duration::from_millis(5_500)).await;
    assert!(!conn.is_closed(), "the relay kept the connection: {:?}", conn.link().close_frame().get());
    // The renewal is applied a moment after the ticket's deadline, and the
    // sample can land between the two: wait it out rather than assert at once.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !(conn.renewals() >= 2 && conn.expires_ms() > now_ms()) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "renewed {} times, expires {} against {}",
            conn.renewals(),
            conn.expires_ms(),
            now_ms()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(stand.hub.tickets() >= 3);
    let again = stand.device.channel(HOST).await.unwrap();
    assert!(Arc::ptr_eq(&channel, &again), "the same channel lives on");
    assert_eq!(stand.device.rpc(HOST, Stand::get("/a2a/two")).await.unwrap().status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ticket_not_renewed_has_the_relay_close_the_connection_and_the_next_request_signs_in_again() {
    let mut config = StandConfig::new(events(Duration::from_millis(50)));
    config.timing = timing(0);
    config.ticket_ttl_ms = 1_500;
    config.options.relays.renew = false;
    let stand = Stand::with(config).await;
    let mut response = stand
        .device
        .rpc(HOST, Stand::get("/a2a/agents/main/rest/tasks/t1:subscribe"))
        .await
        .unwrap();
    let conn = stand.device.relays().connected().await.pop().unwrap();
    let failure = body_failure(&mut response.body).await;
    assert!(matches!(failure, Failure::Offline(_)), "{failure:?}");
    assert_eq!(conn.link().close_frame().get().map(|(code, _)| code), Some(4419));
    // A new connection, a fresh ticket, a new channel.
    let tickets = stand.hub.tickets();
    stand.agent.set_handler(echo());
    assert_eq!(stand.device.rpc(HOST, Stand::get("/a2a/again")).await.unwrap().status, 200);
    assert!(stand.hub.tickets() > tickets);
    assert_eq!(stand.agent.outcomes(), ["open", "open"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_connects_again_after_the_relay_closed_its_connection() {
    let mut stand = Stand::new(events(Duration::from_millis(50))).await;
    // The hub revokes the device's tickets on the relay (4403): what was
    // running fails as a relay's refusal, for the caller's chain to judge.
    let mut response = stand.device.rpc(HOST, Stand::get("/a2a/stream")).await.unwrap();
    first_piece(&mut response.body).await;
    stand.relay.server.revoke_role(ACCOUNT, Some(PROXY), Some(TicketRole::Device), now_ms());
    let failure = body_failure(&mut response.body).await;
    assert!(
        matches!(
            failure,
            Failure::Refused {
                by: Refuser::Relay,
                code: 4403,
                ..
            }
        ),
        "{failure:?}"
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
    stand.agent.set_handler(echo());
    assert_eq!(stand.device.rpc(HOST, Stand::get("/a2a/after-revoke")).await.unwrap().status, 200);
    // The relay restarts: `goaway`, then 1012 for everyone, the host's agent
    // too. Once the host is back, the next request finds it.
    stand.agent.set_handler(events(Duration::from_millis(50)));
    let mut response = stand.device.rpc(HOST, Stand::get("/a2a/stream")).await.unwrap();
    first_piece(&mut response.body).await;
    stand.relay.restart().await;
    let failure = body_failure(&mut response.body).await;
    assert!(matches!(failure, Failure::Offline(_)), "{failure:?}");
    stand.agent.set_handler(echo());
    stand.agent.attach(&stand.relay).await;
    assert_eq!(stand.device.rpc(HOST, Stand::get("/a2a/after-restart")).await.unwrap().status, 200);
    assert_eq!(stand.agent.outcomes(), ["open", "open", "open"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_chain_is_kept_pinned_and_verified_across_restarts() {
    let account = Account::new();
    let dir = scratch("chain");
    let mut view = VerifiedChain::pinned(&dir, ACCOUNT, account.fingerprint);
    assert!(view.view().is_none());
    assert_eq!(view.advance(&[]).unwrap().applied, 0);
    assert!(VerifiedChain::load(&dir).unwrap().is_none(), "nothing kept before a chain");
    let entries = account.entries();
    let advanced = view.advance(&entries).unwrap();
    assert_eq!((advanced.applied, advanced.error), (entries.len(), None));
    assert_eq!(view.root().genesis_hash, Some(entries[0].hash()), "the genesis is pinned too");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join(CHAIN_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let back = VerifiedChain::load(&dir).unwrap().unwrap();
    assert_eq!(back.view().unwrap().head(), view.view().unwrap().head());
    assert_eq!(back.root(), view.root());

    // A chain under another account key is refused by the fingerprint pin,
    // and nothing is kept.
    let other_dir = scratch("chain-other");
    let mut other = VerifiedChain::pinned(&other_dir, ACCOUNT, [0xaa; 16]);
    let refused = other.advance(&entries).unwrap();
    assert_eq!((refused.applied, refused.error), (0, Some(ChainError::PinMismatch)));
    assert!(VerifiedChain::load(&other_dir).unwrap().is_none());

    // A fork of what the view has is refused, and the view stays.
    let mut fork = account.at(3);
    fork.append(vec![Action::SetAccept {
        host: HOST.into(),
        from: Accept::All,
    }]);
    let mut back = back;
    let forked = back.advance(&fork.entries()).unwrap();
    assert_eq!(forked.error, Some(ChainError::Fork { seq: 4 }));
    assert_eq!(back.view().unwrap().head(), view.view().unwrap().head());

    // A file someone changed does not verify: refused, not trusted.
    let text = std::fs::read_to_string(dir.join(CHAIN_FILE)).unwrap();
    let mut file: Value = serde_json::from_str(&text).unwrap();
    let last = file["entries"].as_array().unwrap().len() - 1;
    let entry = file["entries"][last].as_str().unwrap().to_string();
    let flipped = format!("{}{}", &entry[..entry.len() - 2], if entry.ends_with("00") { "01" } else { "00" });
    file["entries"][last] = json!(flipped);
    droppen_client::files::write_private(&dir.join(CHAIN_FILE), file.to_string().as_bytes()).unwrap();
    let error = VerifiedChain::load(&dir).err().expect("a changed chain is refused");
    assert!(error.contains("does not verify"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_comes_back_from_its_data_directory_and_refuses_a_hub_that_serves_a_fork() {
    let stand = Stand::new(echo()).await;
    let seq = stand.device.chain_seq().unwrap();
    let again = StandaloneDevice::open_dir(&stand.dir, device_options()).unwrap().unwrap();
    assert_eq!(again.chain_seq(), Some(seq));
    assert_eq!(again.identity().device, PROXY);
    assert_eq!(again.rpc(HOST, Stand::get("/a2a/back")).await.unwrap().status, 200);
    // The hub now serves another branch past the device's view.
    let mut fork = stand.account.at(seq - 1);
    fork.append(vec![Action::SetAccept {
        host: HOST.into(),
        from: Accept::All,
    }]);
    fork.append(vec![Action::SetAccept {
        host: HOST.into(),
        from: Accept::Admins,
    }]);
    stand.hub.set_entries(fork.entries());
    let failure = again.sync_chain().await.unwrap_err();
    assert!(matches!(failure, Failure::Chain(_)), "{failure:?}");
    assert_eq!(again.chain_seq(), Some(seq), "the view stays where it was");
    // Not joined yet: no device.
    assert!(StandaloneDevice::open_dir(&scratch("empty"), device_options()).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_an_agents_reply_teaches_the_device_is_kept() {
    let mut stand = Stand::new(echo()).await;
    let before = stand.device.chain_seq().unwrap();
    // An entry the agent has and the hub has not served the device yet.
    let entries = stand.account.append(vec![Action::SetAccept {
        host: HOST.into(),
        from: Accept::All,
    }]);
    {
        let mut st = stand.agent.state.lock().unwrap();
        let st = &mut *st;
        st.chain.merge(&entries, &mut st.clock).unwrap();
        st.attestations = vec![stand.account.attestation()];
    }
    assert_eq!(stand.device.rpc(HOST, Stand::get("/a2a/x")).await.unwrap().status, 200);
    assert_eq!(stand.device.chain_seq(), Some(before + 1));
    let kept = VerifiedChain::load(&stand.dir).unwrap().unwrap();
    assert_eq!(kept.seq(), Some(before + 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_behind_the_device_asks_for_entries_and_lets_it_in_once_it_caught_up() {
    let mut stand = Stand::new(echo()).await;
    // Eighteen entries the agent lacks: more than a hello carries. (A host
    // that stops taking a proxy's calls renews its machine key next.)
    let mut actions = Vec::new();
    for n in 0..6 {
        actions.push(Action::SetA2aAccept {
            host: HOST.into(),
            devices: Vec::new(),
        });
        actions.push(Account::host_key(n + 1));
        actions.push(Action::SetA2aAccept {
            host: HOST.into(),
            devices: vec![PROXY.into()],
        });
    }
    let entries = stand.account.append(actions);
    stand.hub.set_entries(stand.account.entries());
    {
        let mut st = stand.agent.state.lock().unwrap();
        st.catch_up_after_need = Some((1, entries));
        st.attestations = vec![stand.account.attestation()];
    }
    stand.device.sync_chain().await.unwrap();
    assert_eq!(stand.device.rpc(HOST, Stand::get("/a2a/x")).await.unwrap().status, 200);
    assert_eq!(stand.agent.outcomes(), ["need", "open"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_failure_is_sorted_as_the_proxy_needs_it() {
    // Offline: the host is on no relay.
    let mut config = StandConfig::new(echo());
    config.attach = false;
    let stand = Stand::with(config).await;
    let failure = stand.device.rpc(HOST, Stand::get("/a2a/x")).await.unwrap_err();
    assert!(matches!(failure, Failure::Offline(_)), "{failure:?}");
    assert!(stand.agent.outcomes().is_empty());

    // Stale: nobody vouched for the agent's view; tried twice.
    let mut config = StandConfig::new(echo());
    config.vouched = false;
    let stand = Stand::with(config).await;
    let failure = stand.device.rpc(HOST, Stand::get("/a2a/x")).await.unwrap_err();
    assert!(matches!(failure, Failure::Stale(_)), "{failure:?}");
    assert_eq!(stand.agent.outcomes(), ["stale", "stale"]);

    // Removed, by the hub's word: its session no longer taken, and a new
    // one refused as `device_removed`.
    let stand = Stand::new(echo()).await;
    stand.hub.state.lock().unwrap().refuse_sessions = Some((403, "device_removed"));
    assert!(matches!(stand.device.sync_chain().await.unwrap_err(), Failure::Removed(_)));
    assert!(matches!(
        stand.device.rpc(HOST, Stand::get("/a2a/x")).await.unwrap_err(),
        Failure::Removed(_)
    ));

    // On a channel: the bridge refusing a path (4403 before a head), its
    // 412 `lapsed`, and channels ended mid-answer by the host with 4410 (the
    // other end gone) and 4412 (its control lapsed).
    let serve = handler(|request: Request, responder: Responder| async move {
        match request.head.path.as_str() {
            "/a2a/refused" => responder.close(4403),
            "/a2a/lapsed" => {
                responder
                    .json(
                        412,
                        &json!({"error": "stale", "reason": "lapsed", "message": "the device you reach takes no control from this device now"}),
                    )
                    .await
            }
            _ => {
                responder.head(200, &[("content-type", "text/event-stream")]).await;
                for n in 0u64.. {
                    if !responder.chunk(format!("data: {n}\n\n").as_bytes()).await {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    });
    let stand = Stand::new(serve).await;
    let failure = stand.device.rpc(HOST, Stand::get("/a2a/refused")).await.unwrap_err();
    assert!(
        matches!(
            failure,
            Failure::Refused {
                by: Refuser::Bridge,
                code: 4403,
                ..
            }
        ),
        "{failure:?}"
    );
    let response = stand.device.rpc(HOST, Stand::get("/a2a/lapsed")).await.unwrap();
    assert_eq!(response.status, 412);
    let body = response.body.read_to_end(64 * 1024).await.unwrap();
    assert!(matches!(refusal_of(412, &body), Some(Failure::Lapsed(_))));
    for (code, stale) in [(4410u16, false), (4412, true)] {
        let mut response = stand.device.rpc(HOST, Stand::get("/a2a/stream")).await.unwrap();
        first_piece(&mut response.body).await;
        stand.agent.close_channels(code, "closed by the host");
        let failure = body_failure(&mut response.body).await;
        if stale {
            assert!(matches!(failure, Failure::Stale(_)), "{code}: {failure:?}");
        } else {
            assert!(matches!(failure, Failure::Offline(_)), "{code}: {failure:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_call_is_a_refusal_for_its_chain_to_judge() {
    // A pause holds the call: the hub trims the ticket's hosts to whom the
    // chain lets the device reach, so no ticket names the host, and the relay
    // refuses the `OPEN`.
    let mut stand = Stand::new(echo()).await;
    stand.append(
        vec![Action::RenewalPause {
            op: [0x88; 16],
            phase: droppen_proto::chain::PausePhase::Start,
            removals: vec![(HOST.into(), vec![PROXY.into()])],
        }],
        true,
    );
    stand.device.sync_chain().await.unwrap();
    let failure = stand.device.rpc(HOST, Stand::get("/a2a/x")).await.unwrap_err();
    assert!(
        matches!(
            failure,
            Failure::Refused {
                by: Refuser::Relay,
                code: 4403,
                ..
            }
        ),
        "{failure:?}"
    );
    assert!(stand.agent.outcomes().is_empty(), "no hello reached the host");
    assert!(stand.device.with_chain(|c| c.unwrap().may_call_a2a(PROXY, HOST).is_err()));

    // A hub that names it anyway: the agent refuses the hello (4401), once
    // more after a sync.
    stand.hub.state.lock().unwrap().peers = Some(vec![HOST.into()]);
    let fresh = device_in(&scratch("not-accepted"), &stand.hub, &stand.account, device_options());
    fresh.sync_chain().await.unwrap();
    let failure = fresh.rpc(HOST, Stand::get("/a2a/x")).await.unwrap_err();
    assert!(
        matches!(
            failure,
            Failure::Refused {
                by: Refuser::Agent,
                code: 4401,
                ..
            }
        ),
        "{failure:?}"
    );
    let refusals = stand.agent.outcomes().iter().filter(|o| o.starts_with("refused")).count();
    assert_eq!(refusals, 2, "{:?}", stand.agent.outcomes());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wss_relay_is_dialled_at_its_checked_address_and_its_certificate_verified() {
    let mut config = StandConfig::new(echo());
    config.tls = true;
    let stand = Stand::with(config).await;
    assert!(stand.relay.origin.starts_with("wss://127.0.0.1:"), "{}", stand.relay.origin);
    let response = stand.device.rpc(HOST, Stand::get("/a2a/over-tls")).await.unwrap();
    assert_eq!(response.status, 200);
    let body: Value = serde_json::from_slice(&response.body.read_to_end(1 << 20).await.unwrap()).unwrap();
    assert_eq!(body["path"], "/a2a/over-tls");
    // A device that does not trust the test CA (the web PKI roots only)
    // does not take the relay's certificate: no connection, nothing sent.
    let mut options = device_options();
    options.relays.tls = None;
    let stranger = device_in(&scratch("tls-stranger"), &stand.hub, &stand.account, options);
    stranger.sync_chain().await.unwrap();
    let failure = stranger.rpc(HOST, Stand::get("/a2a/x")).await.unwrap_err();
    assert!(
        matches!(&failure, Failure::Offline(why) if why.contains("certificate") || why.contains("UnknownIssuer")),
        "{failure:?}"
    );
    assert_eq!(stand.agent.outcomes(), ["open"]);
}
