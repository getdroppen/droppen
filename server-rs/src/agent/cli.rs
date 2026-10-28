use rh_proto::identity::{display_fingerprint, fingerprint_text, EnrollCode, MachineIdentity};

use clap::{Args, Subcommand};

use super::seed::{load_or_create, FileSeedStore};
use super::{control, enroll, hub, rotation, state, AgentFiles};
use crate::paths::Paths;
use crate::time::now_ms_u64;

/// The rest of `agent --help`, after the options clap lists.
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

/// The cloud agent's command line.
#[derive(Debug, PartialEq, Eq, Args)]
#[command(args_override_self = true)]
pub struct AgentArgs {
    /// The hub's origin (`enroll` only).
    #[arg(long, global = true, allow_hyphen_values = true)]
    pub hub: Option<String>,
    /// The instance, wherever among the arguments it is written.
    #[arg(long, global = true, action = clap::ArgAction::Append, value_parser = crate::auth::cli::instance_name, allow_hyphen_values = true)]
    pub instance: Vec<String>,
    /// The action to take.
    #[command(subcommand)]
    pub command: AgentCommand,
}

/// The agent's actions.
#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum AgentCommand {
    /// Join this instance to an account with a code a device issued.
    Enroll {
        /// The code, or `-` to read it from standard input.
        #[arg(value_name = "code", allow_hyphen_values = true)]
        code: String,
    },
    /// Serve with this instance's hub connection.
    Run,
    /// Say where this instance stands.
    Status,
    /// Leave the account and delete what the agent kept.
    Unenroll,
}

impl AgentArgs {
    /// The hub's origin, the checked code and the enrol/other invariant,
    /// before any file or network work.
    pub(super) fn checked(&self) -> Result<(Option<String>, Option<EnrollCode>), String> {
        let hub = self.hub.as_deref().map(hub::origin).transpose()?;
        let code = match &self.command {
            AgentCommand::Enroll { code } if code != "-" => {
                Some(code.parse::<EnrollCode>().map_err(|error: &str| error.to_string())?)
            }
            _ => None,
        };
        let enrolling = matches!(self.command, AgentCommand::Enroll { .. });
        if enrolling && hub.is_none() {
            return Err("enroll needs the hub: --hub wss://<hub>".into());
        }
        if !enrolling && hub.is_some() {
            return Err("--hub is only for enroll: the other subcommands use the hub this instance enrolled with".into());
        }
        Ok((hub, code))
    }

    /// Run the chosen action.
    pub async fn run(self) -> Result<(), String> {
        let (hub, code) = self.checked()?;
        let enrolling = matches!(self.command, AgentCommand::Enroll { .. });
        let paths = crate::auth::cli::home_paths(self.instance.last().map(String::as_str))?;
        if enrolling {
            let config = crate::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok()).map_err(|error| error.to_string())?;
            if let Some(refusal) = refuse_enroll(&paths, config.app.auth.enabled()).await {
                return Err(refusal);
            }
        }
        match self.command {
            AgentCommand::Enroll { code } if code == "-" => {
                let code = read_code_from_stdin()?;
                enroll_cli(&paths, &code, hub.as_deref().expect("enrolling requires a hub")).await
            }
            AgentCommand::Enroll { .. } => {
                enroll_cli(&paths, code.as_ref().expect("parsed above"), hub.as_deref().expect("enrolling requires a hub")).await
            }
            AgentCommand::Status => {
                print_status(&paths);
                Ok(())
            }
            AgentCommand::Unenroll => {
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
                        AgentFiles::of(&paths).dir().display()
                    ),
                    None => println!("instance {} was not enrolled; nothing under agent/ now", paths.instance()),
                }
                Ok(())
            }
            AgentCommand::Run => {
                let enrolled = state::AgentState::load(&AgentFiles::of(&paths))?;
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
}

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

/// `agent unenroll` (the server stopped): everything this instance kept for
/// Droppen goes — `agent/`, the aggregator's key (`aggregator/`) and the
/// local link's settings (`cloud.json`), as the local settings' "Disconnect"
/// does. The state it had, if any.
pub(super) fn unenroll_instance(paths: &Paths) -> Result<Option<state::AgentState>, String> {
    let files = AgentFiles::of(paths);
    let before = state::AgentState::load(&files).ok().flatten();
    files.unenroll()?;
    crate::aggregator::forget_all(paths)?;
    let cloud = crate::cloud::LinkFile::of(paths).path();
    match std::fs::remove_file(&cloud) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", cloud.display())),
    }
    Ok(before)
}

/// `agent status`: where this instance stands, from its files.
pub(super) fn print_status(paths: &Paths) {
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
/// settings' "Sign in to Droppen" has (`crate::cloud`). Checked before
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

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    fn parse(words: &[String]) -> Result<AgentArgs, clap::Error> {
        match crate::cli::tests::parse_words("agent", words)? {
            crate::cli::Command::Agent(parsed) => Ok(parsed),
            other => panic!("{words:?} parsed as {other:?}"),
        }
    }

    /// Parsing and the meaning checks the command applies before it runs.
    fn checked(words: &[String]) -> Result<AgentArgs, String> {
        let args = parse(words).map_err(|error| error.to_string())?;
        args.checked()?;
        Ok(args)
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// A code `EnrollCode` accepts, so the table can tell "the code was refused" from "the shape was refused".
    fn code() -> String {
        EnrollCode::generate([0x11; 16], &mut rand::thread_rng()).to_string()
    }

    /// The accepted command lines; the refused ones are a separate list in `the_refusals`.
    #[test]
    fn the_agent_argument_table() {
        let code = code();
        let accepted: Vec<Vec<String>> = vec![
            args(&["status"]),
            args(&["run"]),
            args(&["unenroll"]),
            vec!["enroll".into(), code.clone(), "--hub".into(), "wss://hub.example".into()],
            vec!["enroll".into(), "-".into(), "--hub".into(), "wss://hub.example".into()],
            vec!["--hub".into(), "wss://hub.example".into(), "enroll".into(), code.clone()],
            vec!["enroll".into(), "--hub".into(), "wss://hub.example".into(), code.clone()],
            vec!["enroll".into(), format!("--hub=wss://hub.example"), code.clone()],
            vec![
                "enroll".into(),
                code.clone(),
                "--hub".into(),
                "wss://a.example".into(),
                "--hub".into(),
                "wss://b.example/path".into(),
            ],
            args(&["status", "--instance", "probe"]),
            args(&["--instance", "probe", "status"]),
            args(&["status", "--instance=probe"]),
            args(&["--instance=probe", "status"]),
            args(&["--instance", "probe", "enroll", "-", "--hub", "wss://hub.example"]),
            args(&["enroll", "-", "--hub", "wss://hub.example", "--instance", "probe"]),
            vec!["enroll".into(), "--hub".into(), "wss://hub.example".into(), "--".into(), code.clone()],
            args(&["status", "--instance", "--"]),
            args(&["status", "--instance", "--instance"]),
            args(&["enroll", "-", "--hub", "wss://hub.example", "--instance", "--hub"]),
        ];
        for argv in &accepted {
            assert!(checked(argv).is_ok(), "{argv:?} must parse: {:?}", checked(argv).err());
        }
        let parsed = parse(&args(&["status"])).expect("parsed");
        assert!(matches!(parsed.command, AgentCommand::Status));
        let parsed = parse(&args(&["enroll", "-", "--hub", "wss://hub.example", "--instance", "probe"])).expect("parsed");
        assert!(matches!(parsed.command, AgentCommand::Enroll { .. }));
        assert_eq!(parsed.hub.as_deref(), Some("wss://hub.example"));
        assert_eq!(parsed.instance.last().map(String::as_str), Some("probe"));
    }

    #[test]
    fn the_refusals() {
        let code = code();
        // clap's refusals.
        for (argv, expected) in [
            (args(&[]), ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
            (args(&["--"]), ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
            (args(&["bogus"]), ErrorKind::InvalidSubcommand),
            (args(&["enroll"]), ErrorKind::MissingRequiredArgument),
            (vec!["enroll".into(), code.clone(), "--hub".into()], ErrorKind::InvalidValue),
            (args(&["enroll", "-", "--", "--hub", "wss://hub.example"]), ErrorKind::UnknownArgument),
            (args(&["enroll", "--", "-", "--hub", "wss://hub.example"]), ErrorKind::UnknownArgument),
            (args(&["enroll", "-", "extra", "--hub", "wss://hub.example"]), ErrorKind::UnknownArgument),
            (args(&["status", "extra"]), ErrorKind::UnknownArgument),
            (args(&["status", "--instance"]), ErrorKind::InvalidValue),
            (args(&["status", "--instance", "Bad Name"]), ErrorKind::ValueValidation),
            (args(&["--instance", "Bad Name", "status"]), ErrorKind::ValueValidation),
            // `--hub` takes the next word, `--instance` included.
            (args(&["enroll", "-", "--hub", "--instance", "probe", "wss://hub.example"]), ErrorKind::UnknownArgument),
            (args(&["enroll", "-", "--hub", "--instance=probe", "wss://hub.example"]), ErrorKind::UnknownArgument),
            (args(&["--hub", "--instance", "probe", "wss://hub.example", "enroll", "-"]), ErrorKind::InvalidSubcommand),
            (args(&["--hub", "--instance=probe", "wss://hub.example", "enroll", "-"]), ErrorKind::InvalidSubcommand),
            (args(&["enroll", "-", "--hub", "--instance", "Bad", "wss://hub.example"]), ErrorKind::UnknownArgument),
            (args(&["enroll", "-", "--hub", "--instance", "other", "ws://127.0.0.1:1"]), ErrorKind::UnknownArgument),
            (args(&["--hub", "wss://hub.example"]), ErrorKind::MissingSubcommand),
        ] {
            assert_eq!(parse(&argv).unwrap_err().kind(), expected, "{argv:?}");
        }
        // The refusals of a parsed command line, before any file or network work.
        for (argv, expected) in [
            (vec!["enroll".into(), code.clone(), "--hub=".into()], hub::origin("").unwrap_err()),
            (
                args(&["enroll", "nonsense", "--hub", "wss://hub.example"]),
                "nonsense".parse::<EnrollCode>().unwrap_err().to_string(),
            ),
            (args(&["enroll", "nonsense"]), "nonsense".parse::<EnrollCode>().unwrap_err().to_string()),
            (args(&["enroll", "-", "--hub", "ftp://hub.example"]), hub::origin("ftp://hub.example").unwrap_err()),
            (args(&["enroll", "-", "--hub", "http://hub.example"]), hub::origin("http://hub.example").unwrap_err()),
        ] {
            assert_eq!(checked(&argv).unwrap_err(), expected, "{argv:?}");
        }
        let refused = checked(&args(&["enroll", "-", "--hub", "ftp://hub.example"])).unwrap_err();
        assert!(refused.contains("a hub is reached over wss://"), "{refused}");
    }

    /// `--instance` and `--hub` are read wherever they are written, and the
    /// last of each is the one used.
    #[test]
    fn the_instance_and_the_hub_are_wherever_they_were_written() {
        for (argv, hub, instance) in [
            (args(&["enroll", "-", "--hub", "wss://hub.example", "--instance", "probe"]), "wss://hub.example", "probe"),
            (args(&["enroll", "-", "--instance", "probe", "--hub", "wss://hub.example"]), "wss://hub.example", "probe"),
            (args(&["--hub", "wss://hub.example", "--instance", "probe", "enroll", "-"]), "wss://hub.example", "probe"),
            (args(&["--instance=probe", "enroll", "-", "--hub", "wss://hub.example"]), "wss://hub.example", "probe"),
            (args(&["enroll", "-", "--hub", "ws://127.0.0.1:1", "--instance", "other"]), "ws://127.0.0.1:1", "other"),
        ] {
            let parsed = checked(&argv).unwrap_or_else(|error| panic!("{argv:?} must parse: {error}"));
            assert_eq!(parsed.checked().expect("checked").0.as_deref(), Some(hub), "{argv:?}");
            assert_eq!(parsed.instance.last().map(String::as_str), Some(instance), "{argv:?}");
            assert_eq!(parsed.command, AgentCommand::Enroll { code: "-".to_string() }, "{argv:?}");
        }

        let parsed = parse(&args(&["--instance=bob", "status"])).expect("parsed");
        assert_eq!(parsed.instance.last().map(String::as_str), Some("bob"));
        assert_eq!(parsed.command, AgentCommand::Status);

        let code = code();
        let parsed = parse(&[
            "enroll".to_string(),
            code.clone(),
            "--hub".to_string(),
            "wss://a.example".to_string(),
            "--hub".to_string(),
            "wss://b.example".to_string(),
        ])
        .expect("parsed");
        assert_eq!(parsed.hub.as_deref(), Some("wss://b.example"), "the last --hub wins");
        assert_eq!(parsed.command, AgentCommand::Enroll { code });
    }

    /// Help is clap's; `--help` and `-h` are the DisplayHelp exit, not a parse error of the grammar.
    #[test]
    fn help_is_claps() {
        for argv in [vec!["--help"], vec!["-h"], vec!["--help", "junk"], vec!["enroll", "--help"], vec!["status", "-h"]] {
            assert_eq!(parse(&args(&argv)).unwrap_err().kind(), ErrorKind::DisplayHelp, "{argv:?}");
        }
    }

    /// Every `--instance` is validated (an invalid earlier one is refused even
    /// when a good one follows) and the last one is used. A value may start
    /// with a hyphen, because instance names may.
    #[test]
    fn every_instance_is_validated_and_the_last_is_used() {
        for (argv, expected) in [
            (args(&["status", "--instance", "Bad", "--instance", "probe"]), ErrorKind::ValueValidation),
            (args(&["--instance", "Bad", "--instance", "probe", "status"]), ErrorKind::ValueValidation),
            (args(&["status", "--instance=Bad", "--instance=probe"]), ErrorKind::ValueValidation),
            (args(&["status", "--instance=", "--instance", "probe"]), ErrorKind::ValueValidation),
            (args(&["status", "--instance", "", "--instance", "probe"]), ErrorKind::ValueValidation),
            (args(&["status", "--instance", "a b", "--instance", "probe"]), ErrorKind::ValueValidation),
            (args(&["status", "--instance"]), ErrorKind::InvalidValue),
            (
                args(&["enroll", "-", "--instance", "Bad", "--instance", "probe", "--hub", "wss://hub.example"]),
                ErrorKind::ValueValidation,
            ),
        ] {
            assert_eq!(parse(&argv).unwrap_err().kind(), expected, "{argv:?}");
        }
        let parsed = parse(&args(&["status", "--instance", "bob", "--instance", "carol"])).expect("parsed");
        assert_eq!(parsed.instance.last().map(String::as_str), Some("carol"));
        let parsed = parse(&args(&["status", "--instance", "--hub"])).expect("parsed");
        assert_eq!(parsed.instance.last().map(String::as_str), Some("--hub"));
    }
}
