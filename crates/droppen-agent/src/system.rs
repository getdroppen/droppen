//! What this computer runs, for its status stream (`docs/local-aggregator.md`
//! §8): `os` — `{name, version, arch}`, plus `distro` on Linux — read once
//! from the system (`SystemVersion.plist` on macOS, `os-release` on Linux),
//! and the agent's own version.

use std::sync::OnceLock;

use serde::Serialize;

/// The most a field read from the system keeps.
const MAX_FIELD: usize = 64;

/// A computer's system as its status says it: `{name, version, arch}`, with
/// `distro` before `arch` on Linux only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Os {
    /// Rust's name for the system: `macos`, `linux`, …
    pub name: String,
    /// The version its system file names, if it names one.
    pub version: Option<String>,
    /// Linux's os-release `ID`: `null` when the file has none, and left out on
    /// every other system.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distro: Option<Option<String>>,
    /// The CPU architecture, Rust's name for it.
    pub arch: String,
}

/// This computer's system, read once.
pub fn os() -> &'static Os {
    static OS: OnceLock<Os> = OnceLock::new();
    OS.get_or_init(|| detect(std::env::consts::OS, std::env::consts::ARCH, |path| std::fs::read_to_string(path).ok()))
}

/// A field as kept: trimmed, printable, at most [`MAX_FIELD`] characters;
/// `None` when nothing is left.
fn field(text: &str) -> Option<String> {
    let kept: String = text.trim().chars().filter(|c| !c.is_control()).take(MAX_FIELD).collect();
    (!kept.is_empty()).then_some(kept)
}

/// `<key>{key}</key>` then `<string>…</string>` in an XML property list.
fn plist_string(text: &str, key: &str) -> Option<String> {
    let after = &text[text.find(&format!("<key>{key}</key>"))?..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")?;
    field(&after[start..start + end])
}

/// `KEY=value` or `KEY="value"` in an `os-release` file.
fn os_release(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(key)?.strip_prefix('=')?;
        field(value.trim_matches(|c| c == '"' || c == '\''))
    })
}

/// The system `os` (Rust's name for it) on `arch`, reading files with `read`.
pub(crate) fn detect(os: &str, arch: &str, read: impl Fn(&str) -> Option<String>) -> Os {
    match os {
        "macos" => Os {
            name: "macos".into(),
            version: read("/System/Library/CoreServices/SystemVersion.plist").and_then(|t| plist_string(&t, "ProductVersion")),
            distro: None,
            arch: arch.into(),
        },
        "linux" => {
            let release = read("/etc/os-release").or_else(|| read("/usr/lib/os-release")).unwrap_or_default();
            Os {
                name: "linux".into(),
                version: os_release(&release, "VERSION_ID"),
                distro: Some(os_release(&release, "ID")),
                arch: arch.into(),
            }
        }
        other => Os {
            name: other.into(),
            version: None,
            distro: None,
            arch: arch.into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_system_is_read_from_the_files_each_one_keeps() {
        let plist = r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>ProductBuildVersion</key><string>24D70</string>
            <key>ProductName</key><string>macOS</string>
            <key>ProductVersion</key>
            <string>15.3.1</string></dict></plist>"#;
        let mac = detect("macos", "aarch64", |p| {
            (p == "/System/Library/CoreServices/SystemVersion.plist").then(|| plist.to_string())
        });
        assert_eq!(serde_json::to_value(mac).unwrap(), json!({"name": "macos", "version": "15.3.1", "arch": "aarch64"}));
        let release = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n";
        let linux = detect("linux", "x86_64", |p| (p == "/usr/lib/os-release").then(|| release.to_string()));
        assert_eq!(serde_json::to_value(linux).unwrap(), json!({"name": "linux", "version": "24.04", "distro": "ubuntu", "arch": "x86_64"}));
        // Nothing to read: the name and the architecture still.
        assert_eq!(
            serde_json::to_value(detect("macos", "x86_64", |_| None)).unwrap(),
            json!({"name": "macos", "version": null, "arch": "x86_64"})
        );
        assert_eq!(detect("freebsd", "x86_64", |_| None).name, "freebsd");
        // What a file says is kept short and printable.
        let long = format!("VERSION_ID={}\u{7}", "9".repeat(200));
        assert_eq!(os_release(&long, "VERSION_ID").unwrap().len(), MAX_FIELD);
        assert_eq!(os_release("VERSION_ID=\"\"", "VERSION_ID"), None);
        assert!(!os().name.is_empty());
    }

    /// The exact bytes each system writes into the status, which its
    /// fingerprint covers: `distro` only on Linux, and `null` there when
    /// os-release names no `ID`.
    #[test]
    fn each_system_writes_the_same_bytes() {
        let bytes = |os: &str, arch: &str, file: Option<&str>| serde_json::to_string(&detect(os, arch, |_| file.map(str::to_string))).unwrap();
        let plist = "<key>ProductVersion</key><string>15.3.1</string>";
        assert_eq!(bytes("macos", "aarch64", Some(plist)), r#"{"name":"macos","version":"15.3.1","arch":"aarch64"}"#);
        assert_eq!(bytes("macos", "x86_64", None), r#"{"name":"macos","version":null,"arch":"x86_64"}"#);
        assert_eq!(
            bytes("linux", "x86_64", Some("VERSION_ID=\"24.04\"\nID=ubuntu\n")),
            r#"{"name":"linux","version":"24.04","distro":"ubuntu","arch":"x86_64"}"#
        );
        assert_eq!(
            bytes("linux", "aarch64", Some("VERSION_ID=12\n")),
            r#"{"name":"linux","version":"12","distro":null,"arch":"aarch64"}"#
        );
        assert_eq!(bytes("linux", "x86_64", None), r#"{"name":"linux","version":null,"distro":null,"arch":"x86_64"}"#);
        assert_eq!(bytes("freebsd", "x86_64", Some("ID=freebsd\n")), r#"{"name":"freebsd","version":null,"arch":"x86_64"}"#);
    }
}
