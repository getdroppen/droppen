//! Write or check the updater manifest, `latest.json`, of a Droppen release.
//! `desktop/updater-artifacts.sh` runs it; see `desktop/SIGNING.md`.
//!
//! ```text
//! droppen-update-manifest write --version <v> --archive <Droppen.app.tar.gz> --signature <.sig>
//!                               --pubkey <file> --out <latest.json> [--url <url>] [--notes <url>] [--loopback-http]
//!                               [--linux aarch64=<AppImage>,<.sig> --linux x86_64=<AppImage>,<.sig>]
//! droppen-update-manifest check <latest.json> --pubkey <file> [--archive <Droppen.app.tar.gz>] [--loopback-http]
//!                               [--linux aarch64=<AppImage>,<.sig> --linux x86_64=<AppImage>,<.sig>]
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
//! `--linux <arch>=<AppImage>,<sig>`, given for both architectures or for
//! neither, adds a release's Linux app: the AppImage must be named
//! `Droppen-<version>-<arch>.AppImage`, and its signature be by the same key,
//! for that name and version. `write` offers each under
//! `linux-<arch>-appimage`, beside `--url`'s archive; `check` verifies each
//! against the manifest's signature, which must be the `.sig` given. Without
//! `--linux`, `write` writes the Mac's manifest exactly as before, and `check`
//! checks an AppImage's signature by its form and fields only.
//!
//! The grammar is clap's: unknown options and positional words a command does
//! not take are refused rather than ignored.

use std::path::Path;
use std::process::ExitCode;
use std::time::SystemTime;

use clap::{Args, Parser, Subcommand};
use droppen_desktop_core::update::{appimage, appimage_target, archive_url, key_id, release_page, Manifest, Rules, LINUX_ARCHES};
use url::Url;

const USAGE: &str = "usage:
  droppen-update-manifest write --version <v> --archive <Droppen.app.tar.gz> --signature <.sig> --pubkey <file> --out <latest.json> [--url <url>] [--notes <url>] [--loopback-http] [--linux <arch>=<AppImage>,<.sig> ...]
  droppen-update-manifest check <latest.json> --pubkey <file> [--archive <Droppen.app.tar.gz>] [--loopback-http] [--linux <arch>=<AppImage>,<.sig> ...]
  droppen-update-manifest key-id --pubkey <file>";

fn main() -> ExitCode {
    let result = match ManifestCli::parse().command {
        ManifestCommand::Write(write) => write.run(SystemTime::now()),
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

/// A repeated option takes its last value; `--linux`, given once per
/// architecture, adds up.
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
    /// `<arch>=<AppImage>,<.sig>`; repeated, one per architecture.
    #[arg(long, allow_hyphen_values = true, action = clap::ArgAction::Append)]
    linux: Vec<String>,
}

impl Options {
    /// Read `--pubkey`, `--archive` and `--linux`'s files. Every command reads
    /// them before its own work, so a path that cannot be read stops
    /// whichever command names it.
    fn inputs(&self) -> Result<Inputs, String> {
        let pubkey = self.pubkey.as_deref().ok_or_else(|| format!("--pubkey is required\n{USAGE}"))?;
        let pubkey = read_pubkey(Path::new(pubkey))?;
        let key = key_id(&pubkey)?;
        let archive = self
            .archive
            .as_deref()
            .map(|path| std::fs::read(path).map_err(|error| format!("{path}: {error}")))
            .transpose()?;
        let (linux, appimages) = linux_appimages(&self.linux)?;
        Ok(Inputs {
            pubkey,
            key,
            archive,
            appimages,
            linux,
            loopback_http: self.loopback_http > 0,
        })
    }
}

/// The key a command checks with, its id, the archive when one is named, and
/// the AppImages `--linux` names.
struct Inputs {
    pubkey: String,
    key: String,
    archive: Option<Vec<u8>>,
    /// Each AppImage's architecture and bytes, as [`Rules`] takes them.
    appimages: Vec<(String, Vec<u8>)>,
    linux: Vec<LinuxAppImage>,
    loopback_http: bool,
}

impl Inputs {
    fn rules(&self) -> Rules<'_> {
        Rules {
            pubkey: &self.pubkey,
            archive: self.archive.as_deref(),
            appimages: &self.appimages,
            loopback_http: self.loopback_http,
        }
    }

    /// Refuse an AppImage that is not named as the release of `version`
    /// publishes it: its signature names the file it was made for, which
    /// the release's must be.
    fn appimages_named_for(&self, version: &str) -> Result<(), String> {
        for named in &self.linux {
            let want = appimage(version, &named.arch);
            if Path::new(&named.path).file_name().and_then(|name| name.to_str()) != Some(want.as_str()) {
                return Err(format!(
                    "--linux {}: {} is not named {want}, the file the release publishes",
                    named.arch, named.path
                ));
            }
        }
        Ok(())
    }
}

/// One `--linux <arch>=<AppImage>,<.sig>`, its signature read (its bytes are
/// in [`Inputs::appimages`]).
struct LinuxAppImage {
    arch: String,
    path: String,
    signature_path: String,
    signature: String,
}

/// `--linux`'s AppImages, and each one's bytes by architecture.
type LinuxInputs = (Vec<LinuxAppImage>, Vec<(String, Vec<u8>)>);

/// Read `--linux`'s AppImages and their signatures: one per architecture of
/// [`LINUX_ARCHES`], or none. A release that ships the Linux app ships it for
/// both, and an AppImage install whose architecture the manifest left out
/// would find no update, ever.
fn linux_appimages(values: &[String]) -> Result<LinuxInputs, String> {
    let mut read = Vec::new();
    let mut appimages = Vec::new();
    for value in values {
        let (arch, paths) = value
            .split_once('=')
            .ok_or_else(|| format!("--linux {value:?}: not <arch>=<AppImage>,<.sig>"))?;
        if !LINUX_ARCHES.contains(&arch) {
            return Err(format!("--linux {value:?}: the architecture is one of {LINUX_ARCHES:?}"));
        }
        // A comma in a path would make the split a guess.
        let (path, signature_path) = match paths.split(',').collect::<Vec<_>>()[..] {
            [path, signature_path] if !path.is_empty() && !signature_path.is_empty() => (path, signature_path),
            _ => return Err(format!("--linux {value:?}: not <arch>=<AppImage>,<.sig> (no comma in either path)")),
        };
        if read.iter().any(|named: &LinuxAppImage| named.arch == arch) {
            return Err(format!("--linux {arch} is given twice"));
        }
        let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
        let signature = std::fs::read_to_string(signature_path).map_err(|error| format!("{signature_path}: {error}"))?;
        read.push(LinuxAppImage {
            arch: arch.to_string(),
            path: path.to_string(),
            signature_path: signature_path.to_string(),
            signature,
        });
        appimages.push((arch.to_string(), bytes));
    }
    if !read.is_empty() && read.len() != LINUX_ARCHES.len() {
        let given: Vec<&str> = read.iter().map(|named| named.arch.as_str()).collect();
        return Err(format!(
            "--linux names {given:?} only: a release ships the Linux app for {LINUX_ARCHES:?}, and offers each of them"
        ));
    }
    Ok((read, appimages))
}

/// The address of `file` beside the archive at `url`: a release's files are
/// all in one place.
fn beside(url: &str, file: &str) -> Result<String, String> {
    Url::parse(url)
        .and_then(|url| url.join(file))
        .map(String::from)
        .map_err(|error| format!("--url {url:?}: {error}"))
}

#[derive(Debug, PartialEq, Eq, Args)]
struct WriteArgs {
    #[command(flatten)]
    options: Options,
}

impl WriteArgs {
    /// Verify the signatures, then write the manifest, dated `published`; the
    /// line that says so.
    fn run(self, published: SystemTime) -> Result<String, String> {
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
        inputs.appimages_named_for(&version)?;
        let mut manifest = Manifest::new(&version, &notes, &url, &signature, published);
        for named in &inputs.linux {
            manifest.add_appimage(&named.arch, &beside(&url, &appimage(&version, &named.arch))?, &named.signature);
        }
        refuse(manifest.check(&inputs.rules()))?;
        let out = required("out", options.out)?;
        std::fs::write(&out, manifest.to_json()).map_err(|error| format!("{out}: {error}"))?;
        if inputs.linux.is_empty() {
            Ok(format!(
                "wrote {out}: Droppen {version}, {url}, signature verified with key {}",
                inputs.key
            ))
        } else {
            Ok(format!(
                "wrote {out}: Droppen {version}, {url} and the AppImages for {LINUX_ARCHES:?} beside it, signatures verified with key {}",
                inputs.key
            ))
        }
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
        inputs.appimages_named_for(&manifest.version)?;
        let mut problems = manifest.check(&inputs.rules());
        // The signature the release publishes beside each AppImage is the one
        // the manifest offers it with.
        for named in &inputs.linux {
            let target = appimage_target(&named.arch);
            let offered = manifest.platforms.get(&target).map(|platform| platform.signature.as_str());
            if offered.is_some_and(|offered| offered != named.signature.trim()) {
                problems.push(format!("{target}: the signature is not {}'s", named.signature_path));
            }
        }
        refuse(problems)?;
        let key = &inputs.key;
        let appimages = match (manifest.has_appimages(), inputs.linux.is_empty()) {
            (false, _) => String::new(),
            (true, false) => "; the AppImages verified too".to_string(),
            (true, true) => "; the AppImages not verified: no --linux".to_string(),
        };
        Ok(format!(
            "{path}: Droppen {} is fit to publish{}",
            manifest.version,
            if inputs.archive.is_some() {
                format!(" (signature by key {key}, verified against the archive{appimages})")
            } else {
                format!(" (signature by key {key}, not verified: no --archive{appimages})")
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
                    linux: Vec::new(),
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
                        linux: Vec::new(),
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
                    linux: Vec::new(),
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

    /// `--linux` is the one option that adds up rather than taking its last value.
    #[test]
    fn linux_is_given_once_per_architecture() {
        assert_eq!(
            parse(&[
                "write",
                "--version",
                "0.2.140",
                "--linux",
                "x86_64=/l/Droppen-0.2.140-x86_64.AppImage,/l/Droppen-0.2.140-x86_64.AppImage.sig",
                "--linux=aarch64=/l/Droppen-0.2.140-aarch64.AppImage,/l/Droppen-0.2.140-aarch64.AppImage.sig",
            ])
            .unwrap(),
            ManifestCommand::Write(WriteArgs {
                options: Options {
                    version: some("0.2.140"),
                    linux: vec![
                        "x86_64=/l/Droppen-0.2.140-x86_64.AppImage,/l/Droppen-0.2.140-x86_64.AppImage.sig".to_string(),
                        "aarch64=/l/Droppen-0.2.140-aarch64.AppImage,/l/Droppen-0.2.140-aarch64.AppImage.sig".to_string(),
                    ],
                    ..Options::default()
                }
            })
        );
    }

    const VERSION: &str = "0.0.2-test";

    fn testdata(name: &str) -> String {
        format!("{}/testdata/update/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    /// The pub_date the library's tests write: `latest-macos.json`'s.
    fn published() -> SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_792_396_800)
    }

    fn linux(arch: &str, signature: &str) -> String {
        format!(
            "{arch}={},{}",
            testdata(&format!("with-linux/Droppen-{VERSION}-{arch}.AppImage")),
            testdata(signature)
        )
    }

    fn both_linux() -> Vec<String> {
        vec![
            linux("x86_64", &format!("with-linux/Droppen-{VERSION}-x86_64.AppImage.sig")),
            linux("aarch64", &format!("with-linux/Droppen-{VERSION}-aarch64.AppImage.sig")),
        ]
    }

    /// `write` of the release that ships the Linux app, into `out`.
    fn write_release(out: &Path, linux: Vec<String>) -> Result<String, String> {
        WriteArgs {
            options: Options {
                version: some(VERSION),
                archive: Some(testdata("Droppen.app.tar.gz")),
                signature: Some(testdata("with-linux/Droppen.app.tar.gz.sig")),
                pubkey: Some(testdata("with-linux/test.key.pub")),
                out: Some(out.display().to_string()),
                linux,
                ..Options::default()
            },
        }
        .run(published())
    }

    fn check_release(path: &Path, linux: Vec<String>) -> Result<String, String> {
        CheckArgs {
            path: path.display().to_string(),
            options: Options {
                archive: Some(testdata("Droppen.app.tar.gz")),
                pubkey: Some(testdata("with-linux/test.key.pub")),
                linux,
                ..Options::default()
            },
        }
        .run()
    }

    /// Without `--linux`, `write` writes what it always has, byte for byte.
    #[test]
    fn a_macos_only_write_is_what_it_always_was() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let out = dir.path().join("latest.json");
        let wrote = WriteArgs {
            options: Options {
                version: some(VERSION),
                archive: Some(testdata("Droppen.app.tar.gz")),
                signature: Some(testdata("Droppen.app.tar.gz.sig")),
                pubkey: Some(testdata("test.key.pub")),
                out: Some(out.display().to_string()),
                ..Options::default()
            },
        }
        .run(published())
        .expect("written");
        let key = key_id(&read_pubkey(Path::new(&testdata("test.key.pub"))).expect("a key")).expect("a key id");
        assert!(
            wrote.ends_with(&format!("Droppen.app.tar.gz, signature verified with key {key}")),
            "{wrote}"
        );
        assert_eq!(
            std::fs::read_to_string(&out).expect("latest.json"),
            include_str!("../../testdata/update/latest-macos.json")
        );
    }

    #[test]
    fn a_release_with_the_linux_app_round_trips() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let out = dir.path().join("latest.json");
        let wrote = write_release(&out, both_linux()).expect("written");
        assert!(wrote.contains("and the AppImages for [\"aarch64\", \"x86_64\"] beside it"), "{wrote}");

        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).expect("latest.json")).expect("json");
        let keys: Vec<&str> = json["platforms"].as_object().expect("platforms").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["darwin-aarch64", "darwin-x86_64", "linux-aarch64-appimage", "linux-x86_64-appimage"],
            "never a bare linux-<arch>"
        );
        assert_eq!(
            json["platforms"]["linux-x86_64-appimage"]["url"],
            "https://github.com/getdroppen/droppen/releases/download/v0.0.2-test/Droppen-0.0.2-test-x86_64.AppImage"
        );

        let checked = check_release(&out, both_linux()).expect("fit");
        assert!(
            checked.ends_with("verified against the archive; the AppImages verified too)"),
            "{checked}"
        );
        let checked = check_release(&out, Vec::new()).expect("fit");
        assert!(checked.ends_with("; the AppImages not verified: no --linux)"), "{checked}");

        // The release's .sig must be the one the manifest offers.
        let refused = check_release(&out, vec![linux("x86_64", "with-linux/wrong-version.sig"), both_linux().remove(1)]).unwrap_err();
        assert!(refused.contains("linux-x86_64-appimage: the signature is not "), "{refused}");
    }

    /// The tool's beside-the-archive addresses, for the end-to-end test's local server.
    #[test]
    fn the_appimages_are_beside_the_archive() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let out = dir.path().join("latest.json");
        let mut write = WriteArgs {
            options: Options {
                version: some(VERSION),
                archive: Some(testdata("Droppen.app.tar.gz")),
                signature: Some(testdata("with-linux/Droppen.app.tar.gz.sig")),
                pubkey: Some(testdata("with-linux/test.key.pub")),
                url: some("http://127.0.0.1:8123/v0.0.2-test/Droppen.app.tar.gz"),
                out: Some(out.display().to_string()),
                linux: both_linux(),
                ..Options::default()
            },
        };
        write.options.loopback_http = 1;
        write.run(published()).expect("written");
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).expect("latest.json")).expect("json");
        assert_eq!(
            json["platforms"]["linux-aarch64-appimage"]["url"],
            "http://127.0.0.1:8123/v0.0.2-test/Droppen-0.0.2-test-aarch64.AppImage"
        );
    }

    #[test]
    fn a_wrong_linux_appimage_or_signature_writes_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let out = dir.path().join("latest.json");
        for (linux, why) in [
            // A signature for another version, or for another file name.
            (
                vec![linux("x86_64", "with-linux/wrong-version.sig"), both_linux().remove(1)],
                "linux-x86_64-appimage: the signature was made for version 0.0.3-test, not 0.0.2-test",
            ),
            (
                vec![linux("x86_64", "with-linux/renamed.sig"), both_linux().remove(1)],
                "linux-x86_64-appimage: the signature was made for Some(\"Droppen_0.0.2-test_amd64.AppImage\")",
            ),
            // One architecture's AppImage and signature given as the other's.
            (
                vec![
                    format!(
                        "x86_64={},{}",
                        testdata("with-linux/Droppen-0.0.2-test-aarch64.AppImage"),
                        testdata("with-linux/Droppen-0.0.2-test-aarch64.AppImage.sig")
                    ),
                    both_linux().remove(1),
                ],
                "is not named Droppen-0.0.2-test-x86_64.AppImage",
            ),
            // One architecture only, or one twice.
            (vec![both_linux().remove(0)], "--linux names [\"x86_64\"] only"),
            (vec![both_linux().remove(0), both_linux().remove(0)], "--linux x86_64 is given twice"),
            // Not <arch>=<AppImage>,<.sig>.
            (vec!["x86_64".to_string(), both_linux().remove(1)], "not <arch>=<AppImage>,<.sig>"),
            (vec!["armv7=a,b".to_string(), both_linux().remove(1)], "the architecture is one of"),
            (vec!["x86_64=a".to_string(), both_linux().remove(1)], "no comma in either path"),
            (vec!["x86_64=a,b,c".to_string(), both_linux().remove(1)], "no comma in either path"),
            (vec!["x86_64=,b".to_string(), both_linux().remove(1)], "no comma in either path"),
        ] {
            let refused = write_release(&out, linux.clone()).unwrap_err();
            assert!(refused.contains(why), "{linux:?}: {refused}");
            assert!(!out.exists(), "{linux:?}: nothing is written");
        }
    }
}
