use rh_proto::identity::{display_fingerprint, fingerprint_text, EnrollCode, MachineIdentity};

use super::now_ms;
use super::seed::{load_or_create, FileSeedStore};
use super::{control, enroll, hub, rotation, state};
use crate::sessions::Paths;

/// What to print when the subcommand does not parse.
pub const USAGE: &str = "\
USAGE:
    remote-hand-server agent enroll <code> --hub <wss-url> [--instance <instance>]
    remote-hand-server agent enroll - --hub <wss-url> [--instance <instance>]
    remote-hand-server agent run [--instance <instance>]
    remote-hand-server agent status [--instance <instance>]
    remote-hand-server agent unenroll [--instance <instance>]

The cloud agent. `enroll` joins this instance to an account. The instance must
have a local account first (`remote-hand-server user add <name>`, or the setup
page): a linked device reaches other devices from its page, so that page must
ask who is there. It creates the instance's machine identity key on first use
(<data>/instances/<instance>/agent/machine-key.json, mode 0600), presents the
code's token to the hub, proves it holds the code, and waits until your device
adds this machine to the account's device chain. With `-` for the code, the
code is read from standard input instead (asked for without echo on a
terminal, else the first line piped in), so it never shows in the process list
other users of this device can read. It exits 0 once the device is added, 76
when the hub took the code but the page has not added the device yet (`serve`
then finishes it), and 1 otherwise. (The local settings' \"Sign in
to Droppen\" does the same without a command line or a restart.) `run` serves
the instance with its hub connection (an enrolled instance's plain `serve`
connects too). `status` says where the instance stands. `unenroll` deletes
everything under <instance>/agent/ — the instance is a server on its own
again; remove the machine on droppen.app too — and needs the instance's server
stopped (or use the local settings, which do it live). An instance that is
never enrolled serves exactly as before.";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Enroll(EnrollCode),
    /// `enroll -`: the code is on the first line of stdin.
    EnrollFromStdin,
    Run,
    Status,
    Unenroll,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Invocation {
    pub(super) action: Action,
    /// The hub's origin (`enroll` only).
    pub(super) hub: Option<String>,
}

pub(super) fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut rest = Vec::new();
    let mut hub = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--hub=") {
            hub = Some(value.to_string());
        } else if arg == "--hub" {
            hub = Some(iter.next().ok_or("--hub needs a url")?.clone());
        } else {
            rest.push(arg.as_str());
        }
    }
    let hub = hub.map(|h| hub::origin(&h)).transpose()?;
    let action = match rest.as_slice() {
        ["enroll", "-"] => Action::EnrollFromStdin,
        ["enroll", code] => Action::Enroll(code.parse().map_err(|e: &str| format!("{e}\n\n{USAGE}"))?),
        ["run"] => Action::Run,
        ["status"] => Action::Status,
        ["unenroll"] => Action::Unenroll,
        _ => return Err(USAGE.to_string()),
    };
    let enrolling = matches!(action, Action::Enroll(_) | Action::EnrollFromStdin);
    if enrolling && hub.is_none() {
        return Err(format!("enroll needs the hub: --hub wss://<hub>\n\n{USAGE}"));
    }
    if !enrolling && hub.is_some() {
        return Err("--hub is only for enroll: the other subcommands use the hub this instance enrolled with".into());
    }
    Ok(Invocation { action, hub })
}
pub(super) fn describe(paths: &Paths, identity: &MachineIdentity, created: bool) {
    println!("Droppen agent — instance {}", paths.instance());
    println!(
        "device identity {} {}",
        if created { "created at" } else { "loaded from" },
        paths.machine_key_path().display()
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

/// `agent unenroll` (the server stopped): everything this instance kept for
/// Droppen goes — `agent/`, the aggregator's key (`aggregator/`) and the
/// local link's settings (`cloud.json`), as the local settings' "Disconnect"
/// does. The state it had, if any.
pub(super) fn unenroll_instance(paths: &Paths) -> Result<Option<state::AgentState>, String> {
    let before = state::AgentState::load(paths).ok().flatten();
    paths.unenroll_agent_files()?;
    crate::aggregator::forget_all(paths)?;
    let cloud = crate::api::cloud::file_path(paths);
    match std::fs::remove_file(&cloud) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", cloud.display())),
    }
    Ok(before)
}

/// `agent status`: where this instance stands, from its files.
pub(super) fn print_status(paths: &Paths) {
    let status = match state::AgentState::load(paths) {
        Ok(Some(st)) => control::AgentStatus::of_instance(paths, &st),
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
    for line in rotation_lines(&status.rotation, now_ms()) {
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
    if !paths.machine_key_path().exists() {
        return Err("no machine key".into());
    }
    load_or_create(paths)
}

/// The longest line `agent enroll -` reads: a code is about 60 characters.
pub(super) const CODE_LINE_LIMIT: u64 = 1024;

/// `agent enroll -`: the code from standard input — asked for without echo on
/// a terminal, else the first line of what is piped in, read no further than
/// [`CODE_LINE_LIMIT`].
pub(super) fn read_code_from_stdin() -> Result<EnrollCode, String> {
    let line = if crate::auth::cli::stdin_is_a_terminal() {
        crate::auth::cli::read_secret("One-time code from droppen.app: ").map_err(|e| format!("could not read the code: {e}"))?
    } else {
        read_capped_line(std::io::stdin().lock())?
    };
    code_from_line(&line)
}

/// The code on the line `agent enroll -` read.
pub(super) fn code_from_line(line: &str) -> Result<EnrollCode, String> {
    line.trim().parse().map_err(|e: &str| format!("the code on standard input: {e}"))
}

pub(super) fn read_capped_line(input: impl std::io::Read) -> Result<String, String> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::BufReader::new(input.take(CODE_LINE_LIMIT))
        .read_line(&mut line)
        .map_err(|e| format!("could not read the code from standard input: {e}"))?;
    Ok(line)
}

/// The exit status of `agent enroll` when the hub took the code but the
/// droppen.app page has not added this device yet: the enrolling state is
/// saved and `serve` finishes it once the page does. Anything else that is
/// not success exits 1.
pub const ENROLL_PENDING_EXIT: u8 = 76;

/// `agent enroll`, the code given either way.
pub(super) async fn enroll_cli(paths: &Paths, code: &EnrollCode, hub: &str) -> Result<(), String> {
    let before = FileSeedStore::for_instance(paths)
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
    match paths.enroll(&identity, code, hub).await {
        Ok(()) => Ok(()),
        Err(enroll::EnrollFailure::Failed(message)) => Err(message),
        Err(enroll::EnrollFailure::Pending(message)) => {
            // Not a failure and not done: its own exit status, so a script
            // (the installer) can start the server, which finishes it.
            let name = crate::program::Name::invoked();
            eprintln!("{}: {}", name.error_prefix(), name.rewrite(&message));
            std::process::exit(i32::from(ENROLL_PENDING_EXIT));
        }
    }
}

/// Why `agent enroll` will not link this instance, if it will not.
///
/// A device linked to Droppen reaches other devices from its page, so that
/// page must ask who is there: the instance needs a local account, and
/// authentication must not be switched off — the same two rules the local
/// settings' "Sign in to Droppen" has (`crate::api::cloud`). Checked before
/// the code is read, so nobody types a code into a command that will refuse it.
pub(super) async fn refuse_enroll(paths: &Paths, auth_enabled: Option<bool>) -> Option<String> {
    let name = paths.instance();
    if auth_enabled == Some(false) {
        return Some(format!(
            "instance {name} is configured with local authentication off (auth.enabled = false): its page would answer anyone who can reach it, and a device linked to Droppen reaches other devices from its page.\n\
             Remove auth.enabled / APP_AUTH__ENABLED=false first."
        ));
    }
    // A user account: the desktop app's own login does not count, for the
    // reason it does not end the setup state.
    if crate::auth::users::UsersStore::new(paths).has_users().await {
        return None;
    }
    let code_file = crate::auth::setup::code_file(paths);
    Some(format!(
        "instance {name} has no local account, and a device linked to Droppen must have one: other devices are reachable from its page.\n\
         Create one first, either here:\n    \
             remote-hand-server user add <name> --instance {name}\n\
         or on its page: open it with remote-hand-server open --instance {name}, or with the\n\
         one-time setup code its running server keeps (remote-hand-server setup-code --instance {name};\n\
         in {} while it runs).\n\
         Then run this again.",
        code_file.display()
    ))
}

/// Run an `agent` subcommand. `args` is everything after `agent`.
pub async fn run(args: &[String]) -> Result<(), String> {
    let (args, instance) = crate::auth::cli::take_instance(args)?;
    let invocation = parse(&args)?;
    let paths = crate::auth::cli::home_paths(instance.as_deref())?;
    if matches!(invocation.action, Action::Enroll(_) | Action::EnrollFromStdin) {
        let config = crate::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok()).map_err(|error| error.to_string())?;
        if let Some(refusal) = refuse_enroll(&paths, config.app.auth.enabled()).await {
            return Err(refusal);
        }
    }
    match invocation.action {
        Action::EnrollFromStdin => {
            let code = read_code_from_stdin()?;
            enroll_cli(&paths, &code, invocation.hub.as_deref().expect("parse requires it")).await
        }
        Action::Enroll(code) => enroll_cli(&paths, &code, invocation.hub.as_deref().expect("parse requires it")).await,
        Action::Status => {
            print_status(&paths);
            Ok(())
        }
        Action::Unenroll => {
            // A running server holds the link and its files open, and would
            // write them back: it is stopped first, or the local settings do it.
            let dirs = crate::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).map_err(|e| e.to_string())?;
            let _lock = crate::instance::InstanceLock::acquire(&dirs).map_err(|e| {
                format!("{e}\nStop the instance's server first, or disconnect it from its settings (Droppen → Disconnect this device).")
            })?;
            let before = unenroll_instance(&paths)?;
            match before {
                Some(st) => println!(
                    "unenrolled instance {}: it was machine {} in account {} at {}; everything under {}, its aggregator \
                     key and cloud.json are gone.\n\
                     Remove the machine on droppen.app too, if it is still listed there.",
                    paths.instance(),
                    st.machine,
                    st.account,
                    st.hub,
                    paths.instance_dir().join("agent").display()
                ),
                None => println!("instance {} was not enrolled; nothing under agent/ now", paths.instance()),
            }
            Ok(())
        }
        Action::Run => {
            let enrolled = state::AgentState::load(&paths)?;
            let Some(state) = enrolled else {
                return Err(format!(
                    "instance {} is not enrolled; run `remote-hand-server agent enroll <code> --hub <url>` first",
                    paths.instance()
                ));
            };
            println!(
                "serving instance {} with its link to {} (device {}, account {})",
                paths.instance(),
                state.hub,
                state.machine,
                state.account
            );
            crate::app::RemoteHandApp {
                instance_override: Some(paths.instance().to_string()),
                ..Default::default()
            }
            .run_server()
            .await
            .map_err(|e| e.to_string())
        }
    }
}
