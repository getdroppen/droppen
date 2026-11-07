use clap::error::ErrorKind;

use super::super::Command;
use super::{args, parse_words};
use droppen_notify::notify::cli::{NotifyArgs, Options, USAGE};
use droppen_notify::testing::NotifyArgsTesting;

fn parse(raw: &[String]) -> Result<NotifyArgs, clap::Error> {
    match parse_words("notify", raw)? {
        Command::Notify(parsed) => Ok(parsed),
        other => panic!("{raw:?} parsed as {other:?}"),
    }
}

fn no_title() -> String {
    format!("a notification needs a title\n\n{USAGE}")
}

/// Parsing and the title check the command applies before it sends.
fn options(raw: &[String]) -> Result<Options, String> {
    parse(raw).map_err(|error| error.to_string())?.options()
}

#[test]
fn the_shape_a_model_writes_first_is_the_one_that_works() {
    let parsed = options(&args(&["Blocked on a decision", "Postgres or SQLite?"])).expect("parsed");
    assert_eq!(parsed.title, "Blocked on a decision");
    assert_eq!(parsed.body, "Postgres or SQLite?");
    assert_eq!(parsed.level, None);
    assert_eq!(parsed.group, None);

    // Everything after the title is the body, so an unquoted sentence still
    // arrives whole rather than as a title and four ignored words.
    let parsed = options(&args(&["Done", "all", "41", "tests", "pass"])).expect("parsed");
    assert_eq!(parsed.body, "all 41 tests pass");
}

#[test]
fn the_flags_are_there_for_the_two_things_with_no_natural_position() {
    let parsed = options(&args(&["Blocked", "Need a decision", "--level", "time-sensitive", "--group", "deploy"])).expect("parsed");
    assert_eq!(parsed.level.as_deref(), Some("time-sensitive"));
    assert_eq!(parsed.group.as_deref(), Some("deploy"));

    let parsed = options(&args(&["--title", "T", "--body", "B"])).expect("parsed");
    assert_eq!((parsed.title.as_str(), parsed.body.as_str()), ("T", "B"));

    // A title that starts with a dash is a title.
    let parsed = options(&args(&["--", "--not-a-flag", "body"])).expect("parsed");
    assert_eq!(parsed.title, "--not-a-flag");
    assert_eq!(parsed.body, "body");
}

#[test]
fn a_call_that_cannot_work_says_so_before_the_network() {
    assert_eq!(options(&args(&[])).expect_err("no title"), no_title());
    assert_eq!(options(&args(&["  "])).expect_err("blank title"), no_title());
    assert_eq!(parse(&args(&["T", "--level"])).expect_err("dangling").kind(), ErrorKind::InvalidValue);
    assert_eq!(parse(&args(&["T", "--nope"])).expect_err("unknown").kind(), ErrorKind::UnknownArgument);
}

/// The spellings clap now takes (`--level=active`, `-lactive`) are accepted;
/// a bare `--` turns the rest into text; help is clap's.
#[test]
fn the_notify_argument_table() {
    let accepted: Vec<Vec<String>> = [
        &["T"][..],
        &["T", "B"],
        &["T", "B", "C"],
        &["--title", "T", "--body", "B"],
        &["T", "B", "--level", "active"],
        &["T", "--level", "passive", "--group", "deploy"],
        &["T", "--", "--not-a-flag"],
        &["--", "--not-a-flag", "body"],
        &["-"],
        &["T", "-"],
        &["T", "--level", "--help"],
        &["T", "--group", "--level=x"],
        &["--title", "-h"],
        &["T", "--title", "U"],
        &["--level", "active", "T", "B"],
        &["T", "--level=active"],
        &["T", "-lactive"],
        &["T", "-gdeploy"],
        &["T", "-tU"],
        &["T", "-bB"],
        &["T", "--title=U"],
    ]
    .into_iter()
    .map(args)
    .collect();
    for argv in &accepted {
        assert!(options(argv).is_ok(), "{argv:?} must parse: {:?}", options(argv).err());
    }
    assert_eq!(options(&args(&["T", "--level=active"])).expect("parsed").level.as_deref(), Some("active"));

    for argv in [&[][..], &["  "]] {
        assert_eq!(options(&args(argv)).unwrap_err(), no_title(), "{argv:?}");
    }
    for (argv, expected) in [
        (&["T", "--level"][..], ErrorKind::InvalidValue),
        (&["T", "--nope"], ErrorKind::UnknownArgument),
        (&["-x"], ErrorKind::UnknownArgument),
    ] {
        assert_eq!(parse(&args(argv)).unwrap_err().kind(), expected, "{argv:?}");
    }
}

/// A call that gets it wrong is told how to call it, under the name the
/// session knows the command by.
#[test]
fn the_usage_names_the_command() {
    assert!(options(&args(&[])).expect_err("no title").contains("usage: droppen-notify <title>"));
    for argv in [&["--help"][..], &["T", "--nope"]] {
        let error = parse(&args(argv)).unwrap_err();
        assert!(error.to_string().contains("Usage: droppen-notify <title>"), "{argv:?}: {error}");
    }
}

#[test]
fn help_is_claps() {
    for argv in [vec!["--help"], vec!["-h"], vec!["T", "--help"], vec!["T", "-h"], vec!["--help", "junk"]] {
        assert_eq!(parse(&args(&argv)).unwrap_err().kind(), ErrorKind::DisplayHelp, "{argv:?}");
    }
}
