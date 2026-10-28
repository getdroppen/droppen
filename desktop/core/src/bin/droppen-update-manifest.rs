//! Write or check the updater manifest, `latest.json`, of a Droppen release.
//! `desktop/updater-artifacts.sh` runs it; see `desktop/SIGNING.md`.
//!
//! ```text
//! droppen-update-manifest write --version <v> --archive <Droppen.app.tar.gz> --signature <.sig>
//!                               --pubkey <file> --out <latest.json> [--url <url>] [--notes <url>] [--loopback-http]
//! droppen-update-manifest check <latest.json> --pubkey <file> [--archive <Droppen.app.tar.gz>] [--loopback-http]
//! droppen-update-manifest key-id --pubkey <file>
//! ```
//!
//! `--pubkey` is the `.pub` file `tauri signer generate` wrote, or a
//! `tauri.conf.json` (its `plugins.updater.pubkey`). `write` verifies the
//! signature against the archive and the key, and refuses to write a manifest
//! the app would not accept; `check` reads one back the same way (without
//! `--archive`, the signature's form and fields only). `--url` defaults to the
//! archive on the release `v<version>` of getdroppen/droppen, `--notes` to
//! that release's page. `--loopback-http` lets `--url` be
//! `http://127.0.0.1:<port>/…`: the updater's end-to-end test, never a release.
//! `key-id` prints the key's minisign key id, as `tauri signer` shows it.
//!
//! The grammar is clap's: unknown options and positional words a command does
//! not take are refused rather than ignored.

use std::path::Path;
use std::process::ExitCode;
use std::time::SystemTime;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use droppen_desktop_core::update::{archive_url, key_id, release_page, Manifest, Rules};

const USAGE: &str = "usage:
  droppen-update-manifest write --version <v> --archive <Droppen.app.tar.gz> --signature <.sig> --pubkey <file> --out <latest.json> [--url <url>] [--notes <url>] [--loopback-http]
  droppen-update-manifest check <latest.json> --pubkey <file> [--archive <Droppen.app.tar.gz>] [--loopback-http]
  droppen-update-manifest key-id --pubkey <file>";

fn main() -> ExitCode {
    match parse(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(Parsed::Help(text)) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Ok(Parsed::Run { command, options }) => match run(command, options) {
            Ok(message) => {
                println!("{message}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("droppen-update-manifest: {message}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("droppen-update-manifest: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command, options: Options) -> Result<String, String> {
    let required = |name: &str, value: Option<String>| value.ok_or_else(|| format!("--{name} is required\n{USAGE}"));
    let pubkey = read_pubkey(Path::new(&required("pubkey", options.pubkey)?))?;
    let key = key_id(&pubkey)?;
    let archive = options
        .archive
        .map(|path| std::fs::read(&path).map_err(|error| format!("{path}: {error}")))
        .transpose()?;
    let rules = Rules {
        pubkey: &pubkey,
        archive: archive.as_deref(),
        loopback_http: options.loopback_http > 0,
    };
    match command {
        Command::Write => {
            let version = required("version", options.version)?;
            if archive.is_none() {
                return Err("--archive is required: write verifies the signature before it writes".to_string());
            }
            let signature_path = required("signature", options.signature)?;
            let signature = std::fs::read_to_string(&signature_path).map_err(|error| format!("{signature_path}: {error}"))?;
            let url = options.url.unwrap_or_else(|| archive_url(&version));
            let notes = options.notes.unwrap_or_else(|| release_page(&version));
            let manifest = Manifest::new(&version, &notes, &url, &signature, SystemTime::now());
            refuse(manifest.check(&rules))?;
            let out = required("out", options.out)?;
            std::fs::write(&out, manifest.to_json()).map_err(|error| format!("{out}: {error}"))?;
            Ok(format!("wrote {out}: Droppen {version}, {url}, signature verified with key {key}"))
        }
        Command::KeyId => Ok(key),
        Command::Check { path } => {
            let json = std::fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
            let manifest = Manifest::parse(&json)?;
            refuse(manifest.check(&rules))?;
            Ok(format!(
                "{path}: Droppen {} is fit to publish{}",
                manifest.version,
                if archive.is_some() {
                    format!(" (signature by key {key}, verified against the archive)")
                } else {
                    format!(" (signature by key {key}, not verified: no --archive)")
                }
            ))
        }
    }
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Write,
    Check { path: String },
    KeyId,
}

/// The tool's options. Every command accepts all of them; only the command that
/// uses one reads it.
#[derive(Debug, Args)]
struct Options {
    #[arg(long, allow_hyphen_values = true)]
    version: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    archive: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    signature: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pubkey: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    out: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    url: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    notes: Option<String>,
    #[arg(long, action = clap::ArgAction::Count)]
    loopback_http: u8,
}

#[derive(Debug, Args)]
struct CheckArgs {
    /// The `latest.json` to read.
    path: String,
    #[command(flatten)]
    options: Options,
}

/// `droppen-update-manifest`'s command line: the command word, then its options.
#[derive(Debug, Parser)]
#[command(name = "droppen-update-manifest", disable_help_subcommand = true, args_override_self = true)]
struct ManifestCli {
    #[command(subcommand)]
    command: Option<ManifestCommand>,
}

#[derive(Debug, Subcommand)]
enum ManifestCommand {
    #[command(args_override_self = true)]
    Write(Options),
    #[command(args_override_self = true)]
    Check(CheckArgs),
    #[command(args_override_self = true)]
    KeyId(Options),
}

/// What the command line asked for, or clap's help text.
#[derive(Debug)]
enum Parsed {
    Help(String),
    Run { command: Command, options: Options },
}

/// Parse the command line, or say what is wrong with it. Strict parsing is
/// clap's: unknown flags are refused, `--name=value` is accepted, and a bare
/// `--` is its terminator.
fn parse(args: &[String]) -> Result<Parsed, String> {
    let matches = match ManifestCli::command().try_get_matches_from(std::iter::once("droppen-update-manifest".to_string()).chain(args.iter().cloned())) {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => return Ok(Parsed::Help(error.to_string())),
        Err(error) => return Err(error.to_string()),
    };
    let cli = ManifestCli::from_arg_matches(&matches).map_err(|error| error.to_string())?;
    match cli.command {
        Some(ManifestCommand::Write(options)) => Ok(Parsed::Run { command: Command::Write, options }),
        Some(ManifestCommand::KeyId(options)) => Ok(Parsed::Run { command: Command::KeyId, options }),
        Some(ManifestCommand::Check(check)) => Ok(Parsed::Run { command: Command::Check { path: check.path }, options: check.options }),
        None => Err(USAGE.to_string()),
    }
}

/// The public key from a `.pub` file or from `tauri.conf.json`.
fn read_pubkey(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if path.extension().is_some_and(|extension| extension == "json") {
        let conf: serde_json::Value = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        conf["plugins"]["updater"]["pubkey"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{} has no plugins.updater.pubkey", path.display()))
    } else {
        Ok(text.trim().to_string())
    }
}

fn refuse(problems: Vec<String>) -> Result<(), String> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("refused:\n  - {}", problems.join("\n  - ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    fn parsed(words: &[&str]) -> (Command, Options) {
        match parse(&args(words)) {
            Ok(Parsed::Run { command, options }) => (command, options),
            Ok(Parsed::Help(text)) => panic!("unexpected help: {text}"),
            Err(error) => panic!("{words:?} must parse: {error}"),
        }
    }

    fn help(words: &[&str]) -> String {
        match parse(&args(words)) {
            Ok(Parsed::Help(text)) => text,
            other => panic!("{words:?} must answer help: {other:?}"),
        }
    }

    /// The exact invocations the release workflow, `desktop/updater-artifacts.sh` and `desktop/SIGNING.md`
    /// run. Each must still be accepted.
    #[test]
    fn the_release_invocations_parse() {
        // desktop/SIGNING.md: verify the app's key.
        let (command, options) = parsed(&[
            "write",
            "--version",
            "0.0.1",
            "--archive",
            "Droppen.app.tar.gz",
            "--signature",
            "Droppen.app.tar.gz.sig",
            "--pubkey",
            "desktop/src-tauri/tauri.conf.json",
            "--out",
            "/tmp/check.json",
        ]);
        assert_eq!(command, Command::Write);
        assert_eq!(options.version.as_deref(), Some("0.0.1"));
        assert_eq!(options.out.as_deref(), Some("/tmp/check.json"));

        // .github/workflows/release.yml's "latest.json (droppen-update-manifest write)" step.
        let (command, options) = parsed(&[
            "write",
            "--version",
            "0.2.133",
            "--archive",
            "/tmp/a/desktop/updater/Droppen.app.tar.gz",
            "--signature",
            "/tmp/a/desktop/updater/Droppen.app.tar.gz.sig",
            "--pubkey",
            "desktop/src-tauri/tauri.conf.json",
            "--url",
            "https://github.com/getdroppen/droppen/releases/download/v0.2.133/Droppen.app.tar.gz",
            "--notes",
            "https://github.com/getdroppen/droppen/releases/tag/v0.2.133",
            "--out",
            "/tmp/a/desktop/updater/latest.json",
        ]);
        assert_eq!(command, Command::Write);
        assert_eq!(options.signature.as_deref(), Some("/tmp/a/desktop/updater/Droppen.app.tar.gz.sig"));

        // desktop/updater-artifacts.sh: one release build, one test build.
        for extra in [&[][..], &["--loopback-http"][..]] {
            let mut words = vec![
                "write",
                "--version",
                "0.2.133",
                "--archive",
                "/out/Droppen.app.tar.gz",
                "--signature",
                "/out/Droppen.app.tar.gz.sig",
                "--pubkey",
                "/keys/updater.pub",
                "--url",
                "https://getdroppen.github.io/releases/v0.2.133/Droppen.app.tar.gz",
                "--notes",
                "https://getdroppen.github.io/releases/v0.2.133",
                "--out",
                "/out/latest.json",
            ];
            words.extend_from_slice(extra);
            let (command, options) = parsed(&words);
            assert_eq!(command, Command::Write);
            assert_eq!(options.loopback_http > 0, !extra.is_empty());
        }

        // desktop/updater-artifacts.sh: key-id of the release key and of the checked key.
        let (command, options) = parsed(&["key-id", "--pubkey", "/keys/release.pub"]);
        assert_eq!(command, Command::KeyId);
        assert_eq!(options.pubkey.as_deref(), Some("/keys/release.pub"));

        // desktop/SIGNING.md: check a published latest.json, with and without the archive.
        let (command, options) = parsed(&["check", "latest.json", "--pubkey", "desktop/src-tauri/tauri.conf.json", "--archive", "Droppen.app.tar.gz"]);
        assert_eq!(command, Command::Check { path: "latest.json".into() });
        assert_eq!(options.archive.as_deref(), Some("Droppen.app.tar.gz"));
        let (command, _) = parsed(&["check", "latest.json", "--pubkey", "desktop/src-tauri/tauri.conf.json"]);
        assert_eq!(command, Command::Check { path: "latest.json".into() });
    }

    /// Strict parsing, but clap's: unknown flags, stray words and single-dash
    /// words are refused; `--name=value` and a bare `--` are clap spellings now.
    #[test]
    fn the_strict_refusals() {
        for words in [
            &["key-id", "--pubkey", "p", "--bogus", "x"][..],
            &["key-id", "junk", "--pubkey", "p"],
            &["--pubkey", "p", "key-id"],
            &["write", "junk"],
            &["check", "a", "b", "--pubkey", "p"],
            &["check", "-x", "--pubkey", "p"],
            &["key-id", "--pubkey", "p", "-x"],
            &["key-id", "--pubkey", "p", "extra"],
            &["write", "--version", "v", "--archive", "a", "--signature", "s", "--pubkey", "p", "--out", "o", "-"],
        ] {
            let error = parse(&args(words)).unwrap_err();
            assert!(error.contains("unexpected argument"), "{words:?}: {error}");
        }
        // An unknown command word, and no command at all.
        for words in [&["bogus", "--pubkey", "p"][..], &[]] {
            assert!(!parse(&args(words)).unwrap_err().is_empty(), "{words:?}");
        }
        // A missing value.
        assert!(parse(&args(&["key-id", "--pubkey"])).unwrap_err().contains("a value is required"));
    }

    /// The spellings clap takes now: `--name=value`, and `--` as its terminator.
    #[test]
    fn the_new_spellings_parse() {
        let (command, options) = parsed(&["key-id", "--pubkey=p"]);
        assert_eq!(command, Command::KeyId);
        assert_eq!(options.pubkey.as_deref(), Some("p"));

        let (command, options) = parsed(&["write", "--version=0.0.1", "--archive=a", "--signature=s", "--pubkey=p", "--out=o"]);
        assert_eq!(command, Command::Write);
        assert_eq!(options.version.as_deref(), Some("0.0.1"));

        for words in [
            &["key-id", "--pubkey", "p", "--"][..],
            &["write", "--version", "v", "--archive", "a", "--signature", "s", "--pubkey", "p", "--out", "o", "--"],
            &["check", "--pubkey", "p", "--", "latest.json"],
            &["check", "latest.json", "--pubkey", "p", "--"],
        ] {
            parsed(words);
        }
        let (command, _) = parsed(&["check", "--pubkey", "p", "--", "latest.json"]);
        assert_eq!(command, Command::Check { path: "latest.json".into() });
    }

    #[test]
    fn help_is_claps() {
        for words in [&["--help"][..], &["-h"], &["--help", "junk"], &["key-id", "--pubkey", "p", "--help"]] {
            assert!(help(words).contains("Usage:"), "{words:?}");
        }
    }

    /// Everything else keeps its old meaning: all options work on every command, repeats take the last,
    /// a value may look like a flag, and `-` is a path.
    #[test]
    fn the_options_keep_their_meaning() {
        let (command, options) = parsed(&["key-id", "--version", "v", "--archive", "a", "--pubkey", "p"]);
        assert_eq!(command, Command::KeyId);
        assert_eq!(options.version.as_deref(), Some("v"), "unused options are still accepted");
        assert_eq!(options.archive.as_deref(), Some("a"));

        let (_, options) = parsed(&["key-id", "--pubkey", "a", "--pubkey", "b"]);
        assert_eq!(options.pubkey.as_deref(), Some("b"), "the last one wins");

        let (_, options) = parsed(&["key-id", "--pubkey", "-h"]);
        assert_eq!(options.pubkey.as_deref(), Some("-h"), "a value, not help");

        let (command, options) = parsed(&["check", "-", "--pubkey", "p"]);
        assert_eq!(command, Command::Check { path: "-".into() });
        assert_eq!(options.pubkey.as_deref(), Some("p"));

        // A bare `--` is clap's terminator now; the forms above parse (test `the_new_spellings_parse`).
    }
}
