//! `droppen-update-signer`: sign a Droppen update archive for the Tauri
//! updater, exactly as `tauri signer sign --app-version <version> <file>`
//! (tauri-cli 2.12.3, `helpers/updater_signature.rs`) does:
//!
//! ```text
//! droppen-update-signer sign <archive> --version <version> [--key-file <file>]
//! ```
//!
//! The key is the Tauri updater key: base64 of a minisign secret key file
//! (encrypted with a password), from `--key-file` or `TAURI_SIGNING_PRIVATE_KEY`;
//! its password from `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (empty when unset).
//! Neither is ever printed or taken from the command line. It writes
//! `<archive>.sig`: base64 of the minisign signature, whose trusted comment is
//! `timestamp:<unix time>\tfile:<archive's name>\tversion:<version>`, which the
//! updater (`requireSignedVersion`) and `desktop/core`'s `update.rs` hold to
//! the version announced.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use minisign::{SecretKey, SecretKeyBox, SignatureBox};

/// The untrusted comment tauri-cli writes.
const UNTRUSTED_COMMENT: &str = "signature from tauri secret key";

/// The Tauri updater key: base64 of a minisign secret key file, decrypted
/// with `password` (as tauri-cli's `secret_key`).
pub fn secret_key(private_key: &str, password: &str) -> Result<SecretKey, String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(private_key.trim())
        .map_err(|_| "the updater key is not base64".to_string())?;
    let text = String::from_utf8(decoded).map_err(|_| "the updater key is not text".to_string())?;
    let sk_box = SecretKeyBox::from_string(&text).map_err(|_| "the updater key is not a minisign secret key".to_string())?;
    sk_box
        .into_secret_key(Some(password.to_string()))
        .map_err(|_| "cannot decrypt the updater key (wrong TAURI_SIGNING_PRIVATE_KEY_PASSWORD?)".to_string())
}

/// The trusted comment of a signature of `file_name` for `version`, made at
/// `timestamp` (tauri-cli's `sign_file`).
pub fn trusted_comment(file_name: &str, version: &str, timestamp: u64) -> Result<String, String> {
    if file_name.contains(['\t', '\r', '\n']) {
        return Err(format!("the file {file_name:?} cannot be signed: its name holds a tab or a newline"));
    }
    if version.is_empty() || version.contains(['\t', '\r', '\n']) {
        return Err(format!("the version {version:?} cannot be signed"));
    }
    Ok(format!("timestamp:{timestamp}\tfile:{file_name}\tversion:{version}"))
}

/// Sign `archive` for `version`: the signature, and its `.sig` file's text.
pub fn sign(key: &SecretKey, archive: &Path, version: &str, timestamp: u64) -> Result<(SignatureBox, String), String> {
    let file_name = archive
        .file_name()
        .ok_or_else(|| format!("{} is not a file", archive.display()))?
        .to_string_lossy()
        .into_owned();
    let comment = trusted_comment(&file_name, version, timestamp)?;
    let data = BufReader::new(File::open(archive).map_err(|e| format!("cannot read {}: {e}", archive.display()))?);
    let signature = minisign::sign(None, key, data, Some(&comment), Some(UNTRUSTED_COMMENT))
        .map_err(|e| format!("cannot sign {}: {e}", archive.display()))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature.to_string());
    Ok((signature, encoded))
}

/// `<archive>.sig`, as tauri-cli names it.
pub fn signature_path(archive: &Path) -> PathBuf {
    let mut name = archive.as_os_str().to_os_string();
    name.push(".sig");
    PathBuf::from(name)
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = "usage: droppen-update-signer sign <archive> --version <version> [--key-file <file>]";
    let (mut archive, mut version, mut key_file) = (None, None, None);
    let mut rest = args.iter();
    match rest.next().map(String::as_str) {
        Some("sign") => {}
        _ => return Err(usage.to_string()),
    }
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--version" => version = Some(rest.next().ok_or(usage)?.clone()),
            "--key-file" => key_file = Some(rest.next().ok_or(usage)?.clone()),
            other if !other.starts_with('-') && archive.is_none() => archive = Some(PathBuf::from(other)),
            _ => return Err(usage.to_string()),
        }
    }
    let (archive, version) = (archive.ok_or(usage)?, version.ok_or(usage)?);
    let private_key = match key_file {
        Some(file) => std::fs::read_to_string(&file).map_err(|e| format!("cannot read the key file {file}: {e}"))?,
        None => std::env::var("TAURI_SIGNING_PRIVATE_KEY")
            .map_err(|_| "no updater key: give --key-file or set TAURI_SIGNING_PRIVATE_KEY".to_string())?,
    };
    let password = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    let key = secret_key(&private_key, &password)?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "the clock is before 1970")?.as_secs();
    let (_, encoded) = sign(&key, &archive, &version, timestamp)?;
    let out = signature_path(&archive);
    std::fs::write(&out, encoded.as_bytes()).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    println!("{}", out.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("droppen-update-signer: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests;
