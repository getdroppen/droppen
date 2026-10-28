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

use clap::{Args, Parser, Subcommand};
use droppen_desktop_core::update::{archive_url, key_id, release_page, Manifest, Rules};

const USAGE: &str = "usage:
  droppen-update-manifest write --version <v> --archive <Droppen.app.tar.gz> --signature <.sig> --pubkey <file> --out <latest.json> [--url <url>] [--notes <url>] [--loopback-http]
  droppen-update-manifest check <latest.json> --pubkey <file> [--archive <Droppen.app.tar.gz>] [--loopback-http]
  droppen-update-manifest key-id --pubkey <file>";

fn main() -> ExitCode {
    let result = match ManifestCli::parse().command {
        ManifestCommand::Write(write) => write.run(),
        ManifestCommand::Check(check) => check.run(),
        ManifestCommand::KeyId(key_id) => key_id.run(),
    };
    match result {
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

/// Write or check the updater manifest, `latest.json`, of a Droppen release.
#[derive(Debug, PartialEq, Eq, Parser)]
#[command(name = "droppen-update-manifest", disable_help_subcommand = true)]
struct ManifestCli {
    #[command(subcommand)]
    command: ManifestCommand,
}

/// A repeated option takes its last value.
#[derive(Debug, PartialEq, Eq, Subcommand)]
enum ManifestCommand {
    /// Verify the archive's signature with the key, then write `latest.json`
    #[command(args_override_self = true)]
    Write(WriteArgs),
    /// Read a `latest.json` back as the app would
    #[command(args_override_self = true)]
    Check(CheckArgs),
    /// Print the key's minisign key id, as `tauri signer` shows it
    #[command(args_override_self = true)]
    KeyId(KeyIdArgs),
}

/// The tool's options. Every command accepts all of them; only the command that
/// uses one reads it.
#[derive(Debug, Default, PartialEq, Eq, Args)]
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

impl Options {
    /// Read `--pubkey` and `--archive`. Every command reads both before its own
    /// work, so a path that cannot be read stops whichever command names it.
    fn inputs(&self) -> Result<Inputs, String> {
        let pubkey = self.pubkey.as_deref().ok_or_else(|| format!("--pubkey is required\n{USAGE}"))?;
        let pubkey = read_pubkey(Path::new(pubkey))?;
        let key = key_id(&pubkey)?;
        let archive = self
            .archive
            .as_deref()
            .map(|path| std::fs::read(path).map_err(|error| format!("{path}: {error}")))
            .transpose()?;
        Ok(Inputs {
            pubkey,
            key,
            archive,
            loopback_http: self.loopback_http > 0,
        })
    }
}

/// The key a command checks with, its id, and the archive when one is named.
struct Inputs {
    pubkey: String,
    key: String,
    archive: Option<Vec<u8>>,
    loopback_http: bool,
}

impl Inputs {
    fn rules(&self) -> Rules<'_> {
        Rules {
            pubkey: &self.pubkey,
            archive: self.archive.as_deref(),
            loopback_http: self.loopback_http,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Args)]
struct WriteArgs {
    #[command(flatten)]
    options: Options,
}

impl WriteArgs {
    /// Verify the signature, then write the manifest; the line that says so.
    fn run(self) -> Result<String, String> {
        let inputs = self.options.inputs()?;
        let options = self.options;
        let required = |name: &str, value: Option<String>| value.ok_or_else(|| format!("--{name} is required\n{USAGE}"));
        let version = required("version", options.version)?;
        if inputs.archive.is_none() {
            return Err("--archive is required: write verifies the signature before it writes".to_string());
        }
        let signature_path = required("signature", options.signature)?;
        let signature = std::fs::read_to_string(&signature_path).map_err(|error| format!("{signature_path}: {error}"))?;
        let url = options.url.unwrap_or_else(|| archive_url(&version));
        let notes = options.notes.unwrap_or_else(|| release_page(&version));
        let manifest = Manifest::new(&version, &notes, &url, &signature, SystemTime::now());
        refuse(manifest.check(&inputs.rules()))?;
        let out = required("out", options.out)?;
        std::fs::write(&out, manifest.to_json()).map_err(|error| format!("{out}: {error}"))?;
        Ok(format!("wrote {out}: Droppen {version}, {url}, signature verified with key {}", inputs.key))
    }
}

#[derive(Debug, PartialEq, Eq, Args)]
struct CheckArgs {
    /// The `latest.json` to read.
    path: String,
    #[command(flatten)]
    options: Options,
}

impl CheckArgs {
    /// Read the manifest back as the app would; the line that says it is fit.
    fn run(self) -> Result<String, String> {
        let inputs = self.options.inputs()?;
        let path = self.path;
        let json = std::fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
        let manifest = Manifest::parse(&json)?;
        refuse(manifest.check(&inputs.rules()))?;
        let key = &inputs.key;
        Ok(format!(
            "{path}: Droppen {} is fit to publish{}",
            manifest.version,
            if inputs.archive.is_some() {
                format!(" (signature by key {key}, verified against the archive)")
            } else {
                format!(" (signature by key {key}, not verified: no --archive)")
            }
        ))
    }
}

#[derive(Debug, PartialEq, Eq, Args)]
struct KeyIdArgs {
    #[command(flatten)]
    options: Options,
}

impl KeyIdArgs {
    fn run(self) -> Result<String, String> {
        Ok(self.options.inputs()?.key)
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
    use clap::error::ErrorKind;

    use super::*;

    fn parse(words: &[&str]) -> Result<ManifestCommand, clap::Error> {
        ManifestCli::try_parse_from(std::iter::once("droppen-update-manifest").chain(words.iter().copied())).map(|cli| cli.command)
    }

    fn some(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    /// The exact invocations the release workflow, the release scripts and
    /// `desktop/SIGNING.md` run, and what each must mean.
    #[test]
    fn the_release_invocations_parse() {
        // desktop/SIGNING.md: verify the app's key.
        assert_eq!(
            parse(&[
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
            ])
            .unwrap(),
            ManifestCommand::Write(WriteArgs {
                options: Options {
                    version: some("0.0.1"),
                    archive: some("Droppen.app.tar.gz"),
                    signature: some("Droppen.app.tar.gz.sig"),
                    pubkey: some("desktop/src-tauri/tauri.conf.json"),
                    out: some("/tmp/check.json"),
                    ..Options::default()
                }
            })
        );

        // .github/workflows/release.yml's "latest.json (droppen-update-manifest write)" step.
        assert_eq!(
            parse(&[
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
            ])
            .unwrap(),
            ManifestCommand::Write(WriteArgs {
                options: Options {
                    version: some("0.2.133"),
                    archive: some("/tmp/a/desktop/updater/Droppen.app.tar.gz"),
                    signature: some("/tmp/a/desktop/updater/Droppen.app.tar.gz.sig"),
                    pubkey: some("desktop/src-tauri/tauri.conf.json"),
                    url: some("https://github.com/getdroppen/droppen/releases/download/v0.2.133/Droppen.app.tar.gz"),
                    notes: some("https://github.com/getdroppen/droppen/releases/tag/v0.2.133"),
                    out: some("/tmp/a/desktop/updater/latest.json"),
                    loopback_http: 0,
                }
            })
        );

        // desktop/updater-artifacts.sh: one release build, one test build.
        for (extra, loopback_http) in [(&[][..], 0), (&["--loopback-http"][..], 1)] {
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
            assert_eq!(
                parse(&words).unwrap(),
                ManifestCommand::Write(WriteArgs {
                    options: Options {
                        version: some("0.2.133"),
                        archive: some("/out/Droppen.app.tar.gz"),
                        signature: some("/out/Droppen.app.tar.gz.sig"),
                        pubkey: some("/keys/updater.pub"),
                        url: some("https://getdroppen.github.io/releases/v0.2.133/Droppen.app.tar.gz"),
                        notes: some("https://getdroppen.github.io/releases/v0.2.133"),
                        out: some("/out/latest.json"),
                        loopback_http,
                    }
                }),
                "{extra:?}"
            );
        }

        // desktop/e2e-updater-macos.sh: the broken update the end-to-end test serves.
        assert_eq!(
            parse(&[
                "write",
                "--version",
                "0.2.134",
                "--archive",
                "/serve/broken/Droppen.app.tar.gz",
                "--signature",
                "/serve/broken/Droppen.app.tar.gz.sig",
                "--pubkey",
                "/keys/test.key.pub",
                "--url",
                "http://127.0.0.1:8080/broken/Droppen.app.tar.gz",
                "--notes",
                "https://github.com/getdroppen/droppen/releases/tag/v0.2.134",
                "--out",
                "/root/broken/latest.json",
                "--loopback-http",
            ])
            .unwrap(),
            ManifestCommand::Write(WriteArgs {
                options: Options {
                    version: some("0.2.134"),
                    archive: some("/serve/broken/Droppen.app.tar.gz"),
                    signature: some("/serve/broken/Droppen.app.tar.gz.sig"),
                    pubkey: some("/keys/test.key.pub"),
                    url: some("http://127.0.0.1:8080/broken/Droppen.app.tar.gz"),
                    notes: some("https://github.com/getdroppen/droppen/releases/tag/v0.2.134"),
                    out: some("/root/broken/latest.json"),
                    loopback_http: 1,
                }
            })
        );

        // desktop/updater-artifacts.sh and scripts/publish-release.sh: key-id of the release key and of the checked key.
        assert_eq!(
            parse(&["key-id", "--pubkey", "/keys/release.pub"]).unwrap(),
            ManifestCommand::KeyId(KeyIdArgs {
                options: Options {
                    pubkey: some("/keys/release.pub"),
                    ..Options::default()
                }
            })
        );

        // scripts/publish-release.sh and desktop/SIGNING.md: check a latest.json against its archive.
        assert_eq!(
            parse(&["check", "latest.json", "--pubkey", "desktop/src-tauri/tauri.conf.json", "--archive", "Droppen.app.tar.gz"]).unwrap(),
            ManifestCommand::Check(CheckArgs {
                path: "latest.json".into(),
                options: Options {
                    pubkey: some("desktop/src-tauri/tauri.conf.json"),
                    archive: some("Droppen.app.tar.gz"),
                    ..Options::default()
                }
            })
        );
        // Without the archive.
        assert_eq!(
            parse(&["check", "latest.json", "--pubkey", "desktop/src-tauri/tauri.conf.json"]).unwrap(),
            ManifestCommand::Check(CheckArgs {
                path: "latest.json".into(),
                options: Options {
                    pubkey: some("desktop/src-tauri/tauri.conf.json"),
                    ..Options::default()
                }
            })
        );
    }

    /// Strict parsing, but clap's: unknown flags, stray words and single-dash
    /// words are refused.
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
            let error = parse(words).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::UnknownArgument, "{words:?}: {error}");
            assert!(error.to_string().contains("unexpected argument"), "{words:?}: {error}");
        }
        // An unknown command word, and no command at all.
        assert_eq!(parse(&["bogus", "--pubkey", "p"]).unwrap_err().kind(), ErrorKind::InvalidSubcommand);
        assert_eq!(parse(&[]).unwrap_err().kind(), ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand);
        // A missing value.
        let error = parse(&["key-id", "--pubkey"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
        assert!(error.to_string().contains("a value is required"), "{error}");
    }

    /// The spellings clap takes: `--name=value`, and `--` as its terminator.
    #[test]
    fn the_new_spellings_parse() {
        assert_eq!(
            parse(&["key-id", "--pubkey=p"]).unwrap(),
            ManifestCommand::KeyId(KeyIdArgs {
                options: Options {
                    pubkey: some("p"),
                    ..Options::default()
                }
            })
        );

        let write = ManifestCommand::Write(WriteArgs {
            options: Options {
                version: some("0.0.1"),
                archive: some("a"),
                signature: some("s"),
                pubkey: some("p"),
                out: some("o"),
                ..Options::default()
            },
        });
        assert_eq!(parse(&["write", "--version=0.0.1", "--archive=a", "--signature=s", "--pubkey=p", "--out=o"]).unwrap(), write);
        assert_eq!(
            parse(&["write", "--version", "0.0.1", "--archive", "a", "--signature", "s", "--pubkey", "p", "--out", "o", "--"]).unwrap(),
            write
        );

        assert_eq!(
            parse(&["key-id", "--pubkey", "p", "--"]).unwrap(),
            ManifestCommand::KeyId(KeyIdArgs {
                options: Options {
                    pubkey: some("p"),
                    ..Options::default()
                }
            })
        );
        let check = ManifestCommand::Check(CheckArgs {
            path: "latest.json".into(),
            options: Options {
                pubkey: some("p"),
                ..Options::default()
            },
        });
        assert_eq!(parse(&["check", "--pubkey", "p", "--", "latest.json"]).unwrap(), check);
        assert_eq!(parse(&["check", "latest.json", "--pubkey", "p", "--"]).unwrap(), check);
    }

    #[test]
    fn help_is_claps() {
        for words in [&["--help"][..], &["-h"], &["--help", "junk"], &["key-id", "--pubkey", "p", "--help"]] {
            let error = parse(words).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DisplayHelp, "{words:?}");
            assert!(error.to_string().contains("Usage:"), "{words:?}");
        }
    }

    /// Everything else keeps its meaning: all options work on every command, repeats take the last,
    /// a value may look like a flag, and `-` is a path.
    #[test]
    fn the_options_keep_their_meaning() {
        assert_eq!(
            parse(&["key-id", "--version", "v", "--archive", "a", "--pubkey", "p"]).unwrap(),
            ManifestCommand::KeyId(KeyIdArgs {
                options: Options {
                    version: some("v"),
                    archive: some("a"),
                    pubkey: some("p"),
                    ..Options::default()
                }
            }),
            "unused options are still accepted"
        );
        assert_eq!(
            parse(&["key-id", "--pubkey", "a", "--pubkey", "b"]).unwrap(),
            ManifestCommand::KeyId(KeyIdArgs {
                options: Options {
                    pubkey: some("b"),
                    ..Options::default()
                }
            }),
            "the last one wins"
        );
        assert_eq!(
            parse(&["key-id", "--pubkey", "-h"]).unwrap(),
            ManifestCommand::KeyId(KeyIdArgs {
                options: Options {
                    pubkey: some("-h"),
                    ..Options::default()
                }
            }),
            "a value, not help"
        );
        assert_eq!(
            parse(&["check", "-", "--pubkey", "p"]).unwrap(),
            ManifestCommand::Check(CheckArgs {
                path: "-".into(),
                options: Options {
                    pubkey: some("p"),
                    ..Options::default()
                }
            })
        );
    }
}
