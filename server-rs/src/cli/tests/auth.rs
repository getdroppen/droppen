use clap::error::ErrorKind;

use super::super::Command;
use super::parse_words;
use droppen_auth::cli::{OpenArgs, SetupCodeArgs, UserArgs, UserCommand};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

fn parse_user(words: &[String]) -> Result<UserArgs, clap::Error> {
    match parse_words("user", words)? {
        Command::User(parsed) => Ok(parsed),
        other => panic!("{words:?} parsed as {other:?}"),
    }
}

/// The refusal `run` gives a flag the action has no meaning for.
const FLAG_MISUSE: &str = "usage: remote-hand-server user add|list|passwd|rm|mark-app [--app] [--yes] [--instance <instance>]";

/// Parsing, and the flag check the command applies before it runs.
fn checked(words: &[String]) -> Result<UserArgs, String> {
    let parsed = parse_user(words).map_err(|error| error.to_string())?;
    if parsed.flags_fit() {
        Ok(parsed)
    } else {
        Err(FLAG_MISUSE.to_string())
    }
}

fn parse_setup(words: &[String]) -> Result<SetupCodeArgs, clap::Error> {
    match parse_words("setup-code", words)? {
        Command::SetupCode(parsed) => Ok(parsed),
        other => panic!("{words:?} parsed as {other:?}"),
    }
}

fn parse_open(words: &[String]) -> Result<OpenArgs, clap::Error> {
    match parse_words("open", words)? {
        Command::Open(parsed) => Ok(parsed),
        other => panic!("{words:?} parsed as {other:?}"),
    }
}

/// The `user` command lines: names may start with a hyphen, `--` makes even
/// a flag-shaped word a name, and each action reads only its own flags.
#[test]
fn the_user_argument_table() {
    let accepted: Vec<Vec<String>> = [
        &["add", "ada"][..],
        &["add", "--app", "ada"],
        &["add", "ada", "--app"],
        &["add", "ada", "--app", "--app"],
        &["list"],
        &["list", "--app"],
        &["list", "--app", "--app"],
        &["passwd", "ada"],
        &["rm", "ada"],
        &["rm", "ada", "--yes"],
        &["rm", "--yes", "ada"],
        &["rm", "ada", "--yes", "--yes"],
        &["mark-app", "ada"],
        &["add", "ada", "--instance", "bob"],
        &["--instance=bob", "list"],
        &["--instance", "bob", "--instance", "carol", "list"],
        &["add", "--", "--help"],
        &["add", "--", "ada"],
        &["passwd", "--", "-x"],
        &["mark-app", "-x"],
        &["add", "-x"],
        &["add", "--anything"],
        &["add", "--instance", "bob", "ada"],
        &["add", "ada", "--instance=bob", "--app"],
        &["list", "--instance", "--x"],
        &["--instance", "--", "list"],
    ]
    .into_iter()
    .map(args)
    .collect();
    for argv in &accepted {
        assert!(checked(argv).is_ok(), "{argv:?} must be accepted: {:?}", checked(argv).err());
    }

    // clap's refusals.
    for (argv, expected) in [
        (&[][..], ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
        (&["add"], ErrorKind::MissingRequiredArgument),
        (&["add", "ada", "extra"], ErrorKind::UnknownArgument),
        (&["--instance"], ErrorKind::InvalidValue),
        (&["--instance", "Bad Name", "list"], ErrorKind::ValueValidation),
        (&["bogus"], ErrorKind::InvalidSubcommand),
        (&["add", "ada", "--instance"], ErrorKind::InvalidValue),
        (&["add", "--"], ErrorKind::MissingRequiredArgument),
        (&["--", "list"], ErrorKind::UnknownArgument),
        // clap keeps `--` as its terminator, so a name of exactly `--`
        // cannot be given in this form.
        (&["add", "--", "--app", "--instance", "probe"], ErrorKind::UnknownArgument),
    ] {
        assert_eq!(parse_user(&args(argv)).unwrap_err().kind(), expected, "{argv:?}");
    }

    // The flags an action has no meaning for, refused before any file is read.
    for argv in [
        &["add", "ada", "--yes"][..],
        &["list", "--yes"],
        &["passwd", "ada", "--app"],
        &["passwd", "ada", "--yes"],
        &["rm", "ada", "--app"],
        &["mark-app", "ada", "--app"],
        &["mark-app", "ada", "--yes"],
        &["list", "--app", "--yes"],
    ] {
        assert_eq!(checked(&args(argv)).unwrap_err(), FLAG_MISUSE, "{argv:?}");
    }
}

#[test]
fn the_subcommand_is_read_alongside_the_flag() {
    let parsed = checked(&args(&["rm", "ada", "--instance", "bob", "--yes"])).expect("parse");
    assert_eq!(parsed.command, UserCommand::Rm { name: "ada".into() });
    assert_eq!(parsed.yes, 1);
    assert_eq!(parsed.instance.last().map(String::as_str), Some("bob"));

    assert_eq!(
        parse_user(&args(&["add", "ada", "extra"])).unwrap_err().kind(),
        ErrorKind::UnknownArgument,
        "a stray word is refused"
    );
    assert_eq!(parse_user(&args(&["list", "ada"])).unwrap_err().kind(), ErrorKind::UnknownArgument);
}

#[test]
fn app_internal_accounts_are_asked_for_with_a_flag() {
    let action = |words: &[&str]| checked(&args(words)).map(|parsed| (parsed.command, parsed.app > 0, parsed.yes > 0));
    let add = || UserCommand::Add { name: "ada".into() };
    assert_eq!(action(&["add", "ada"]), Ok((add(), false, false)));
    assert_eq!(action(&["add", "--app", "ada"]), Ok((add(), true, false)));
    assert_eq!(action(&["add", "ada", "--app"]), Ok((add(), true, false)));
    assert_eq!(action(&["list", "--app"]), Ok((UserCommand::List, true, false)));
    assert_eq!(action(&["list"]), Ok((UserCommand::List, false, false)));
    assert_eq!(action(&["mark-app", "ada"]), Ok((UserCommand::MarkApp { name: "ada".into() }, false, false)));
    assert_eq!(action(&["rm", "ada", "--app"]), Err(FLAG_MISUSE.to_string()), "the flag means nothing to rm");
    assert_eq!(action(&["rm", "--yes", "ada"]), Ok((UserCommand::Rm { name: "ada".into() }, false, true)));
    assert_eq!(action(&["add", "ada", "--yes"]), Err(FLAG_MISUSE.to_string()), "--yes is rm's alone");
    assert_eq!(parse_user(&args(&["mark-app"])).unwrap_err().kind(), ErrorKind::MissingRequiredArgument);
}

/// What each command line means: the action, the flags it reads, and the
/// instance, wherever they are written.
#[test]
fn the_user_action_and_instance_are_read_from_the_same_table() {
    let user = |instance: Option<&str>, app: u8, yes: u8, command: UserCommand| UserArgs {
        instance: instance.map(str::to_string).into_iter().collect(),
        app,
        yes,
        command,
    };
    let named = |name: &str| name.to_string();
    for (argv, expected) in [
        (&["add", "ada", "--app", "--instance", "bob"][..], user(Some("bob"), 1, 0, UserCommand::Add { name: named("ada") })),
        (&["rm", "ada", "--instance", "bob"], user(Some("bob"), 0, 0, UserCommand::Rm { name: named("ada") })),
        (&["add", "--help"], user(None, 0, 0, UserCommand::Add { name: named("--help") })),
        (&["rm", "--anything"], user(None, 0, 0, UserCommand::Rm { name: named("--anything") })),
        // After clap's `--` every word is a name, flags included.
        (&["add", "--", "--app"], user(None, 0, 0, UserCommand::Add { name: named("--app") })),
        (&["rm", "--", "--yes"], user(None, 0, 0, UserCommand::Rm { name: named("--yes") })),
        (&["rm", "--yes", "--", "--instance=bob"], user(None, 0, 1, UserCommand::Rm { name: named("--instance=bob") })),
    ] {
        assert_eq!(parse_user(&args(argv)).expect("parsed"), expected, "{argv:?}");
    }
    for (argv, expected) in [
        (&["mark-app", "--"][..], ErrorKind::MissingRequiredArgument),
        (&["passwd", "--"], ErrorKind::MissingRequiredArgument),
        (&["--instance", "Bad", "--instance", "probe", "list"], ErrorKind::ValueValidation),
        (&["add", "ada", "--instance", "Bad", "--instance", "probe"], ErrorKind::ValueValidation),
        (&["list", "--instance", "", "--instance", "probe"], ErrorKind::ValueValidation),
        (&["list", "--instance", "a b", "--instance", "probe"], ErrorKind::ValueValidation),
        (&["list", "--instance=", "--instance", "probe"], ErrorKind::ValueValidation),
    ] {
        assert_eq!(parse_user(&args(argv)).unwrap_err().kind(), expected, "{argv:?}");
    }
}

/// The names the desktop app and the installer may pass (their regex allows
/// any hyphen-led word) are names, not help flags: the name leaves have
/// clap's help disabled.
#[test]
fn a_name_in_the_name_position_is_always_a_name() {
    for want in ["--help", "-h", "-hh", "--version", "-V", "--x", "-x", "-"] {
        let add = parse_user(&args(&["add", want, "--app", "--instance", "probe"])).expect(want);
        assert!(matches!(add.command, UserCommand::Add { ref name } if name.as_str() == want), "{want}");
        assert!(add.app > 0);
        assert_eq!(add.instance.last().map(String::as_str), Some("probe"));

        let passwd = parse_user(&args(&["passwd", want, "--instance", "probe"])).expect(want);
        assert!(matches!(passwd.command, UserCommand::Passwd { ref name } if name.as_str() == want), "{want}");

        let mark = parse_user(&args(&["mark-app", want, "--instance", "probe"])).expect(want);
        assert!(matches!(mark.command, UserCommand::MarkApp { ref name } if name.as_str() == want), "{want}");

        let rm = checked(&args(&["rm", want, "--yes", "--instance", "probe"])).expect(want);
        assert_eq!(rm.command, UserCommand::Rm { name: want.to_string() });
        assert_eq!(rm.yes, 1);

        // The installer's form, without --app.
        let plain = checked(&args(&["add", want, "--instance", "probe"])).expect(want);
        assert_eq!(plain.command, UserCommand::Add { name: want.to_string() });
        assert_eq!(plain.app, 0);
    }
    // `--` as the name is not expressible: clap ends the options there.
    assert_eq!(
        parse_user(&args(&["add", "--", "--instance", "probe"])).unwrap_err().kind(),
        ErrorKind::UnknownArgument
    );
}

#[test]
fn help_is_claps() {
    for argv in [vec!["--help"], vec!["-h"], vec!["--help", "junk"], vec!["list", "--help"]] {
        assert_eq!(parse_user(&args(&argv)).unwrap_err().kind(), ErrorKind::DisplayHelp, "{argv:?}");
    }
}

/// Every `--instance` is validated (an invalid earlier one is refused even
/// when a good one follows) and the last one is used; a value may start
/// with a hyphen, because instance names may.
#[test]
fn every_instance_is_validated_and_the_last_is_used() {
    for (argv, expected) in [
        (args(&["add", "ada", "--instance", "bob"]), "bob"),
        (args(&["--instance=bob", "passwd", "ada"]), "bob"),
        (args(&["list", "--instance", "bob"]), "bob"),
        (args(&["--instance", "bob", "list"]), "bob"),
        (args(&["list", "--instance", "bob", "--instance", "carol"]), "carol"),
        (args(&["list", "--instance", "--x"]), "--x"),
    ] {
        assert_eq!(parse_user(&argv).expect("parsed").instance.last().map(String::as_str), Some(expected), "{argv:?}");
    }
    assert!(parse_user(&args(&["list"])).expect("parsed").instance.is_empty());

    for (argv, expected) in [
        (args(&["add", "ada", "--instance", "Bad", "--instance", "bob"]), ErrorKind::ValueValidation),
        (args(&["--instance", "Bad", "--instance", "bob", "list"]), ErrorKind::ValueValidation),
        (args(&["list", "--instance=Bad", "--instance=bob"]), ErrorKind::ValueValidation),
        (args(&["list", "--instance=", "--instance", "bob"]), ErrorKind::ValueValidation),
        (args(&["list", "--instance", "", "--instance", "bob"]), ErrorKind::ValueValidation),
        (args(&["list", "--instance", "a b", "--instance", "bob"]), ErrorKind::ValueValidation),
        (args(&["list", "--instance"]), ErrorKind::InvalidValue),
        (args(&["list", "--instance", "../etc"]), ErrorKind::ValueValidation),
        (args(&["list", "--instance="]), ErrorKind::ValueValidation),
        (args(&["list", "--instance", "Staging"]), ErrorKind::ValueValidation),
    ] {
        assert_eq!(parse_user(&argv).unwrap_err().kind(), expected, "{argv:?}");
    }
}

#[test]
fn open_and_setup_code_parse_their_words() {
    assert_eq!(parse_setup(&args(&[])).expect("parsed").instance.last(), None);
    assert_eq!(parse_setup(&args(&["--instance", "bob"])).expect("parsed").instance.last().map(String::as_str), Some("bob"));
    assert_eq!(parse_setup(&args(&["--instance", "--hub"])).expect("parsed").instance.last().map(String::as_str), Some("--hub"));
    assert_eq!(parse_setup(&args(&["--"])).expect("a bare -- is clap's terminator").instance, Vec::<String>::new());
    for (argv, expected) in [
        (&["junk"][..], ErrorKind::UnknownArgument),
        (&["--", "x"], ErrorKind::UnknownArgument),
        (&["--instance"], ErrorKind::InvalidValue),
        (&["--instance", "Bad Name"], ErrorKind::ValueValidation),
        (&["--instance", "Bad", "--instance", "probe"], ErrorKind::ValueValidation),
        (&["--instance=", "--instance", "probe"], ErrorKind::ValueValidation),
    ] {
        assert_eq!(parse_setup(&args(argv)).unwrap_err().kind(), expected, "{argv:?}");
    }
    for argv in [&["--help"][..], &["-h"]] {
        assert_eq!(parse_setup(&args(argv)).unwrap_err().kind(), ErrorKind::DisplayHelp, "{argv:?}");
    }

    assert_eq!(parse_open(&args(&[])).expect("parsed").print, 0);
    assert_eq!(parse_open(&args(&["--print"])).expect("parsed").print, 1);
    let open = parse_open(&args(&["--instance=bob", "--print"])).expect("parsed");
    assert_eq!(open.instance.last().map(String::as_str), Some("bob"));
    assert_eq!(open.print, 1);
    let open = parse_open(&args(&["--print", "--instance", "bob"])).expect("parsed");
    assert_eq!((open.print, open.instance.last().map(String::as_str)), (1, Some("bob")));
    assert_eq!(parse_open(&args(&["--instance", "--"])).expect("parsed").instance.last().map(String::as_str), Some("--"));
    assert_eq!(parse_open(&args(&["--"])).expect("a bare -- is clap's terminator").print, 0);
    assert_eq!(parse_open(&args(&["--print", "--"])).expect("parsed").print, 1);
    let twice = parse_open(&args(&["--print", "--print"])).expect("parsed");
    assert_eq!(twice.print, 2, "the run refuses two --print words");
    for (argv, expected) in [
        (&["nonsense"][..], ErrorKind::UnknownArgument),
        (&["--print=x"], ErrorKind::TooManyValues),
        (&["--", "--print"], ErrorKind::UnknownArgument),
        (&["--instance"], ErrorKind::InvalidValue),
        (&["--instance", "Bad", "--instance", "probe"], ErrorKind::ValueValidation),
        (&["--instance=", "--instance", "probe"], ErrorKind::ValueValidation),
        (&["--instance", "a b", "--instance", "probe", "--print"], ErrorKind::ValueValidation),
    ] {
        assert_eq!(parse_open(&args(argv)).unwrap_err().kind(), expected, "{argv:?}");
    }
    for argv in [&["--help"][..], &["-h"], &["--help", "junk"]] {
        assert_eq!(parse_open(&args(argv)).unwrap_err().kind(), ErrorKind::DisplayHelp, "{argv:?}");
    }
}

/// `open` takes `--print` once; the second is refused before anything is
/// read or opened.
#[tokio::test]
async fn open_takes_its_flag_once() {
    let twice = parse_open(&args(&["--print", "--print"])).expect("parsed");
    assert_eq!(
        twice.run().await.unwrap_err(),
        "usage: remote-hand-server open [--print] [--instance <instance>]"
    );
}
