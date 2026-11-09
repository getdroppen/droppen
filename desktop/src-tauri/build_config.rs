//! What `build.rs` reads of the Tauri configuration, apart from it so that
//! `tests/build_config.rs` can test it: which file a target's build merges
//! over `tauri.conf.json`, the bundle identifier the merged files give, and
//! the one the app is compiled with.

use serde_json::Value;

/// The platform's own configuration, which Tauri merges over `tauri.conf.json`
/// in a build for `target` (a target triple): the file
/// `tauri_utils::platform::Target::from_triple` names, `tauri.linux.conf.json`
/// for anything it does not recognise.
pub fn platform_config(target: &str) -> &'static str {
    if target.contains("darwin") {
        "tauri.macos.conf.json"
    } else if target.contains("windows") {
        "tauri.windows.conf.json"
    } else if target.contains("android") {
        "tauri.android.conf.json"
    } else if target.contains("ios") {
        "tauri.ios.conf.json"
    } else {
        "tauri.linux.conf.json"
    }
}

/// The bundle identifier of `configs`, merged in that order: the last one that
/// names one.
pub fn identifier(configs: &[Value]) -> Option<String> {
    configs
        .iter()
        .filter_map(|config| config.get("identifier").and_then(Value::as_str))
        .next_back()
        .map(str::to_string)
}

/// `bundle_id: "…"` of `pub const <name>: Identity = Identity { … };`.
pub fn bundle_id_of(source: &str, name: &str) -> Option<String> {
    let start = source.find(&format!("pub const {name}: Identity = Identity {{"))?;
    let block = &source[start..start + source[start..].find("};")?];
    let line = block.lines().find_map(|line| line.trim().strip_prefix("bundle_id: \""))?;
    Some(line.strip_suffix("\",")?.to_string())
}
