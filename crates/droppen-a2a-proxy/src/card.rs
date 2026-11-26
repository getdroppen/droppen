//! The Agent Card, as the proxy hands it on (`docs/a2a.md` §2, §7.4): every
//! interface address the device wrote (`supportedInterfaces[].url`, under its
//! own base: `…/a2a/agents/<entry>/jsonrpc`) becomes the proxy's own
//! (`<base>/a2a/<device>/agents/<entry>/jsonrpc`), so a client that read the
//! card through the proxy keeps calling through it. An address that is not
//! the entry's own is replaced by the one its binding has here (JSON-RPC,
//! HTTP+JSON), or the interface left out: nothing in the card points past
//! the proxy at the device.

use serde_json::Value;

/// Rewrite `card`'s interface addresses for the proxy at `base`, reaching
/// the device as `device`, for entry `entry`.
pub fn rewrite(card: &mut Value, base: &str, device: &str, entry: &str) {
    let ours = |url: &str, binding: Option<&str>| proxied(url, binding, base, device, entry);
    for key in ["supportedInterfaces", "additionalInterfaces"] {
        if let Some(list) = card.get_mut(key).and_then(Value::as_array_mut) {
            list.retain_mut(|interface| {
                let binding = interface
                    .get("protocolBinding")
                    .or_else(|| interface.get("transport"))
                    .and_then(Value::as_str)
                    .map(String::from);
                let url = interface.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
                match ours(&url, binding.as_deref()) {
                    Some(url) => {
                        interface["url"] = Value::String(url);
                        true
                    }
                    None => false,
                }
            });
        }
    }
    // A card of before v1.0 names its preferred address at the top.
    if let Some(url) = card.get("url").and_then(Value::as_str).map(String::from) {
        let binding = card.get("preferredTransport").and_then(Value::as_str).map(String::from);
        match ours(&url, binding.as_deref().or(Some("JSONRPC"))) {
            Some(url) => card["url"] = Value::String(url),
            None => {
                card.as_object_mut().map(|c| c.remove("url"));
            }
        }
    }
}

/// The proxy's address for `url` (an interface of `binding`).
fn proxied(url: &str, binding: Option<&str>, base: &str, device: &str, entry: &str) -> Option<String> {
    let here = format!("{base}/a2a/{device}/agents/");
    // The entry's own address under the device's base: kept past its entry.
    let path = url.split_once("://").map_or(url, |(_, rest)| rest.find('/').map_or("", |i| &rest[i..]));
    if let Some((_, after)) = path.split_once("/a2a/agents/") {
        let own = after == entry || after.strip_prefix(entry).is_some_and(|r| r.starts_with('/'));
        if own && !after.contains(['?', '#']) {
            return Some(format!("{here}{after}"));
        }
    }
    match binding {
        Some("JSONRPC") => Some(format!("{here}{entry}/jsonrpc")),
        Some("HTTP+JSON") => Some(format!("{here}{entry}/rest")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_cards_addresses_become_the_proxys_own() {
        let mut card = json!({
            "name": "main",
            "supportedInterfaces": [
                {"url": "http://127.0.0.1:7777/a2a/agents/main/jsonrpc", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
                {"url": "https://mac.local/a2a/agents/main/rest", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"},
                {"url": "http://10.0.0.2/elsewhere", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
                {"url": "http://10.0.0.2/a2a/agents/other/rest", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"},
                {"url": "grpc://10.0.0.2:50051", "protocolBinding": "GRPC", "protocolVersion": "1.0"},
            ],
            "capabilities": {"streaming": true},
        });
        rewrite(&mut card, "https://a2a.example", "mac", "main");
        let urls: Vec<&str> = card["supportedInterfaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["url"].as_str().unwrap())
            .collect();
        assert_eq!(
            urls,
            [
                "https://a2a.example/a2a/mac/agents/main/jsonrpc",
                "https://a2a.example/a2a/mac/agents/main/rest",
                "https://a2a.example/a2a/mac/agents/main/jsonrpc",
                "https://a2a.example/a2a/mac/agents/main/rest",
            ],
            "the entry's own kept, others replaced by their binding's, an unknown binding left out"
        );
        assert_eq!(card["capabilities"], json!({"streaming": true}), "the rest untouched");
        let mut old = json!({"url": "http://127.0.0.1:7777/a2a/agents/main/jsonrpc", "preferredTransport": "JSONRPC"});
        rewrite(&mut old, "http://proxy:8080", "host-1", "main");
        assert_eq!(old["url"], "http://proxy:8080/a2a/host-1/agents/main/jsonrpc");
    }
}
