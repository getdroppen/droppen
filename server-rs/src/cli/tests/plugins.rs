use clap::error::ErrorKind;

use super::super::Command;
use super::parse_words;
use droppen_plugins::cli::{PluginArgs, PluginCommand};

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn parse(words: &[String]) -> Result<PluginArgs, clap::Error> {
    match parse_words("plugin", words)? {
        Command::Plugin(parsed) => Ok(parsed),
        other => panic!("{words:?} parsed as {other:?}"),
    }
}

/// The refusal `run` gives `purge` without `--yes`.
const PURGE_WITHOUT_YES: &str = "purge moves the plugin's data aside; say --yes to mean it";

/// Parsing, and the check the command applies before it runs.
fn checked(words: &[String]) -> Result<PluginCommand, String> {
    let parsed = parse(words).map_err(|error| error.to_string())?;
    match parsed.command {
        PluginCommand::Purge { yes: 0, .. } => Err(PURGE_WITHOUT_YES.to_string()),
        command => Ok(command),
    }
}

fn action(line: &str) -> Result<PluginCommand, String> {
    checked(&args(line))
}

/// The clap refusal of a command line.
fn refusal(line: &[&str]) -> ErrorKind {
    let words: Vec<String> = line.iter().map(|word| (*word).to_string()).collect();
    parse(&words).unwrap_err().kind()
}

#[test]
fn the_subcommands_parse_and_anything_else_is_the_usage() {
    assert_eq!(action("list"), Ok(PluginCommand::List));
    assert_eq!(action("install /tmp/tasks.tgz"), Ok(PluginCommand::Install { file: "/tmp/tasks.tgz".into() }));
    assert_eq!(action("uninstall tasks"), Ok(PluginCommand::Uninstall { id: "tasks".into() }));
    assert_eq!(action("enable tasks"), Ok(PluginCommand::Enable { id: "tasks".into() }));
    assert_eq!(
        action("export tasks --out /tmp/x.json"),
        Ok(PluginCommand::Export {
            id: "tasks".into(),
            out: Some("/tmp/x.json".into())
        })
    );
    assert_eq!(
        action("import tasks backup.json"),
        Ok(PluginCommand::Import {
            id: "tasks".into(),
            file: "backup.json".into(),
            replace: 0
        })
    );
    assert_eq!(
        action("import tasks backup.json --replace"),
        Ok(PluginCommand::Import {
            id: "tasks".into(),
            file: "backup.json".into(),
            replace: 1
        })
    );
    assert_eq!(action("purge tasks --yes"), Ok(PluginCommand::Purge { id: "tasks".into(), yes: 1 }));
    assert_eq!(action("purge tasks").unwrap_err(), PURGE_WITHOUT_YES);
    assert_eq!(
        parse(&args("enable ../x")).unwrap_err().kind(),
        ErrorKind::ValueValidation,
        "{}",
        parse(&args("enable ../x")).unwrap_err()
    );
    assert!(parse(&args("enable ../x")).unwrap_err().to_string().contains("is not a plugin id"));
    for (bad, expected) in [
        ("", ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
        ("list extra", ErrorKind::UnknownArgument),
        ("install", ErrorKind::MissingRequiredArgument),
        ("uninstall", ErrorKind::MissingRequiredArgument),
        ("enable", ErrorKind::MissingRequiredArgument),
        ("import tasks", ErrorKind::MissingRequiredArgument),
        ("import tasks a b", ErrorKind::UnknownArgument),
        ("export tasks --out", ErrorKind::InvalidValue),
    ] {
        assert_eq!(parse(&args(bad)).unwrap_err().kind(), expected, "{bad:?}");
    }
}

/// clap parses the subcommands and options; `--out`, `--replace` and
/// `--yes` may be spelled `--name=value` and written in any order, and
/// repeated `--replace`/`--yes` count.
#[test]
fn the_plugin_argument_table() {
    let words = |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_string()).collect() };
    let accepted: Vec<(&[&str], PluginCommand)> = vec![
        (&["list"][..], PluginCommand::List),
        (&["list", "--instance", "--hub"], PluginCommand::List),
        (&["install", "/tmp/tasks.tgz"], PluginCommand::Install { file: "/tmp/tasks.tgz".into() }),
        (&["install", "-x"], PluginCommand::Install { file: "-x".into() }),
        (&["install", "-"], PluginCommand::Install { file: "-".into() }),
        // A file that looks like a flag is a file now.
        (&["install", "--force"], PluginCommand::Install { file: "--force".into() }),
        (&["install", "--", "file"], PluginCommand::Install { file: "file".into() }),
        (&["uninstall", "tasks"], PluginCommand::Uninstall { id: "tasks".into() }),
        (&["enable", "tasks"], PluginCommand::Enable { id: "tasks".into() }),
        (&["disable", "tasks"], PluginCommand::Disable { id: "tasks".into() }),
        (&["verify", "tasks"], PluginCommand::Verify { id: "tasks".into() }),
        (&["export", "tasks"], PluginCommand::Export { id: "tasks".into(), out: None }),
        (
            &["export", "tasks", "--out", "/tmp/x.json"],
            PluginCommand::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
        ),
        (
            &["export", "--out", "/tmp/x.json", "tasks"],
            PluginCommand::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
        ),
        (
            &["export", "tasks", "--out=/tmp/x.json"],
            PluginCommand::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
        ),
        (&["export", "tasks", "--out", "--"], PluginCommand::Export { id: "tasks".into(), out: Some("--".into()) }),
        (&["export", "tasks", "--out", "--out"], PluginCommand::Export { id: "tasks".into(), out: Some("--out".into()) }),
        (
            &["import", "tasks", "backup.json"],
            PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 0 },
        ),
        (
            &["import", "tasks", "backup.json", "--replace"],
            PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 1 },
        ),
        (
            &["import", "tasks", "--replace", "backup.json"],
            PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 1 },
        ),
        (
            &["import", "--replace", "tasks", "backup.json"],
            PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 1 },
        ),
        (
            &["import", "tasks", "-x"],
            PluginCommand::Import { id: "tasks".into(), file: "-x".into(), replace: 0 },
        ),
        (
            &["import", "tasks", "--force"],
            PluginCommand::Import { id: "tasks".into(), file: "--force".into(), replace: 0 },
        ),
        (&["import", "--", "tasks", "f"], PluginCommand::Import { id: "tasks".into(), file: "f".into(), replace: 0 }),
        (&["purge", "tasks", "--yes"], PluginCommand::Purge { id: "tasks".into(), yes: 1 }),
        (&["purge", "--yes", "tasks"], PluginCommand::Purge { id: "tasks".into(), yes: 1 }),
        (&["purge", "tasks", "--yes", "--yes"], PluginCommand::Purge { id: "tasks".into(), yes: 2 }),
        (&["uninstall", "--", "tasks"], PluginCommand::Uninstall { id: "tasks".into() }),
        (&["export", "--", "tasks"], PluginCommand::Export { id: "tasks".into(), out: None }),
    ];
    for (line, expected) in accepted {
        assert_eq!(checked(&words(line)), Ok(expected), "{line:?}");
    }

    for (line, expected) in [
        (&[][..], ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
        (&["list", "extra"], ErrorKind::UnknownArgument),
        (&["install"], ErrorKind::MissingRequiredArgument),
        (&["uninstall"], ErrorKind::MissingRequiredArgument),
        (&["uninstall", "../x"], ErrorKind::ValueValidation),
        (&["enable"], ErrorKind::MissingRequiredArgument),
        (&["disable"], ErrorKind::MissingRequiredArgument),
        (&["verify"], ErrorKind::MissingRequiredArgument),
        (&["import", "tasks"], ErrorKind::MissingRequiredArgument),
        (&["import", "tasks", "a", "b"], ErrorKind::UnknownArgument),
        (&["import", "tasks", "--replace"], ErrorKind::MissingRequiredArgument),
        (&["export", "tasks", "--out"], ErrorKind::InvalidValue),
        (&["export", "tasks", "f"], ErrorKind::UnknownArgument),
        (&["purge", "tasks", "--yes=x"], ErrorKind::TooManyValues),
        (&["--", "list"], ErrorKind::UnknownArgument),
        (&["export", "tasks", "--", "--out", "f"], ErrorKind::UnknownArgument),
    ] {
        assert_eq!(refusal(line), expected, "{line:?}");
    }
    assert_eq!(checked(&words(&["purge", "tasks"])).unwrap_err(), PURGE_WITHOUT_YES);
}

#[test]
fn help_is_claps() {
    for line in ["--help", "-h", "list --help", "export --help", "install --help", "export tasks --help"] {
        assert_eq!(parse(&args(line)).unwrap_err().kind(), ErrorKind::DisplayHelp, "{line:?}");
    }
}

#[test]
fn every_instance_is_validated_and_the_last_is_used() {
    let parsed = parse(&args("list --instance bob --instance carol")).expect("parsed");
    assert_eq!(parsed.instance.last().map(String::as_str), Some("carol"));
    for (line, expected) in [
        ("list --instance Bad --instance probe", ErrorKind::ValueValidation),
        ("list --instance=Bad --instance=probe", ErrorKind::ValueValidation),
        ("list --instance=", ErrorKind::ValueValidation),
        ("list --instance", ErrorKind::InvalidValue),
        ("list --instance Staging", ErrorKind::ValueValidation),
    ] {
        assert_eq!(parse(&args(line)).unwrap_err().kind(), expected, "{line:?}");
    }
}
