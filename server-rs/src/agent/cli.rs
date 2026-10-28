//! The agent's half of `agent enroll` and `agent status`: what they do with,
//! and print from, this instance's agent files, which only this module names.
//! The command line itself is [`crate::cli::agent`].

use rh_proto::identity::{display_fingerprint, fingerprint_text, EnrollCode, MachineIdentity};

use super::seed::{load_or_create, FileSeedStore};
use super::{control, enroll, rotation, state, AgentFiles};
use crate::paths::Paths;
use droppen_base::time::now_ms_u64;

pub(super) fn describe(paths: &Paths, identity: &MachineIdentity, created: bool) {
    println!("Droppen agent — instance {}", paths.instance());
    println!(
        "device identity {} {}",
        if created { "created at" } else { "loaded from" },
        AgentFiles::of(paths).machine_key().display()
    );
    println!(
        "  signing key    {}  (Ed25519: hub sign-in, stream manifests, push payloads)",
        fingerprint_text(&identity.sign_public())
    );
    println!(
        "  channel key    {}  (X25519: Noise static key; MK is sealed to it)",
        fingerprint_text(&identity.dh_public())
    );
}

/// `agent status`: where this instance stands, from its files.
pub(crate) fn print_status(paths: &Paths) {
    let files = AgentFiles::of(paths);
    let status = match state::AgentState::load(&files) {
        Ok(Some(st)) => control::AgentStatus::of_instance(&files, &st),
        Ok(None) => {
            println!("instance {}: not enrolled (a server on its own)", paths.instance());
            return;
        }
        Err(error) => {
            println!("instance {}: the agent state does not load: {error}", paths.instance());
            return;
        }
    };
    let state = match status.state {
        control::Standing::Linked => "enrolled",
        control::Standing::NotHosting => {
            "enrolled, not running AI in this account (it writes no streams and accepts no channels until a device makes it run AI again)"
        }
        control::Standing::Enrolling => "enrolling (waiting for the page that showed the code to add this device)",
        control::Standing::Removed => "removed from the account (the link does not run)",
        control::Standing::None | control::Standing::Broken => "unknown",
    };
    println!("instance {}: {state}", paths.instance());
    let show = |label: &str, value: Option<String>| {
        if let Some(value) = value {
            println!("  {label:<12} {value}");
        }
    };
    show("hub", status.hub);
    show("account", status.account);
    show("account key", status.account_fingerprint);
    show("device id", status.machine);
    show("chain seq", status.chain_seq.map(|s| s.to_string()));
    show("devices", status.devices.map(|n| n.to_string()));
    show("run AI", status.machines.map(|n| n.to_string()));
    show("machine key", status.mk_epoch.map(|e| format!("epoch {e}")));
    if let Ok((identity, _)) = load_or_create_existing(paths) {
        show("fingerprint", Some(control::machine_fingerprint(&identity)));
    }
    for line in rotation_lines(&status.rotation, now_ms_u64()) {
        println!("  {line}");
    }
    for alarm in &status.alarms {
        println!("  ALARM        {}", alarm.what);
    }
    for change in &status.changes {
        println!("  change       {}", change.what);
    }
}

/// What `agent status` says about renewing this device's identity key: on a
/// server without a screen, the code of a high-assurance renewal is read
/// here (over SSH) and typed on the admin device.
pub(super) fn rotation_lines(rotation: &rotation::Status, now: u64) -> Vec<String> {
    let minutes = |until: u64| until.saturating_sub(now).div_ceil(60_000);
    match rotation.state.as_str() {
        "staged" => match (&rotation.code, rotation.code_expires_ms) {
            (Some(code), Some(until)) => vec![
                "renewal      a new identity key is staged; the admin device asks you to confirm it".to_string(),
                format!("CODE         {code}  (type it on the admin device; shown for {} more minute(s))", minutes(until)),
            ],
            _ => vec!["renewal      a new identity key is staged, waiting for the renewal to be written".to_string()],
        },
        "renewed" => vec![format!(
            "renewal      identity key renewed{}",
            rotation
                .previous_fingerprint
                .as_ref()
                .map(|fp| format!(" (it was {fp})"))
                .unwrap_or_default()
        )],
        "foreign" => vec![
            format!("renewal      {}", rotation::REPLACED_ELSEWHERE),
            "             It does not connect meanwhile. On an admin device remove this device's old id; then here: \
             the local settings' Droppen section, \"add this device again with a new identity\""
                .to_string(),
        ],
        "refused" => vec![
            "renewal      the hub refuses this device's identity key as renewed but showed no renewal; it tries again".to_string(),
        ],
        "unreadable" => vec![format!("renewal      {}", rotation::STAGED_KEY_UNREADABLE)],
        _ => Vec::new(),
    }
}

/// The machine identity if it exists (never made here).
pub(super) fn load_or_create_existing(paths: &Paths) -> Result<(MachineIdentity, bool), String> {
    if !AgentFiles::of(paths).machine_key().exists() {
        return Err("no machine key".into());
    }
    load_or_create(paths)
}

/// The exit status of `agent enroll` when the hub took the code but the
/// droppen.app page has not added this device yet: the enrolling state is
/// saved and `serve` finishes it once the page does. Anything else that is
/// not success exits 1.
pub const ENROLL_PENDING_EXIT: u8 = 76;

/// `agent enroll`, the code given either way.
pub(crate) async fn enroll_cli(paths: &Paths, code: &EnrollCode, hub: &str) -> Result<(), String> {
    let files = AgentFiles::of(paths);
    let before = FileSeedStore::for_instance(&files)
        .load()
        .ok()
        .flatten()
        .map(|s| MachineIdentity::from_seed(&s.seed).sign_public());
    // A seed a renewal retired (or one archived) never signs up again.
    let identity = rotation::identity_for_enrolling(paths)?;
    let created = before != Some(identity.sign_public());
    describe(paths, &identity, created);
    println!(
        "enrolling with account key {} (the code's secret never leaves this device)",
        display_fingerprint(&code.account_fingerprint)
    );
    match files.enroll(&identity, code, hub).await {
        Ok(()) => Ok(()),
        Err(enroll::EnrollFailure::Failed(message)) => Err(message),
        Err(enroll::EnrollFailure::Pending(message)) => {
            // Not a failure and not done: its own exit status, so a script
            // (the installer) can start the server, which finishes it.
            let name = droppen_base::program::Name::invoked();
            eprintln!("{}: {}", name.error_prefix(), name.rewrite(&message));
            std::process::exit(i32::from(ENROLL_PENDING_EXIT));
        }
    }
}
