//! The proxy's configuration, from the environment only (`docs/a2a.md` §7.4):
//!
//! | variable | meaning |
//! |---|---|
//! | `DROPPEN_DATA_DIR` | required: a persistent volume — the proxy's identity and its verified chain (`identity.json`, `chain.json`, `0600`) |
//! | `DROPPEN_HUB` | the hub (default `https://api.droppen.app`, Droppen's); a bare host means `https://` |
//! | `DROPPEN_A2A_PROXY_TOKEN` | required: the Bearer token A2A clients present (at least 32 characters; `openssl rand -hex 32`) |
//! | `PORT` | where to listen (default 8080, on every address) |
//! | `DROPPEN_ENROLL_CODE` | the first start only: the code that joins the proxy to an account (`RH1`, `kind: "a2a"`) — not in this version yet |
//! | `DROPPEN_A2A_PROXY_BASE` | optional: the proxy's own base as clients reach it (`https://a2a.example`), for the Agent Card's addresses; unset, the request's `Host` over `http` |
//! | `DROPPEN_A2A_PROXY_ALIASES` | optional: `name=device,…`, names to reach devices by in `/a2a/<device>/…` besides their ids |
//! | `RUST_LOG` | the log filter (default `info`) |
//!
//! Every value is trimmed, and an empty one is unset. The token is never
//! logged, nor shown by `Debug`.

use std::collections::BTreeMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use zeroize::Zeroizing;

/// Droppen's hub.
pub const DEFAULT_HUB: &str = "https://api.droppen.app";
/// The shortest proxy token taken.
pub const MIN_TOKEN: usize = 32;
const DEFAULT_PORT: u16 = 8080;

pub struct Config {
    pub data_dir: PathBuf,
    /// `https://…` (or `http://` to loopback, for local testing).
    pub hub: String,
    pub token: Zeroizing<String>,
    pub bind: SocketAddr,
    /// The proxy's own base for the Agent Card (`https://a2a.example`, no
    /// trailing `/`); `None`: the request's `Host` over `http`.
    pub public_base: Option<String>,
    /// Name → device id.
    pub aliases: BTreeMap<String, String>,
    /// The first start's enrollment code.
    pub enroll_code: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("data_dir", &self.data_dir)
            .field("hub", &self.hub)
            .field("bind", &self.bind)
            .field("public_base", &self.public_base)
            .field("aliases", &self.aliases)
            .field("enroll_code", &self.enroll_code.as_ref().map(|_| "(set)"))
            .finish_non_exhaustive()
    }
}

impl Config {
    /// From this process' environment.
    pub fn from_env() -> Result<Config, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// From `read` (a variable's value, if set).
    pub fn from_vars(read: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        let var = |name: &str| read(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let data_dir = var("DROPPEN_DATA_DIR")
            .map(PathBuf::from)
            .ok_or("DROPPEN_DATA_DIR is required: a persistent volume for the proxy's identity and its device chain")?;
        let hub = hub_url(&var("DROPPEN_HUB").unwrap_or_else(|| DEFAULT_HUB.into()))?;
        let token =
            Zeroizing::new(var("DROPPEN_A2A_PROXY_TOKEN").ok_or("DROPPEN_A2A_PROXY_TOKEN is required: the Bearer token A2A clients present")?);
        if token.len() < MIN_TOKEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(format!(
                "DROPPEN_A2A_PROXY_TOKEN must be at least {MIN_TOKEN} printable characters with no spaces (e.g. `openssl rand -hex 32`)"
            ));
        }
        let port = match var("PORT") {
            Some(p) => p.parse::<u16>().map_err(|_| format!("PORT is not a port: {p}"))?,
            None => DEFAULT_PORT,
        };
        let public_base = var("DROPPEN_A2A_PROXY_BASE").map(|b| base_url(&b)).transpose()?;
        let aliases = var("DROPPEN_A2A_PROXY_ALIASES").map(|a| aliases(&a)).transpose()?.unwrap_or_default();
        Ok(Config {
            data_dir,
            hub,
            token,
            bind: SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port),
            public_base,
            aliases,
            enroll_code: read("DROPPEN_ENROLL_CODE")
                .map(|c| Zeroizing::new(c.trim().to_string()))
                .filter(|c| !c.is_empty()),
        })
    }
}

/// `https://host[:port]` from what `DROPPEN_HUB` says (a bare host is https).
fn hub_url(text: &str) -> Result<String, String> {
    let url = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let parsed = droppen_client::HubUrl::parse(&url).map_err(|e| format!("DROPPEN_HUB: {e}"))?;
    Ok(parsed.http)
}

/// A base the card's addresses start with: `http(s)://host[:port]` and an
/// optional path, no trailing `/`, query or fragment.
fn base_url(text: &str) -> Result<String, String> {
    let bad = || format!("DROPPEN_A2A_PROXY_BASE is not an http(s) base: {text}");
    let (scheme, rest) = text.split_once("://").ok_or_else(bad)?;
    if !matches!(scheme, "http" | "https") || rest.is_empty() || rest.contains(['?', '#', '@']) || !text.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(bad());
    }
    Ok(text.trim_end_matches('/').to_string())
}

/// `name=device,…`: names of letters, digits, `-`, `_`, `.`; ids as the
/// chain has them.
fn aliases(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for pair in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, device) = pair
            .split_once('=')
            .map(|(n, d)| (n.trim(), d.trim()))
            .ok_or_else(|| format!("DROPPEN_A2A_PROXY_ALIASES: `{pair}` is not name=device"))?;
        if !crate::route::segment_ok(name) || !crate::route::segment_ok(device) {
            return Err(format!(
                "DROPPEN_A2A_PROXY_ALIASES: `{pair}`: names and ids are letters, digits, `-`, `_`, `.` and `~`"
            ));
        }
        if out.insert(name.to_string(), device.to_string()).is_some() {
            return Err(format!("DROPPEN_A2A_PROXY_ALIASES: `{name}` named twice"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<Config, String> {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_vars(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn the_environment_sets_the_proxy_up_and_what_it_must_have_is_required() {
        let c = config(&[("DROPPEN_DATA_DIR", "/data"), ("DROPPEN_A2A_PROXY_TOKEN", TOKEN)]).unwrap();
        assert_eq!(c.data_dir, PathBuf::from("/data"));
        assert_eq!(c.hub, "https://api.droppen.app");
        assert_eq!(c.bind.port(), 8080);
        assert!(c.public_base.is_none() && c.aliases.is_empty() && c.enroll_code.is_none());
        assert!(!format!("{c:?}").contains(TOKEN), "no token in Debug");

        let c = config(&[
            ("DROPPEN_DATA_DIR", " /data "),
            ("DROPPEN_A2A_PROXY_TOKEN", TOKEN),
            ("DROPPEN_HUB", "hub.example:8443"),
            ("PORT", "9000"),
            ("DROPPEN_A2A_PROXY_BASE", "https://a2a.example/"),
            ("DROPPEN_A2A_PROXY_ALIASES", "mac=host-1, linux = host-2"),
            ("DROPPEN_ENROLL_CODE", "RH1-x"),
        ])
        .unwrap();
        assert_eq!(c.hub, "https://hub.example:8443");
        assert_eq!(c.bind.port(), 9000);
        assert_eq!(c.public_base.as_deref(), Some("https://a2a.example"));
        assert_eq!(c.aliases.get("linux").map(String::as_str), Some("host-2"));
        assert!(c.enroll_code.is_some());

        for (vars, says) in [
            (vec![("DROPPEN_A2A_PROXY_TOKEN", TOKEN)], "DROPPEN_DATA_DIR"),
            (vec![("DROPPEN_DATA_DIR", "/d")], "DROPPEN_A2A_PROXY_TOKEN is required"),
            (vec![("DROPPEN_DATA_DIR", "/d"), ("DROPPEN_A2A_PROXY_TOKEN", "short")], "at least 32"),
            (
                vec![("DROPPEN_DATA_DIR", "/d"), ("DROPPEN_A2A_PROXY_TOKEN", TOKEN), ("PORT", "x")],
                "PORT",
            ),
            (
                vec![
                    ("DROPPEN_DATA_DIR", "/d"),
                    ("DROPPEN_A2A_PROXY_TOKEN", TOKEN),
                    ("DROPPEN_A2A_PROXY_BASE", "ftp://x"),
                ],
                "BASE",
            ),
            (
                vec![
                    ("DROPPEN_DATA_DIR", "/d"),
                    ("DROPPEN_A2A_PROXY_TOKEN", TOKEN),
                    ("DROPPEN_A2A_PROXY_ALIASES", "a=b/c"),
                ],
                "ALIASES",
            ),
            (
                vec![("DROPPEN_DATA_DIR", "/d"), ("DROPPEN_A2A_PROXY_TOKEN", TOKEN), ("DROPPEN_HUB", "ftp://x")],
                "DROPPEN_HUB",
            ),
        ] {
            let error = config(&vars).unwrap_err();
            assert!(error.contains(says), "{vars:?}: {error}");
        }
    }
}
