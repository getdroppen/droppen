//! `remote-hand-server agent enroll <code> --hub <url> | run` — the cloud agent.
//!
//! In the cloud design (`docs/cross-machine-architecture.md`) the agent *is*
//! this server plus one outbound connection to a hub: it keeps the
//! transcripts, credentials, terminals and files, and the hub only ever
//! forwards ciphertext ([`rh_proto`]).
//!
//! - [`enroll`] — `agent enroll`: join an account with a code a device issued.
//! - [`link`] — the connection an enrolled server keeps to its hub, answering
//!   devices' end-to-end handshakes; started by `serve` (and `agent run`) when,
//!   and only when, this instance is enrolled.
//! - [`direct`] — direct paths: a WebRTC DataChannel next to the hub that a
//!   channel moves onto when the device can reach this machine, and back off
//!   when it fails (`RH_AGENT_DIRECT=0` turns them off).
//! - [`relays`] — the account's relays: tickets from the hub, a connection
//!   to each relay of the account's set, round trips measured and reported
//!   (`RH_AGENT_RELAYS=0` turns them off).
//! - [`bridge`] — a channel's sub-streams bridged to this server's own
//!   loopback API, with a per-boot credential `auth::guard` accepts only from
//!   loopback.
//! - [`state`] — what an enrolled agent remembers (pinned root, verified
//!   chain, first sightings, replay guard, attestations, machine keys).
//! - [`streams`] — the encrypted copy of this machine's session list, status
//!   and transcripts it keeps on the hub for offline reading (M2).
//!
//! A server that has never been enrolled is exactly today's single-machine
//! server: without `<instance>/agent/state.json` nothing here runs.
//!
//! # The identity seed
//!
//! One random 32-byte seed; the Ed25519 and X25519 keys are expanded from it
//! ([`rh_proto::identity::MachineIdentity`]). It is kept by [`FileSeedStore`],
//! `<instance dir>/agent/machine-key.json`, mode 0600
//! in a 0700 directory, next to the instance identity uuid ([`crate::identity`])
//! it belongs to. A key file that names another instance, or that anyone but
//! the owner can read, is refused rather than used.
//!
//! **What 0600 does not do.** Every process running as this user can read the
//! file — including the AI CLIs this server runs. Whoever reads it can sign in
//! to the hub as this machine and accept handshakes as it from elsewhere, for
//! as long as the machine stays enrolled (removing the machine from the chain,
//! or re-enrolling, ends that). It cannot impersonate a *device* to this agent
//! in any way that changes state: a first message carries no request, and a
//! transport message needs the device's own key. The macOS Keychain would
//! narrow who can read the seed to this binary's code identity — at the cost
//! of a prompt after every self-upgrade unless releases are signed with a
//! stable identity — which is why the store is a seam and not done yet.

/// What this agent declares (`agent_info.features`, and its handshake
/// reply's `features`) once it reads chain v3 (local-aggregator §3.5): the
/// hub sends it v3 entries, and lets the account take them once every host
/// declares this and [`POLICY_FEATURE`].
pub const CHAIN_V3_FEATURE: &str = "chain/3";
/// What this agent declares once it refuses the admin-only routes to devices
/// that are not admins (local-aggregator §5.4, [`bridge::refusal`]) and
/// accepts control only as its chain's accept policy says.
pub const POLICY_FEATURE: &str = "policy/1";
/// What this agent declares once it renews its identity key on request
/// (`docs/host-identity-rotation.md` §3.2, §5.6): `POST /rh/rotate`, the
/// staging on its hub connection, the sign-in confirmation and the
/// promotion ([`rotation`]). The hub takes a renewal entry only once every
/// host declares it; the hub's own `welcome` offering it is what lets this
/// agent send the rotation messages at all.
pub const ROTATE_FEATURE: &str = "rotate/1";

pub mod bridge;
pub mod control;
pub mod direct;
pub mod dispute;
pub mod enroll;
pub mod hub;
pub mod link;
pub mod relays;
pub mod rotation;
#[cfg(test)]
mod rotation_tests;
pub mod state;
pub mod streams;
pub mod system;
pub mod traffic;

use std::path::Path;

use crate::sessions::Paths;

mod cli;
mod seed;

pub use cli::{run, ENROLL_PENDING_EXIT, USAGE};
pub use seed::{key_path, load_or_create, load_or_create_in, FileSeedStore, StagedSeed, StoredSeed};
pub(in crate::agent) use seed::sync_dir;
#[cfg(test)]
pub(crate) use seed::dir_syncs;

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The hub origin this deployment used before the `api.` split, and the one
/// it uses now.
const LEGACY_HUB: &str = "wss://droppen.app";
const CANONICAL_HUB: &str = "wss://api.droppen.app";
/// How long the canonical hub has to answer the pre-flight sign-in.
const MIGRATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Move an install enrolled before the split from the apex to the canonical
/// `api.` origin — once, and only if the canonical hub takes its sign-in.
/// The machine keeps its identity, account and chain; only the origin its
/// link dials changes. A hub that cannot be reached, or refuses the machine,
/// leaves the enrolled origin alone (the apex still serves it).
pub async fn migrate_hub(paths: &Paths) -> Result<(), String> {
    let Some(mut state) = state::AgentState::load(paths)? else {
        return Ok(());
    };
    if state.hub != LEGACY_HUB {
        return Ok(());
    }
    let (identity, created) = load_or_create(paths)?;
    if created {
        return Ok(());
    }
    let origin = hub::origin(CANONICAL_HUB)?;
    let attempt = async {
        let (mut sink, mut source, nonce) = hub::dial(&origin).await?;
        hub::login(&mut sink, &mut source, &identity, &state.account, &state.machine, &origin, nonce)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    match tokio::time::timeout(MIGRATE_TIMEOUT, attempt).await {
        Ok(Ok(())) => {
            let from = std::mem::replace(&mut state.hub, origin);
            state.save(paths)?;
            tracing::info!(from = %from, to = %state.hub, machine = %state.machine, "moved this machine to the canonical hub");
        }
        Ok(Err(error)) => tracing::warn!(%error, "the canonical hub did not take this machine; keeping the enrolled hub"),
        Err(_) => tracing::warn!("the canonical hub did not answer in time; keeping the enrolled hub"),
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn check_private(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "{} is readable by others (mode {:o}); chmod 600 it, or delete it to make a new identity",
            path.display(),
            mode & 0o777
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn check_private(_: &Path) -> Result<(), String> {
    Ok(())
}

/// Write `data` to `path` as a new 0600 file in a 0700 directory, atomically:
/// a half-written key file must never be what the next run finds.
///
/// The rule itself lives in [`crate::fsutil::write_private_creating_parent`];
/// this adds the path to the error, which is the shape every caller here
/// reports.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    crate::fsutil::write_private_creating_parent(path, data).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::cli::*;
    use super::seed::*;
    use super::{hub, rotation, state};
    use crate::sessions::Paths;
    use rh_proto::identity::{EnrollCode, MachineIdentity};
    use zeroize::Zeroizing;
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn code() -> String {
        EnrollCode {
            account_fingerprint: [9; 16],
            secret: [7; 16],
        }
        .to_string()
    }

    #[test]
    fn the_subcommands_parse_and_junk_is_the_usage() {
        assert_eq!(
            parse(&args(&["run"])).unwrap(),
            Invocation {
                action: Action::Run,
                hub: None
            }
        );
        let enroll = parse(&args(&["enroll", &code(), "--hub", "wss://hub.example/v1/agent"])).unwrap();
        assert_eq!(enroll.hub.as_deref(), Some("wss://hub.example"));
        assert!(matches!(enroll.action, Action::Enroll(ref code) if code.secret == [7; 16]));
        assert_eq!(
            parse(&args(&["enroll", "-", "--hub", "wss://hub.example"])).unwrap().action,
            Action::EnrollFromStdin,
            "`-`: the code comes on stdin, out of the process list"
        );
        assert!(parse(&args(&["enroll", "-"])).is_err(), "still needs the hub");
        // Piped in: one line, and never more than the cap however much follows.
        assert_eq!(read_capped_line(format!("{}\nmore", code()).as_bytes()).unwrap().trim(), code());
        // The same code with the newer `DR1-` prefix, in any case, as an argument or piped in.
        let dr = code().replacen("RH1-", "DR1-", 1);
        let want = EnrollCode {
            account_fingerprint: [9; 16],
            secret: [7; 16],
        };
        for text in [dr.clone(), dr.to_lowercase()] {
            let parsed = parse(&args(&["enroll", &text, "--hub", "wss://hub.example"])).unwrap();
            assert!(matches!(parsed.action, Action::Enroll(ref c) if *c == want), "{text}");
            let line = read_capped_line(format!("{text}\r\nmore").as_bytes()).unwrap();
            assert_eq!(code_from_line(&line).unwrap(), want, "{text} on stdin");
        }
        assert_eq!(code_from_line(&format!("{}\n", code())).unwrap(), want);
        assert!(code_from_line(&code().replacen("RH1-", "DRP1-", 1)).is_err());
        assert!(code_from_line("").is_err());
        let flood = "x".repeat(10 * CODE_LINE_LIMIT as usize);
        assert_eq!(read_capped_line(flood.as_bytes()).unwrap().len(), CODE_LINE_LIMIT as usize);
        assert_eq!(parse(&args(&["status"])).unwrap().action, Action::Status);
        assert_eq!(parse(&args(&["unenroll"])).unwrap().action, Action::Unenroll);
        assert!(parse(&args(&["unenroll", "--hub", "wss://hub.example"])).is_err());
        for bad in [
            &["enroll"][..],
            &["enroll", "RH1-nope", "--hub", "wss://hub.example"],
            &["enroll", "DR1-nope", "--hub", "wss://hub.example"],
            &["enroll", &code().replacen("RH1-", "DR2-", 1), "--hub", "wss://hub.example"],
            &["enroll", &code()],
            &["enroll", &code(), "--hub", "ws://hub.example"],
            &["run", "extra"],
            &["serve"],
            &["run", "--hub", "wss://hub.example"],
            &[],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
        assert!(
            parse(&args(&["enroll", &code(), "--hub=ws://127.0.0.1:9000/v1/agent"])).is_ok(),
            "plain ws on loopback, for local testing"
        );
    }

    #[tokio::test]
    async fn enrolling_needs_a_local_account_and_authentication_on() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "enrolltest");
        let refusal = refuse_enroll(&paths, None).await.expect("no account: refused");
        assert!(refusal.contains("has no local account"), "{refusal}");
        assert!(refusal.contains("user add <name> --instance enrolltest"), "says how: {refusal}");
        assert!(refusal.contains("setup-code --instance enrolltest"), "and where the code is: {refusal}");
        assert!(refusal.contains(&crate::auth::setup::code_file(&paths).display().to_string()));
        assert!(!key_path(&paths).exists(), "refused before a machine key is made");

        let users = crate::auth::users::UsersStore::new(&paths);
        users.add_app("app", "correct horse battery").await.expect("the desktop app's account");
        assert!(
            refuse_enroll(&paths, None).await.is_some(),
            "the desktop app's own account is not a user account"
        );
        users.add("ada", "correct horse battery").await.expect("account");
        assert_eq!(refuse_enroll(&paths, None).await, None, "with an account it goes ahead");
        assert_eq!(refuse_enroll(&paths, Some(true)).await, None);
        let off = refuse_enroll(&paths, Some(false)).await.expect("authentication off: refused");
        assert!(off.contains("auth.enabled = false"), "{off}");
    }

    #[test]
    fn the_identity_is_created_once_private_and_bound_to_the_instance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        let (first, created) = load_or_create(&paths).unwrap();
        assert!(created);
        let (again, created) = load_or_create(&paths).unwrap();
        assert!(!created, "a second run loads rather than mints");
        assert_eq!(first.sign_public(), again.sign_public());
        assert_eq!(first.dh_public(), again.dh_public());
        let stored = FileSeedStore::for_instance(&paths).load().unwrap().unwrap();
        assert_eq!(
            stored.uuid,
            crate::identity::ensure(&paths).unwrap().uuid,
            "recorded against this instance's identity"
        );
        assert_eq!(MachineIdentity::from_seed(&stored.seed).sign_public(), first.sign_public());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = key_path(&paths);
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load_or_create(&paths).unwrap_err().contains("readable by others"));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        // The same file copied into another instance is refused, not silently re-derived.
        let other = Paths::for_instance(dir.path().to_path_buf(), "agentother");
        let copied = key_path(&other);
        std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
        std::fs::copy(key_path(&paths), &copied).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(load_or_create(&other).unwrap_err().contains("belongs to identity"));
    }

    /// Review of the local sign-in: the command line's unenroll left
    /// `cloud.json` (the owner) and the aggregator's key behind.
    #[test]
    fn unenroll_on_the_command_line_leaves_nothing_of_droppen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        load_or_create(&paths).unwrap();
        state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5)
            .save(&paths)
            .unwrap();
        // A pending key an older release left behind.
        std::fs::create_dir_all(crate::aggregator::dir(&paths)).unwrap();
        std::fs::write(crate::aggregator::key_path(&paths), b"{}").unwrap();
        let cloud = crate::api::cloud::file_path(&paths);
        std::fs::write(&cloud, br#"{"owner":"alice"}"#).unwrap();
        let before = unenroll_instance(&paths).unwrap();
        assert_eq!(before.map(|st| st.machine).as_deref(), Some("m-1"));
        assert!(!paths.instance_dir().join("agent").exists());
        assert!(!crate::aggregator::dir(&paths).exists(), "the aggregator's key is gone");
        assert!(!cloud.exists(), "cloud.json is gone");
    }

    /// Host identity renewal (`docs/host-identity-rotation.md` §3.2, §3.5):
    /// staged seeds wait beside the one in use — one 0600 file each, by
    /// nonce, never over it, several at once — and the one a renewal names
    /// takes its place in one rename, the others going after it. At every
    /// point in between the store holds the old seed (alone, or with staged
    /// ones) or the new one (at worst with staged ones never used): what a
    /// crash leaves.
    /// R6 review: every write or rename of a staging — staging it, setting
    /// it aside, taking it back, promoting it — flushes the directory, so a
    /// crash right after never loses it (nor brings an old one back). A2:
    /// one set aside is never among the stagings; a promotion forgets it.
    #[test]
    fn every_staging_rename_is_flushed_and_one_set_aside_is_kept_apart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentsync");
        let (old, _) = load_or_create(&paths).unwrap();
        let store = FileSeedStore::for_instance(&paths);
        let uuid = crate::identity::ensure(&paths).unwrap().uuid;
        let staged = |seed: u8, nonce: u8| StagedSeed {
            uuid: uuid.clone(),
            seed: Zeroizing::new([seed; 32]),
            nonce: [nonce; 16],
            chain_seq: 4,
            chain_head: [5; 32],
            assurance: rh_proto::chain::Assurance::Auto,
            created_ms: u64::from(nonce),
            accepted_ms: Some(1),
        };
        let flushed = |what: &str, f: &dyn Fn()| {
            let before = dir_syncs();
            f();
            assert!(dir_syncs() > before, "{what} flushes the directory");
        };
        flushed("staging", &|| store.stage(&staged(7, 1)).unwrap());
        flushed("staging another", &|| store.stage(&staged(8, 2)).unwrap());
        flushed("setting one aside", &|| store.expire(&[1; 16]).unwrap());
        assert!(store.expired_path(&[1; 16]).ends_with(format!("agent/machine-key.expired-{}.json", hub::hex(&[1; 16]))));
        assert_eq!(store.stagings().unwrap().iter().map(|s| s.nonce).collect::<Vec<_>>(), vec![[2; 16]]);
        assert_eq!(store.expired().iter().map(|s| s.nonce).collect::<Vec<_>>(), vec![[1; 16]]);
        assert!(store.expire(&[1; 16]).is_ok(), "none left to set aside: nothing to do");
        flushed("taking it back", &|| store.restore(&[1; 16]).unwrap());
        assert_eq!(store.stagings().unwrap().len(), 2);
        store.expire(&[1; 16]).unwrap();
        flushed("promoting", &|| store.promote(&[2; 16]).unwrap());
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), MachineIdentity::from_seed(&[8; 32]).sign_public());
        assert_ne!(old.sign_public(), MachineIdentity::from_seed(&[8; 32]).sign_public());
        assert!(store.stagings().unwrap().is_empty() && store.expired().is_empty(), "a promotion forgets those set aside too");
        assert!(store.unreadable().is_empty());
    }

    #[test]
    fn staged_seeds_never_touch_the_one_in_use_until_one_is_promoted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        let (old, _) = load_or_create(&paths).unwrap();
        let store = FileSeedStore::for_instance(&paths);
        assert_eq!(
            store.staging_path(&[1; 16]),
            key_path(&paths).with_file_name(format!("machine-key.next-{}.json", hub::hex(&[1; 16])))
        );
        assert!(store.stagings().unwrap().is_empty() && store.staged().unwrap().is_none());
        assert!(store.promote(&[1; 16]).is_err(), "nothing staged, nothing promoted");
        let uuid = crate::identity::ensure(&paths).unwrap().uuid;
        let staged = |seed: u8, nonce: u8, created_ms: u64| StagedSeed {
            uuid: uuid.clone(),
            seed: Zeroizing::new([seed; 32]),
            nonce: [nonce; 16],
            chain_seq: 4,
            chain_head: [5; 32],
            assurance: rh_proto::chain::Assurance::OnDevice,
            created_ms,
            accepted_ms: None,
        };
        store.stage(&staged(7, 1, 6)).unwrap();
        // Crash here: the old seed is the one in use.
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), old.sign_public());
        let read = store.staged().unwrap().unwrap();
        assert_eq!((read.nonce, read.chain_seq, read.chain_head, read.created_ms, read.accepted_ms), ([1; 16], 4, [5; 32], 6, None));
        assert_eq!(read.assurance, rh_proto::chain::Assurance::OnDevice);
        assert_eq!(read.identity().sign_public(), MachineIdentity::from_seed(&[7; 32]).sign_public());
        let file: serde_json::Value = serde_json::from_slice(&std::fs::read(store.staging_path(&[1; 16])).unwrap()).unwrap();
        for field in ["version", "uuid", "seed", "nonce", "chainSeq", "chainHead", "assurance", "createdMs"] {
            assert!(file.get(field).is_some(), "{field}: {file}");
        }
        // Taken by the hub: noted in the same file.
        store.stage(&StagedSeed { accepted_ms: Some(9), ..staged(7, 1, 6) }).unwrap();
        assert_eq!(store.stagings().unwrap().len(), 1, "the same nonce again: the same staging");
        assert_eq!(store.staged().unwrap().unwrap().accepted_ms, Some(9));
        // A second staging waits beside the first: neither replaces the other.
        store.stage(&staged(8, 2, 7)).unwrap();
        let both: Vec<[u8; 16]> = store.stagings().unwrap().iter().map(|s| s.nonce).collect();
        assert_eq!(both, vec![[1; 16], [2; 16]], "oldest first");
        assert_eq!(store.staged().unwrap().unwrap().nonce, [2; 16], "the newest");
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), old.sign_public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let first = store.staging_path(&[1; 16]);
            assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);
            std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(store.stagings().unwrap().len(), 1, "one others can read is not used");
            assert!(store.promote(&[1; 16]).is_err(), "nor taken");
            std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        // The FIRST is the one a renewal names: promoted; the other goes.
        store.promote(&[1; 16]).unwrap();
        assert!(store.stagings().unwrap().is_empty());
        let (now, created) = load_or_create(&paths).unwrap();
        assert!(!created);
        assert_eq!(now.sign_public(), MachineIdentity::from_seed(&[7; 32]).sign_public());
        assert_eq!(store.load().unwrap().unwrap().uuid, uuid, "the same instance");
        // Forgetting staged seeds leaves the one in use.
        store.stage(&staged(9, 3, 8)).unwrap();
        store.stage(&staged(10, 4, 9)).unwrap();
        store.discard_staged(&[3; 16]).unwrap();
        store.discard_staged(&[3; 16]).unwrap();
        assert_eq!(store.stagings().unwrap().len(), 1);
        store.discard_all_staged().unwrap();
        assert!(store.stagings().unwrap().is_empty());
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), now.sign_public());
    }

    #[test]
    fn the_status_shows_a_renewal_and_its_code() {
        let code = rotation::Status {
            state: "staged".into(),
            code: Some("ABCD-EFGH".into()),
            code_expires_ms: Some(10 * 60_000),
            ..Default::default()
        };
        let lines = rotation_lines(&code, 0);
        assert!(lines.iter().any(|l| l.contains("ABCD-EFGH") && l.contains("10 more minute")), "{lines:?}");
        let foreign = rotation::Status {
            state: "foreign".into(),
            reason: Some(rotation::FOREIGN_RENEWED_ELSEWHERE.into()),
            ..Default::default()
        };
        let said = rotation_lines(&foreign, 0).join("\n");
        assert!(said.contains("replaced elsewhere") && said.contains("backup") && said.contains("new identity"), "{said}");
        assert!(!said.to_lowercase().contains("impersonat") && !said.to_lowercase().contains("hijack"));
        assert!(rotation_lines(&rotation::Status::default(), 0).is_empty());
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        load_or_create(&paths).unwrap();
        let path = key_path(&paths);
        let uuid = crate::identity::ensure(&paths).unwrap().uuid;
        std::fs::write(&path, format!(r#"{{"version":1,"uuid":"{uuid}","seed":"abcd"}}"#)).unwrap();
        assert!(load_or_create(&paths).unwrap_err().contains("not 32 bytes"));
    }
}
