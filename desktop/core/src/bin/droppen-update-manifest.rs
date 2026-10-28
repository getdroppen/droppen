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
//! The grammar is clap's: unknown options, `--name=value` and positional words
//! a command does not take are refused rather than ignored.

use std::path::Path;
use std::process::ExitCode;
use std::time::SystemTime;

use clap::{Args, Parser, Subcommand};
use droppen_desktop_core::update::{archive_url, key_id, release_page, Manifest, Rules};

const USAGE: &str = "usage:
  droppen-update-manifest write --version <v> --archive <Droppen.app.tar.gz> --signature <.sig> --pubkey <file> --out <latest.json> [--url <url>] [--notes <url>] [--loopback-http]
  droppen-update-manifest check <latest.json> --pubkey <file> [--archive <Droppen.app.tar.gz>] [--loopback-http]
  droppen-update-manifest key-id --pubkey <file>";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("droppen-update-manifest: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<String, String> {
    let (command, options) = parse(&args)?;
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

/// The tool's options. Every command accepts all of them, as the hand parser
/// did; only the command that uses one reads it.
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

/// `droppen-update-manifest`'s command line. The command word is first (the
/// hand parser took the first argument as the command), then its options.
#[derive(Debug, Parser)]
#[command(
    name = "droppen-update-manifest",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
struct ManifestCli {
    #[command(subcommand)]
    command: Option<ManifestCommand>,
}

#[derive(Debug, Subcommand)]
enum ManifestCommand {
    #[command(disable_help_flag = true, disable_version_flag = true, args_override_self = true)]
    Write(Options),
    #[command(disable_help_flag = true, disable_version_flag = true, args_override_self = true)]
    Check(CheckArgs),
    #[command(disable_help_flag = true, disable_version_flag = true, args_override_self = true)]
    KeyId(Options),
}

/// Parse the command line, or say what is wrong with it.
fn parse(args: &[String]) -> Result<(Command, Options), String> {
    scan(args)?;
    let cli = ManifestCli::try_parse_from(std::iter::once("droppen-update-manifest".to_string()).chain(args.iter().cloned()))
        .map_err(|error| error.to_string())?;
    match cli.command {
        Some(ManifestCommand::Write(options)) => Ok((Command::Write, options)),
        Some(ManifestCommand::KeyId(options)) => Ok((Command::KeyId, options)),
        Some(ManifestCommand::Check(check)) => Ok((Command::Check { path: check.path }, check.options)),
        None => Err(USAGE.to_string()),
    }
}

/// The spellings this tool refuses before clap sees them.
///
/// `--name=value` is the one approved break from the old accepted argv: it is
/// now an error. `-h`/`--help` keep their old meaning (this usage as an error,
/// not clap's help, so the exit status stays 1). The token after a value-taking
/// option is skipped: it is that option's value, whatever it looks like.
fn scan(args: &[String]) -> Result<(), String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" | "--archive" | "--signature" | "--pubkey" | "--out" | "--url" | "--notes" => {
                iter.next();
            }
            "--loopback-http" => {}
            // Refused, not clap's terminator: this tool has no positional words after a `--`, and
            // accepting one would loosen the parser the owner asked to tighten.
            "--" => return Err(format!("-- is not accepted by this tool\n{USAGE}")),
            "-h" | "--help" => return Err(USAGE.to_string()),
            other if is_attached(other) => {
                return Err(format!("{other}: the --name=value spelling is not accepted; use --name value\n{USAGE}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// A value-taking option and its value in one word.
fn is_attached(arg: &str) -> bool {
    ["--version=", "--archive=", "--signature=", "--pubkey=", "--out=", "--url=", "--notes="]
        .iter()
        .any(|prefix| arg.starts_with(prefix))
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
        parse(&args(words)).unwrap_or_else(|error| panic!("{words:?} must parse: {error}"))
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

    /// The strictness the owner approved: unknown flags and `--name=value` are errors, and so are extra
    /// positional words and options before the command word.
    #[test]
    fn the_strict_refusals() {
        // `--name=value`: the approved tightening, with its own message.
        for words in [&["write", "--version", "v", "--pubkey=p"][..], &["write", "--pubkey=p"]] {
            let error = parse(&args(words)).unwrap_err();
            assert!(error.contains("--name=value spelling is not accepted"), "{words:?}: {error}");
        }
        // Unknown options and extra positionals: clap's unexpected argument.
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
        // A missing value, and the old help-as-error.
        assert!(parse(&args(&["key-id", "--pubkey"])).unwrap_err().contains("a value is required"));
        for words in [&["key-id", "--pubkey", "p", "--help"][..], &["key-id", "--pubkey", "p", "-h"]] {
            assert_eq!(parse(&args(words)).unwrap_err(), USAGE, "{words:?}");
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

        // A bare `--` is refused (it used to swallow the next word as the value of an empty-named option).
        for words in [
            &["key-id", "--pubkey", "p", "--"][..],
            &["write", "--version", "v", "--archive", "a", "--signature", "s", "--pubkey", "p", "--out", "o", "--"],
            &["check", "--pubkey", "p", "--", "latest.json"],
            &["check", "latest.json", "--pubkey", "p", "--"],
        ] {
            let error = parse(&args(words)).unwrap_err();
            assert!(error.contains("-- is not accepted"), "{words:?}: {error}");
        }
    }
}
