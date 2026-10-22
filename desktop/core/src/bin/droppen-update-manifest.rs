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

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::time::SystemTime;

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
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let mut positional = Vec::new();
    let mut options = HashMap::new();
    let mut loopback_http = false;
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--loopback-http" => loopback_http = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            flag if flag.starts_with("--") => {
                let value = rest.next().ok_or_else(|| format!("{flag} needs a value"))?;
                options.insert(flag.trim_start_matches("--").to_string(), value.clone());
            }
            _ => positional.push(arg.clone()),
        }
    }
    let option = |name: &str| options.get(name).cloned();
    let required = |name: &str| option(name).ok_or_else(|| format!("--{name} is required\n{USAGE}"));
    let pubkey = read_pubkey(Path::new(&required("pubkey")?))?;
    let key = key_id(&pubkey)?;
    let archive = option("archive")
        .map(|path| std::fs::read(&path).map_err(|error| format!("{path}: {error}")))
        .transpose()?;
    let rules = Rules {
        pubkey: &pubkey,
        archive: archive.as_deref(),
        loopback_http,
    };
    match command.as_str() {
        "write" => {
            let version = required("version")?;
            if archive.is_none() {
                return Err("--archive is required: write verifies the signature before it writes".to_string());
            }
            let signature_path = required("signature")?;
            let signature = std::fs::read_to_string(&signature_path).map_err(|error| format!("{signature_path}: {error}"))?;
            let url = option("url").unwrap_or_else(|| archive_url(&version));
            let notes = option("notes").unwrap_or_else(|| release_page(&version));
            let manifest = Manifest::new(&version, &notes, &url, &signature, SystemTime::now());
            refuse(manifest.check(&rules))?;
            let out = required("out")?;
            std::fs::write(&out, manifest.to_json()).map_err(|error| format!("{out}: {error}"))?;
            Ok(format!("wrote {out}: Droppen {version}, {url}, signature verified with key {key}"))
        }
        "key-id" => Ok(key),
        "check" => {
            let [path] = positional.as_slice() else {
                return Err(USAGE.to_string());
            };
            let json = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
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
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
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
