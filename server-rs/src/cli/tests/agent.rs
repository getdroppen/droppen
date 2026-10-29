//! The `agent` command line as this binary parses and runs it (`cli::agent`),
//! against this instance's agent files (`crate::agent`).

use droppen_instance::paths::Paths;
use rh_proto::identity::EnrollCode;

use super::super::agent::tests::parse;
use super::super::agent::*;
use super::args;
use crate::agent::{hub, load_or_create, state, AgentFiles};

fn code() -> String {
    EnrollCode {
        account_fingerprint: [9; 16],
        secret: [7; 16],
    }
    .to_string()
}

#[test]
fn the_subcommands_parse_and_a_missing_action_is_an_error() {
    let parsed = parse(&args(&["run"])).expect("run");
    assert!(matches!(parsed.command, AgentCommand::Run));

    let enroll = parse(&args(&["enroll", &code(), "--hub", "wss://hub.example/v1/agent"])).expect("enroll");
    assert_eq!(enroll.checked().expect("checked").0.as_deref(), Some("wss://hub.example"));
    assert!(matches!(enroll.command, AgentCommand::Enroll { ref code } if code.parse::<EnrollCode>().map(|c| c.secret) == Ok([7; 16])));

    assert!(parse(&args(&["enroll", "-", "--hub", "wss://hub.example"])).is_ok(), "`-`: the code comes on stdin");
    assert_eq!(
        parse(&args(&["enroll", "-"])).expect("parses").checked().unwrap_err(),
        "enroll needs the hub: --hub wss://<hub>",
        "still needs the hub"
    );
    // Piped in: one line, and never more than the cap however much follows.
    assert_eq!(read_capped_line(format!("{}\nmore", code()).as_bytes()).unwrap().trim(), code());
    // The same code with the newer `DR1-` prefix, in any case, as an argument or piped in.
    let dr = code().replacen("RH1-", "DR1-", 1);
    let want = EnrollCode {
        account_fingerprint: [9; 16],
        secret: [7; 16],
    };
    for text in [dr.clone(), dr.to_lowercase()] {
        let parsed = parse(&args(&["enroll", &text, "--hub", "wss://hub.example"])).expect("enroll");
        assert!(matches!(parsed.command, AgentCommand::Enroll { ref code } if code.parse::<EnrollCode>().map(|c| c.secret) == Ok(want.secret)), "{text}");
        let line = read_capped_line(format!("{text}\r\nmore").as_bytes()).unwrap();
        assert_eq!(code_from_line(&line).unwrap(), want, "{text} on stdin");
    }
    assert_eq!(code_from_line(&format!("{}\n", code())).unwrap(), want);
    assert!(code_from_line(&code().replacen("RH1-", "DRP1-", 1)).is_err());
    assert!(code_from_line("").is_err());
    let flood = "x".repeat(10 * CODE_LINE_LIMIT as usize);
    assert_eq!(read_capped_line(flood.as_bytes()).unwrap().len(), CODE_LINE_LIMIT as usize);
    assert!(parse(&args(&["enroll", &code(), "--hub=ws://127.0.0.1:9000/v1/agent"])).is_ok());
    assert!(matches!(parse(&args(&["status"])).unwrap().command, AgentCommand::Status));
    assert!(matches!(parse(&args(&["unenroll"])).unwrap().command, AgentCommand::Unenroll));
    for (bad, expected) in [
        (&[][..], clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
        (&["nonsense"], clap::error::ErrorKind::InvalidSubcommand),
        (&["status", "extra"], clap::error::ErrorKind::UnknownArgument),
        (&["enroll", "-", "--hub", "wss://hub.example", "extra"], clap::error::ErrorKind::UnknownArgument),
    ] {
        assert_eq!(parse(&args(bad)).unwrap_err().kind(), expected, "{bad:?}");
    }
    assert_eq!(
        parse(&args(&["enroll", "nonsense", "--hub", "wss://hub.example"])).expect("parses").checked().unwrap_err(),
        "nonsense".parse::<EnrollCode>().unwrap_err().to_string()
    );
    assert_eq!(parse(&args(&["--help"])).unwrap_err().kind(), clap::error::ErrorKind::DisplayHelp);
    // The refusals that come after parsing, each with its own message.
    let only_enroll = "--hub is only for enroll: the other subcommands use the hub this instance enrolled with";
    let dr2 = code().replacen("RH1-", "DR2-", 1);
    let code_refusal = |text: &str| text.parse::<EnrollCode>().unwrap_err().to_string();
    for (bad, expected) in [
        (args(&["unenroll", "--hub", "wss://hub.example"]), only_enroll.to_string()),
        (args(&["run", "--hub", "wss://hub.example"]), only_enroll.to_string()),
        (args(&["enroll", "RH1-nope", "--hub", "wss://hub.example"]), code_refusal("RH1-nope")),
        (args(&["enroll", "DR1-nope", "--hub", "wss://hub.example"]), code_refusal("DR1-nope")),
        (args(&["enroll", &dr2, "--hub", "wss://hub.example"]), code_refusal(&dr2)),
        (args(&["enroll", &code()]), "enroll needs the hub: --hub wss://<hub>".to_string()),
        (args(&["enroll", &code(), "--hub", "ws://hub.example"]), hub::origin("ws://hub.example").unwrap_err()),
    ] {
        assert_eq!(parse(&bad).expect("parses").checked().unwrap_err(), expected, "{bad:?}");
    }
    for (bad, expected) in [
        (&["enroll"][..], clap::error::ErrorKind::MissingRequiredArgument),
        (&["run", "extra"], clap::error::ErrorKind::UnknownArgument),
        (&["serve"], clap::error::ErrorKind::InvalidSubcommand),
    ] {
        assert_eq!(parse(&args(bad)).unwrap_err().kind(), expected, "{bad:?}");
    }
    assert_eq!(
        parse(&args(&["enroll", "-", "--hub", "wss://hub.example"])).unwrap().command,
        AgentCommand::Enroll { code: "-".into() }
    );
}

/// The command line's unenroll leaves neither `cloud.json` (the owner)
/// nor the aggregator's key behind.
#[test]
fn unenroll_on_the_command_line_leaves_nothing_of_droppen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
    load_or_create(&paths).unwrap();
    state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5)
        .save(&AgentFiles::of(&paths))
        .unwrap();
    // A pending key an older release left behind.
    std::fs::create_dir_all(droppen_aggregator::dir(&paths)).unwrap();
    std::fs::write(droppen_aggregator::key_path(&paths), b"{}").unwrap();
    let cloud = crate::cloud::LinkFile::of(&paths).path();
    std::fs::write(&cloud, br#"{"owner":"alice"}"#).unwrap();
    let before = unenroll_instance(&paths).unwrap();
    assert_eq!(before.map(|st| st.machine).as_deref(), Some("m-1"));
    assert!(!paths.instance_dir().join("agent").exists());
    assert!(!droppen_aggregator::dir(&paths).exists(), "the aggregator's key is gone");
    assert!(!cloud.exists(), "cloud.json is gone");
}
