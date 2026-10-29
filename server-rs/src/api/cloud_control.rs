//! The server's handle on its hub link ([`droppen_cloud::control::Control`])
//! against this instance's agent files: enrolling, cancelling, adding again,
//! restarting with a renewed key, unenrolling.

use std::time::Duration;

use rh_proto::identity::{EnrollCode, MachineIdentity};

use droppen_agent::enroll::BeginError;
use droppen_agent::link::LinkEnv;
use droppen_agent::state::AgentState;
use droppen_agent::status::Standing;
use droppen_agent::testing::{AgentFilesTesting, RunningTesting, TestSeed};
use droppen_agent::AgentFiles;
use droppen_cloud::control::*;
use droppen_cloud::testing::{ControlTesting, LinkFileTesting};
use droppen_instance::paths::Paths;
#[tokio::test]
async fn a_server_that_does_not_own_its_instance_runs_no_link() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctltest");
    let control = Control::new(paths.clone());
    assert!(matches!(control.available(), Err(ControlError::Unavailable(_))));
    assert!(!control.start_if_enrolled().await);
    assert_eq!(control.status().state, Standing::None);
    let code = EnrollCode {
        account_fingerprint: [1; 16],
        secret: [2; 16],
    };
    assert!(matches!(
        control.enroll(&code, "ws://127.0.0.1:9").await,
        Err(ControlError::Unavailable(_))
    ));
}

#[tokio::test]
async fn status_reads_the_saved_state_and_unenroll_leaves_nothing_under_agent() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctltest");
    let control = Control::new(paths.clone());
    let identity = control.identity().unwrap();
    let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5_000);
    st.save(&AgentFiles::of(&paths)).unwrap();
    AgentFiles::of(&paths).store_machine_key(0, &[7; 32]).unwrap();
    let status = control.status();
    assert_eq!(status.state, Standing::Enrolling);
    assert_eq!(status.machine.as_deref(), Some("m-1"));
    assert!(!status.running);
    st.mark_removed(6_000);
    st.save(&AgentFiles::of(&paths)).unwrap();
    assert_eq!(
        control.status().state,
        Standing::Enrolling,
        "a removal mark the chain does not back is no removal"
    );
    control.unenroll().await.unwrap();
    assert!(!paths.instance_dir().join("agent").exists(), "agent/ is gone");
    assert_eq!(control.status().state, Standing::None);
    assert_ne!(
        control.identity().unwrap().sign_public(),
        identity.sign_public(),
        "a new machine key after unenroll"
    );
}

/// The process keeps one connection per database file: after unenroll
/// deletes `streams.db`, the next enrollment's store would be the old one
/// — its rows (re-sent to the new account) in an unlinked file, every
/// write lost on restart. The cached connection goes with the file.
#[tokio::test]
async fn a_new_enrollment_never_reopens_the_old_streams_store() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlstore");
    let control = Control::new(paths.clone());
    let _ = control.identity().unwrap();
    AgentFiles::of(&paths).set_streams_kv("plan", "old-account-plan");
    control.unenroll().await.unwrap();
    let file = AgentFiles::of(&paths).streams_db();
    assert!(!file.exists(), "agent/ is gone on disk");
    let _ = control.identity().unwrap(); // a new machine key, as a new sign-in makes
    assert_eq!(AgentFiles::of(&paths).streams_kv("plan"), None, "the new enrollment starts from an empty store");
    AgentFiles::of(&paths).set_streams_kv("plan", "new-account-plan");
    assert!(file.exists(), "and it is the file on disk");

    // The same for an enrollment that replaces one that never completed.
    AgentFiles::of(&paths).forget_enrollment_leftovers().unwrap();
    assert_eq!(AgentFiles::of(&paths).streams_kv("plan"), None);
}

/// What a link needs, for a server of these paths that listens nowhere.
fn link_env(paths: &Paths) -> LinkEnv {
    let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).unwrap();
    let auth = droppen_auth::Auth::new(paths, &instance, &droppen_http::config::AuthSettings::default(), false, std::sync::Arc::new(crate::app::NotifyTokens));
    LinkEnv {
        addr: "127.0.0.1:9".parse().unwrap(),
        credential: "test".into(),
        streams: {
            let drive = crate::drive::Drive::new(std::sync::Arc::new(paths.clone()), &instance, auth, crate::app::transcript_sources(), crate::app::register_providers, crate::app::logins());
            std::sync::Arc::new(crate::stream_source::SessionStreams::new(drive.paths.clone(), drive.transcripts.clone(), drive))
        },
        host: std::sync::Arc::new(crate::app::PushLinkHost),
        cancel: tokio_util::sync::CancellationToken::new(),
        aggregator_switch: std::sync::Arc::new(droppen_cloud::LinkFile::of(paths)),
    }
}

/// A port nothing listens on.
fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A deletion of the other devices' copies that a stop interrupted (it
/// stopped right after turning the aggregator off) is finished at the next
/// start, with the aggregator off.
#[tokio::test]
async fn an_unfinished_deletion_of_the_copies_is_finished_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlsweep");
    std::fs::create_dir_all(paths.instance_dir()).unwrap();
    std::fs::write(droppen_cloud::LinkFile::of(&paths).path(), br#"{"aggregatorOff": true}"#).unwrap();
    assert!(!droppen_cloud::LinkFile::of(&paths).wants_aggregator());
    let left = paths.instance_dir().join(".aggregator-deleted-1-2-3");
    std::fs::create_dir_all(&left).unwrap();
    std::fs::write(left.join("copies.db"), b"STARTUP-SWEEP-CANARY").unwrap();
    let mut held = std::fs::File::open(left.join("copies.db")).unwrap();
    let control = Control::new(paths.clone());
    control.attach(link_env(&paths));
    droppen_aggregator::finish_deletions(Duration::from_secs(5)).await;
    assert!(!left.exists(), "swept at start");
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut held, &mut bytes).unwrap();
    assert!(bytes.iter().all(|b| *b == 0), "and zeroed before it went");
}

/// A sign-in that cannot begin (the hub unreachable, or the code
/// refused) leaves the link that was running as it was: a failed sign-in
/// never leaves it stopped.
#[tokio::test]
async fn a_sign_in_that_cannot_begin_leaves_the_link_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlrestore");
    let control = Control::new(paths.clone());
    let _ = control.identity().unwrap();
    let hub = format!("ws://127.0.0.1:{}", closed_port());
    AgentState::enrolling(&hub, "a-1", "m-1", [9; 16], 5_000).save(&AgentFiles::of(&paths)).unwrap();
    control.attach(link_env(&paths));
    assert!(control.start_if_enrolled().await);
    assert!(control.status().running);
    let code = EnrollCode {
        account_fingerprint: [1; 16],
        secret: [2; 16],
    };
    let failed = control.enroll(&code, &hub).await;
    assert!(matches!(failed, Err(ControlError::Begin(BeginError::Unreachable(_)))), "{failed:?}");
    assert!(control.status().running, "the link that ran before runs again");
    assert_eq!(control.status().machine.as_deref(), Some("m-1"), "the same enrollment");
    control.stop().await;
}

/// A member that stopped running AI (`SetHost false`) is its own standing,
/// not an enrollment still waiting for a device: the settings do not say
/// "enrolling", and neither cancel nor the code's expiry deletes its
/// `state.json`.
#[tokio::test]
async fn a_member_that_stopped_running_ai_keeps_its_state() {
    use rh_proto::chain::{genesis_draft, Action, ChainClock, ChainState, DeviceInfo, Signer};
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlhost");
    let control = Control::new(paths.clone());
    let machine = control.identity().unwrap();
    let ak = MachineIdentity::from_seed(&[1; 32]);
    let device = MachineIdentity::from_seed(&[2; 32]);
    let info = DeviceInfo {
        id: "d-1".into(),
        sign_key: device.sign_public(),
        dh_key: device.dh_public(),
        full: true,
    };
    let genesis = genesis_draft("a-1", ak.sign_public(), info, None, 1_000).sign(ak.signing_key());
    let mut chain = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
    let mut entries = vec![genesis];
    let stop = Action::SetHost {
        id: "m-1".into(),
        host: false,
    };
    for action in [Action::AddMachine(machine.info("m-1")), stop] {
        let e = chain.draft(Signer::Device("d-1".into()), action, 2_000).sign(device.signing_key());
        chain.apply(&e, &mut ChainClock::history()).unwrap();
        entries.push(e);
    }
    let mut st = AgentState::enrolling(
        "wss://hub.example",
        "a-1",
        "m-1",
        rh_proto::identity::fingerprint(&ak.sign_public()),
        5_000,
    );
    assert!(st.absorb(&entries, 5_000).error.is_none());
    assert!(st.enrolled() && st.not_hosting());
    st.save(&AgentFiles::of(&paths)).unwrap();
    let state_file = AgentFiles::of(&paths).state();
    assert_eq!(control.status().state, Standing::NotHosting, "not \"enrolling\"");
    assert!(!control.cancel_enrollment().await.unwrap(), "nothing to cancel");
    assert!(state_file.exists());
    control.expire_enrollment_now(0).await;
    assert!(state_file.exists(), "the code's expiry leaves a member alone");
    assert_eq!(control.delete_copies().await.unwrap(), 0, "no copy to delete while it runs no AI");
    assert_eq!(control.status().state, Standing::NotHosting);
}

/// Host identity renewal (§3.5): once a renewed key is in place the link
/// is restarted in this process — through the hook a link calls, on a
/// task of its own — and runs with the new key; the server's token (what
/// its AI sessions and terminals live under) is never touched.
#[tokio::test]
async fn a_restart_runs_the_link_with_the_renewed_key_and_leaves_the_server_alone() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlrestart");
    let control = Control::new(paths.clone());
    let old = control.identity().unwrap();
    let hub = format!("ws://127.0.0.1:{}", closed_port());
    AgentState::enrolling(&hub, "a-1", "m-1", [9; 16], 5_000).save(&AgentFiles::of(&paths)).unwrap();
    let env = link_env(&paths);
    let server = env.cancel.clone();
    let session = server.child_token();
    control.attach(env);
    assert!(control.start_if_enrolled().await);
    let key_in_use = |control: &Control| {
        let running = control.running_link();
        running.as_ref().filter(|r| !r.finished()).map(|r| r.identity().sign_public())
    };
    assert_eq!(key_in_use(&control), Some(old.sign_public()));
    // The renewed seed in place, as a promotion leaves it.
    let files = AgentFiles::of(&paths);
    files
        .stage_seed(TestSeed {
            uuid: droppen_instance::identity::ensure(&paths).unwrap().uuid,
            seed: [9; 32],
            nonce: [1; 16],
            chain_seq: 0,
            chain_head: [0; 32],
            assurance: rh_proto::chain::Assurance::Auto,
            created_ms: 1,
            accepted_ms: Some(2),
        })
        .unwrap();
    files.promote_seed(&[1; 16]).unwrap();
    let new = MachineIdentity::from_seed(&[9; 32]);
    // The promotion noted that the push keys follow; a
    // device subscribed against the old VAPID key.
    let old_vapid = crate::push::store::vapid_public(&paths).await.unwrap();
    let point = {
        use p256::elliptic_curve::sec1::ToEncodedPoint as _;
        let device = p256::SecretKey::random(&mut rand::rngs::OsRng);
        crate::push::ece::b64url(device.public_key().to_encoded_point(false).as_bytes())
    };
    crate::push::store::upsert(&paths, "https://push.example/a".into(), point, crate::push::ece::b64url(&[9; 16]), None)
        .await
        .unwrap();
    control.running_link().as_ref().unwrap().ask_push_renew();
    (control.link_restart_hook())();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while key_in_use(&control) != Some(new.sign_public()) {
        assert!(std::time::Instant::now() < deadline, "the link restarts with the new key");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while AgentState::load(&AgentFiles::of(&paths)).unwrap().unwrap().rotation.push_renew {
        assert!(std::time::Instant::now() < deadline, "the restarted link renews the push keys");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(crate::push::store::subscriptions(&paths).await.is_empty(), "every push subscription forgotten");
    assert_ne!(crate::push::store::vapid_public(&paths).await.unwrap(), old_vapid, "a new VAPID keypair");
    assert!(control.status().running);
    assert!(!server.is_cancelled() && !session.is_cancelled(), "the server and what runs under it go on");
    control.restart().await.unwrap();
    assert_eq!(key_in_use(&control), Some(new.sign_public()));
    control.stop().await;
    assert!(!server.is_cancelled() && !session.is_cancelled());
}

/// "Add this device again" cut short by a crash after
/// the seed was archived (its `state.json` still there) is finished when
/// the control comes up: the instance is not enrolled, with a fresh seed,
/// and the link does not start as the old enrollment.
#[tokio::test]
async fn adding_again_cut_short_by_a_crash_is_finished_when_the_control_comes_up() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlfresh");
    let control = Control::new(paths.clone());
    let old = control.identity().unwrap();
    AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5_000).save(&AgentFiles::of(&paths)).unwrap();
    // What the crash left: the marker, the seed archived, the state kept.
    let agent = AgentFiles::of(&paths).machine_key().parent().unwrap().to_path_buf();
    droppen_base::fsutil::write_private_reporting_path(&agent.join("re-enroll-fresh.json"), b"{}").unwrap();
    std::fs::rename(AgentFiles::of(&paths).machine_key(), AgentFiles::of(&paths).archived_key(5)).unwrap();
    control.attach(link_env(&paths));
    assert!(!AgentFiles::of(&paths).has_state(), "the old enrollment is forgotten");
    assert!(!agent.join("re-enroll-fresh.json").exists());
    let fresh = control.identity().unwrap();
    assert_ne!(fresh.sign_public(), old.sign_public());
    assert!(AgentFiles::of(&paths).seed_archived(&old));
    assert!(!control.start_if_enrolled().await, "nothing to start");
    assert_eq!(control.status().state, Standing::None);
}

#[tokio::test]
async fn cancelling_an_enrollment_keeps_the_machine_key() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctltest");
    let control = Control::new(paths.clone());
    let identity = control.identity().unwrap();
    AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5_000)
        .save(&AgentFiles::of(&paths))
        .unwrap();
    assert!(control.cancel_enrollment().await.unwrap());
    assert_eq!(control.status().state, Standing::None);
    assert_eq!(control.identity().unwrap().sign_public(), identity.sign_public(), "the machine key stays");
}

/// m-1's state with a v3 chain that added it with `current`'s keys, then
/// renewed it to `to`'s (as the agent's rotation tests build one).
fn renewed_to(current: &MachineIdentity, to: &MachineIdentity) -> AgentState {
    use rh_proto::{chain, sig};
    let ak = MachineIdentity::from_seed(&[1; 32]);
    let d1 = MachineIdentity::from_seed(&[2; 32]);
    let genesis = chain::genesis_draft(
        "a-1",
        ak.sign_public(),
        chain::DeviceInfo {
            id: "d-1".into(),
            sign_key: d1.sign_public(),
            dh_key: d1.dh_public(),
            full: true,
        },
        None,
        1_000,
    )
    .sign(ak.signing_key());
    let mut hub = chain::ChainState::verify(std::slice::from_ref(&genesis), None, &mut chain::ChainClock::history()).unwrap();
    let mut entries = vec![genesis];
    let mut push = |hub: &mut chain::ChainState, action: chain::Action| {
        let e = hub.draft(chain::Signer::Device("d-1".into()), action, 2_000 + hub.seq()).sign(d1.signing_key());
        hub.apply(&e, &mut chain::ChainClock::history()).unwrap();
        entries.push(e);
    };
    let epoch = |e: u32| {
        chain::Action::KeyEpochs(chain::KeyCommits {
            user: None,
            machines: vec![("m-1".into(), chain::KeyCommit { epoch: e, hash: [e as u8; 32] })],
        })
    };
    push(
        &mut hub,
        chain::Action::AddDevice3(chain::DeviceV3 {
            id: "m-1".into(),
            sign_key: current.sign_public(),
            dh_key: current.dh_public(),
            flags: chain::Flags { host: true, admin: false },
        }),
    );
    push(&mut hub, epoch(0));
    let mut r = chain::HostRotation {
        host: "m-1".into(),
        sign_key: to.sign_public(),
        dh_key: to.dh_public(),
        staged_seq: hub.seq(),
        staged_head: hub.head(),
        nonce: [1; 16],
        assurance: chain::Assurance::Auto,
        old_sig: [0; 64],
        new_sig: [0; 64],
    };
    let body = r.body("a-1", &current.sign_public(), &current.dh_public());
    r.old_sig = sig::sign(current.signing_key(), sig::HOST_ROTATE, &body);
    r.new_sig = sig::sign(to.signing_key(), sig::HOST_ROTATE, &body);
    push(&mut hub, chain::Action::RotateHostKey(r));
    push(&mut hub, epoch(1));
    let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", rh_proto::identity::fingerprint(&ak.sign_public()), 5);
    assert!(st.absorb(&entries, droppen_base::time::now_ms_u64()).error.is_none());
    st
}

/// The local settings' sign-in renews the identity only when the seed in use
/// is retired: while the chain's renewal of this machine is to a seed it has
/// staged — its own renewal, waiting or set aside as expired — it is left
/// alone; with no staging of it at all (the keys were renewed elsewhere) the
/// seed is archived, the enrollment forgotten and a fresh identity made. The agent's
/// `rotation::tests::an_honest_hosts_own_renewal_is_never_wiped_for_a_fresh_identity`
/// checks the predicate this acts on (`AgentFiles::seed_in_use_retired`).
#[tokio::test]
async fn the_sign_in_renews_only_a_retired_seed() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlretired");
    let control = Control::new(paths.clone());
    let current = control.identity().unwrap();
    let files = AgentFiles::of(&paths);
    let to = MachineIdentity::from_seed(&[8; 32]);
    files
        .stage_seed(TestSeed {
            uuid: droppen_instance::identity::ensure(&paths).unwrap().uuid,
            seed: [8; 32],
            nonce: [3; 16],
            chain_seq: 2,
            chain_head: [4; 32],
            assurance: rh_proto::chain::Assurance::Auto,
            created_ms: 5,
            accepted_ms: Some(5),
        })
        .unwrap();
    renewed_to(&current, &to).save(&files).unwrap();
    let untouched = |why: &str| {
        assert!(!files.seed_retired(&current), "{why}: its own renewal");
        assert!(!files.seed_archived(&current), "{why}: nothing archived");
        assert!(files.has_state(), "{why}: the enrollment stays");
        assert_eq!(control.identity().unwrap().sign_public(), current.sign_public(), "{why}: the seed in use stays");
        assert_eq!(files.kept_staged_seeds(), 1, "{why}: the staged seed stays");
    };
    assert!(!control.renew_retired_seed().await.unwrap(), "its own renewal: left alone");
    untouched("staged");
    // Set aside as expired: still its own.
    files.expire_staged_seed(&[3; 16]).unwrap();
    assert!(!control.renew_retired_seed().await.unwrap(), "set aside as expired: left alone");
    untouched("set aside");
    // No staging of it at all: replaced elsewhere — only then a fresh one.
    files.discard_staged_seeds().unwrap();
    assert!(files.seed_retired(&current));
    assert!(control.renew_retired_seed().await.unwrap(), "renewed elsewhere: a fresh identity");
    assert!(files.seed_archived(&current));
    assert!(!files.has_state(), "the enrollment is forgotten");
    assert_ne!(control.identity().unwrap().sign_public(), current.sign_public());
}

/// Disconnecting keeps a dispute statement the hub does not have yet, and
/// nothing else under `agent/`. The agent's
/// `rotation_tests::an_undelivered_dispute_statement_survives_disconnecting`
/// checks that the statement is still delivered afterwards.
#[tokio::test]
async fn unenrolling_keeps_an_undelivered_dispute_statement() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "ctldispute");
    let control = Control::new(paths.clone());
    let _ = control.identity().unwrap();
    let files = AgentFiles::of(&paths);
    AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5_000).save(&files).unwrap();
    let pending = droppen_agent::dispute::Statement {
        version: 1,
        hub: "wss://hub.example".into(),
        account: "a-1".into(),
        host: "m-1".into(),
        seq: 4,
        entry_hash: "aa".repeat(32),
        new_sign: "bb".repeat(32),
        sig: "cc".repeat(64),
        created_ms: 5_000,
        acked_ms: None,
        stopped: None,
    };
    let statement = pending.path(&files);
    std::fs::write(&statement, serde_json::to_vec(&pending).unwrap()).unwrap();
    control.unenroll().await.unwrap();
    assert!(statement.exists(), "the statement survives");
    let left: Vec<std::ffi::OsString> = std::fs::read_dir(statement.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, vec![statement.file_name().unwrap().to_os_string()], "everything else in agent/ is gone");
    assert!(!files.has_state());
}
