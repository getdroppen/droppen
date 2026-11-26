//! The proxy's configuration, from the environment only:
//!
//! | variable | meaning |
//! |---|---|
//! | `DROPPEN_DATA_DIR` | required: a persistent volume — the proxy's identity and its verified chain (`identity.json`, `chain.json`, `0600`) |
//! | `DROPPEN_HUB` | the hub (default `https://api.droppen.app`, Droppen's); a bare host means `https://` |
//! | `DROPPEN_SESSION_PROXY_TOKEN` | required: the Bearer token the backend presents (at least 32 characters; `openssl rand -hex 32`) |
//! | `PORT` | where to listen (default 8080, on every address) |
//! | `DROPPEN_ENROLL_CODE` | the first start only: the `RH1-` code from my.droppen.app that joins the proxy to an account ([`crate::enroll`]); on this service alone, and removed once it has joined |
//! | `DROPPEN_SESSION_PROXY_RELAY_INSECURE` | `1`: relays on loopback, over plain `ws://` too (local testing only, as the agent's `DROPPEN_AGENT_RELAY_INSECURE`) |
//! | `RUST_LOG` | the log filter (default `info`) |
//!
//! Every value is trimmed, and an empty one is unset. The token is never
//! logged, nor shown by `Debug`.

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
    /// Relays on loopback over plain `ws://` too: local testing only (a
    /// self-run relay). Production relays are public `wss://` names.
    pub relay_insecure: bool,
    /// The first start's enrollment code.
    pub enroll_code: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("data_dir", &self.data_dir)
            .field("hub", &self.hub)
            .field("bind", &self.bind)
            .field("relay_insecure", &self.relay_insecure)
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
        let token = Zeroizing::new(
            var("DROPPEN_SESSION_PROXY_TOKEN").ok_or("DROPPEN_SESSION_PROXY_TOKEN is required: the Bearer token the backend presents")?,
        );
        if token.len() < MIN_TOKEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(format!(
                "DROPPEN_SESSION_PROXY_TOKEN must be at least {MIN_TOKEN} printable characters with no spaces (e.g. `openssl rand -hex 32`)"
            ));
        }
        let port = port(var("PORT"))?;
        let relay_insecure = var("DROPPEN_SESSION_PROXY_RELAY_INSECURE").as_deref() == Some("1");
        Ok(Config {
            data_dir,
            hub,
            token,
            bind: SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port),
            relay_insecure,
            enroll_code: read("DROPPEN_ENROLL_CODE")
                .map(|c| Zeroizing::new(c.trim().to_string()))
                .filter(|c| !c.is_empty()),
        })
    }
}

/// The port the proxy listens on, from this process' `PORT` (default 8080):
/// what its health check dials.
pub fn port_from_env() -> Result<u16, String> {
    port(std::env::var("PORT").ok().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()))
}

fn port(value: Option<String>) -> Result<u16, String> {
    match value {
        Some(p) => p.parse::<u16>().map_err(|_| format!("PORT is not a port: {p}")),
        None => Ok(DEFAULT_PORT),
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
        let c = config(&[("DROPPEN_DATA_DIR", "/data"), ("DROPPEN_SESSION_PROXY_TOKEN", TOKEN)]).unwrap();
        assert_eq!(c.data_dir, PathBuf::from("/data"));
        assert_eq!(c.hub, "https://api.droppen.app");
        assert_eq!(c.bind.port(), 8080);
        assert!(c.enroll_code.is_none());
        assert!(!c.relay_insecure, "public relays only by default");
        assert!(!format!("{c:?}").contains(TOKEN), "no token in Debug");

        let c = config(&[
            ("DROPPEN_DATA_DIR", " /data "),
            ("DROPPEN_SESSION_PROXY_TOKEN", TOKEN),
            ("DROPPEN_HUB", "hub.example:8443"),
            ("PORT", "9000"),
            ("DROPPEN_SESSION_PROXY_RELAY_INSECURE", "1"),
            ("DROPPEN_ENROLL_CODE", " RH1-x "),
        ])
        .unwrap();
        assert_eq!(c.hub, "https://hub.example:8443");
        assert!(c.relay_insecure);
        assert_eq!(c.bind.port(), 9000);
        assert!(c.enroll_code.is_some());
        assert!(!format!("{c:?}").contains("RH1-x"), "no code in Debug");

        for (vars, says) in [
            (vec![("DROPPEN_SESSION_PROXY_TOKEN", TOKEN)], "DROPPEN_DATA_DIR"),
            (vec![("DROPPEN_DATA_DIR", "/d")], "DROPPEN_SESSION_PROXY_TOKEN is required"),
            (vec![("DROPPEN_DATA_DIR", "/d"), ("DROPPEN_SESSION_PROXY_TOKEN", "short")], "at least 32"),
            (vec![("DROPPEN_DATA_DIR", "/d"), ("DROPPEN_SESSION_PROXY_TOKEN", "0123456789abcdef 0123456789abcdef")], "printable"), // gitleaks:allow
            (
                vec![("DROPPEN_DATA_DIR", "/d"), ("DROPPEN_SESSION_PROXY_TOKEN", TOKEN), ("PORT", "x")],
                "PORT",
            ),
            (
                vec![("DROPPEN_DATA_DIR", "/d"), ("DROPPEN_SESSION_PROXY_TOKEN", TOKEN), ("DROPPEN_HUB", "ftp://x")],
                "DROPPEN_HUB",
            ),
        ] {
            let error = config(&vars).unwrap_err();
            assert!(error.contains(says), "{vars:?}: {error}");
        }
    }

    #[test]
    fn a_blank_value_is_unset() {
        let c = config(&[
            ("DROPPEN_DATA_DIR", "/d"),
            ("DROPPEN_SESSION_PROXY_TOKEN", TOKEN),
            ("DROPPEN_ENROLL_CODE", "  "),
            ("DROPPEN_SESSION_PROXY_RELAY_INSECURE", " "),
        ])
        .unwrap();
        assert!(c.enroll_code.is_none());
        assert!(!c.relay_insecure);
    }
}
