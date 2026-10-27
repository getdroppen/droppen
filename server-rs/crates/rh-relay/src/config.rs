//! A relay's configuration, from the environment only (§2.5, §2.11, §2.13).
//!
//! A relay's variables are `DROPPEN_RELAY_*`. Before the rename to Droppen
//! they were `RH_RELAY_*`; until v0.2.135 each old name is still read when
//! its new name is unset, and the relay logs once per old name that it is
//! deprecated and stops working in v0.2.135 (or, when both are set to
//! different values, that the new one is used). See [`env`].
//!
//! | variable | meaning |
//! |---|---|
//! | `DROPPEN_RELAY_HUB` | the Hub, e.g. `https://api.droppen.app` (unset: no control link — tickets still verify under `DROPPEN_RELAY_TICKET_KEYS`) |
//! | `DROPPEN_RELAY_ORIGIN` | this relay's public origin as clients dial it, e.g. `wss://sin-1.relay.droppen.app` (required); every sign-in here names it |
//! | `DROPPEN_RELAY_KEY` | the relay's Ed25519 seed (64 hex, a secret; `droppen-relay keygen` makes one). Unset: `DROPPEN_RELAY_STATE_DIR/relay.key`, made on first start (that needs a persistent volume) |
//! | `DROPPEN_RELAY_STATE_DIR` | where the key (and the Hub's ticket keys, for a restart while the Hub is down) are kept when `DROPPEN_RELAY_KEY` is not set |
//! | `DROPPEN_RELAY_REGISTRATION_TOKEN` | the operator's one-time token, needed for the first start only; it also names the relay id |
//! | `DROPPEN_RELAY_ID` | the relay id (else read from the token) |
//! | `DROPPEN_RELAY_TICKET_KEYS` | `kid:hex,…`: pin the Hub's ticket keys instead of taking them from the control link |
//! | `PORT`, `DROPPEN_RELAY_BIND` | where to listen (default `[::]:8080`) |
//! | `DROPPEN_RELAY_TRUST_PROXY` | `fly` (`Fly-Client-IP`), `railway` (`X-Real-IP`) or unset (the socket's address): where a client's address comes from |
//! | `DROPPEN_RELAY_MAX_CONNS` | connections this relay holds at most (default 20,000) |
//! | `DROPPEN_RELAY_DRAIN_SECS` | how long `goaway` gives clients before the relay stops (default 30) |
//! | `RUST_LOG` | log filter (default `info`) |

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

/// Where a client's real address comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustProxy {
    None,
    /// `Fly-Client-IP`.
    Fly,
    /// `X-Real-IP` (Railway's edge).
    Railway,
}

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    /// `wss://…` (or `ws://` on loopback, for tests).
    pub origin: String,
    /// The Hub's origin as this relay dials it (`wss://hub`), if it has one.
    pub hub: Option<String>,
    pub id: String,
    pub key: SigningKey,
    pub registration_token: Option<String>,
    /// Ticket keys pinned by the operator (`kid → key`); the Hub's are ignored then.
    pub pinned_keys: Option<Vec<(u8, [u8; 32])>>,
    pub state_dir: Option<PathBuf>,
    pub trust_proxy: TrustProxy,
    pub max_conns: usize,
    pub drain: Duration,
    pub timing: Timing,
}

/// The clocks a relay runs on (shortened in tests).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// A peer has this long to sign in.
    pub login_timeout: Duration,
    /// A sign-in's clock may be this far from the relay's.
    pub login_skew_ms: u64,
    /// A ticket that ran out this long ago closes the connection (4419).
    pub renew_grace_ms: u64,
    /// The control link down this long: established connections outlive
    /// their tickets, by up to [`Timing::outage_grace_ms`].
    pub outage_after: Duration,
    pub outage_grace_ms: u64,
    /// Silence from a peer after which it is gone.
    pub silence: Duration,
    /// How often usage is reported, and how long reports wait for the Hub.
    pub usage_every: Duration,
    pub usage_keep: Duration,
    pub health_every: Duration,
    /// The longest wait between attempts to reach the Hub.
    pub uplink_backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            login_timeout: Duration::from_secs(10),
            login_skew_ms: 60_000,
            renew_grace_ms: 60_000,
            outage_after: Duration::from_secs(120),
            outage_grace_ms: 2 * 60 * 60 * 1000,
            silence: Duration::from_secs(90),
            usage_every: Duration::from_secs(60),
            usage_keep: Duration::from_secs(2 * 60 * 60),
            health_every: Duration::from_secs(20),
            uplink_backoff_max: Duration::from_secs(30),
        }
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("origin", &self.origin)
            .field("hub", &self.hub)
            .field("id", &self.id)
            .field("pinned_keys", &self.pinned_keys.as_ref().map(Vec::len))
            .field("trust_proxy", &self.trust_proxy)
            .field("max_conns", &self.max_conns)
            .finish_non_exhaustive()
    }
}

/// The variable `name`, trimmed (empty is unset). A `DROPPEN_RELAY_*` name
/// falls back to its old name ([`old_name`]) when unset, so a relay
/// configured before the rename keeps working until v0.2.135; using the old
/// name, or setting both to different values (the new one wins), is logged
/// once per name.
pub fn env(name: &str) -> Option<String> {
    let (value, old) = lookup(name, |n| std::env::var(n).ok());
    if let Some(line) = old.and_then(|old| notice(&NOTICED, name, &old)) {
        tracing::warn!("{line}");
    }
    value
}

/// The old names already logged in this process.
static NOTICED: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// The name a relay variable had before the rename to Droppen:
/// `DROPPEN_RELAY_X` → `RH_RELAY_X`, and the shared capture's
/// `DROPPEN_HUB_API_TRAFFIC` → `RH_HUB_API_TRAFFIC`; `None` for any other
/// name.
pub fn old_name(name: &str) -> Option<String> {
    if name == "DROPPEN_HUB_API_TRAFFIC" {
        // The hub-API capture (`crate::traffic`), named like the other
        // binaries that write to it.
        return Some("RH_HUB_API_TRAFFIC".into());
    }
    name.strip_prefix("DROPPEN_RELAY_").map(|rest| format!("RH_RELAY_{rest}"))
}

/// What a renamed variable's old name did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OldName {
    /// Only the old name is set: its value is used.
    Used(String),
    /// Both are set, to different values: the new name's is used.
    Overridden(String),
}

/// `name`'s value as [`env`] reads it through `read`, and what its old name
/// did. A new name set to the old one's value says nothing: that is the
/// switch-over, both set until the old one is removed.
pub fn lookup(name: &str, read: impl Fn(&str) -> Option<String>) -> (Option<String>, Option<OldName>) {
    let get = |n: &str| read(n).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let value = get(name);
    let Some(old) = old_name(name) else { return (value, None) };
    match (value, get(&old)) {
        (None, Some(was)) => (Some(was), Some(OldName::Used(old))),
        (Some(value), Some(was)) if was != value => (Some(value), Some(OldName::Overridden(old))),
        (value, _) => (value, None),
    }
}

/// The line to log about `name`'s old name, the first time it comes up
/// (`noticed` remembers); `None` after.
fn notice(noticed: &std::sync::Mutex<std::collections::BTreeSet<String>>, name: &str, old: &OldName) -> Option<String> {
    let (OldName::Used(was) | OldName::Overridden(was)) = old;
    if !noticed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(was.clone()) {
        return None;
    }
    Some(match old {
        OldName::Used(_) => format!("{was} is deprecated: add {name} with the same value; {was} stops working in v0.2.135"),
        OldName::Overridden(_) => format!(
            "{name} and the deprecated {was} are both set, to different values: {name} is used; remove {was} (it stops working in v0.2.135)"
        ),
    })
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// `wss://Host:443/x` → `wss://host`; `https` becomes `wss`. Plain `ws://` is
/// accepted only for loopback hosts.
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "wss" | "https" => true,
        "ws" | "http" => false,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = authority.rsplit_once(':').map_or(authority.as_str(), |(h, _)| h);
    if !secure && !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    let authority = match (secure, authority.rsplit_once(':')) {
        (true, Some((h, "443"))) | (false, Some((h, "80"))) => h.to_string(),
        _ => authority,
    };
    Some(format!("{}://{authority}", if secure { "wss" } else { "ws" }))
}

/// `wss://x` → `https://x` (`ws://` → `http://`).
pub fn http_of(origin: &str) -> String {
    match origin.split_once("://") {
        Some(("ws", rest)) => format!("http://{rest}"),
        Some((_, rest)) => format!("https://{rest}"),
        None => origin.to_string(),
    }
}

/// `kid:hex,kid:hex`.
pub fn parse_keys(text: &str) -> Option<Vec<(u8, [u8; 32])>> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (kid, key) = pair.split_once(':')?;
            Some((kid.trim().parse().ok()?, unhex(key.trim())?.try_into().ok()?))
        })
        .collect()
}

/// The relay's key: `DROPPEN_RELAY_KEY`, else the state directory's `relay.key`
/// (made on first start, mode 0600).
pub fn load_key(state_dir: Option<&std::path::Path>) -> Result<SigningKey, String> {
    if let Some(seed) = env("DROPPEN_RELAY_KEY") {
        let seed = Zeroizing::new(unhex(&seed).ok_or("DROPPEN_RELAY_KEY is 64 hex characters")?);
        let seed: [u8; 32] = seed.as_slice().try_into().map_err(|_| "DROPPEN_RELAY_KEY is 32 bytes")?;
        return Ok(SigningKey::from_bytes(&seed));
    }
    let dir = state_dir.ok_or("set DROPPEN_RELAY_KEY (`droppen-relay keygen` makes one) or DROPPEN_RELAY_STATE_DIR on a persistent volume")?;
    let path = dir.join("relay.key");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let seed = Zeroizing::new(unhex(text.trim()).ok_or_else(|| format!("{}: not a key", path.display()))?);
            let seed: [u8; 32] = seed.as_slice().try_into().map_err(|_| format!("{}: not a key", path.display()))?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SigningKey::generate(&mut rand::rngs::OsRng);
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            write_private(&path, &hex(key.as_bytes()))?;
            tracing::info!(path = %path.display(), "made this relay's key");
            Ok(key)
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write a file only its owner may read.
pub fn write_private(path: &std::path::Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// The relay id a registration token names (`RHR1-<base32(len ‖ id ‖ secret)>`,
/// or the same with `DRR1-`; either prefix exactly as written).
pub fn token_relay_id(token: &str) -> Option<String> {
    let token = token.trim();
    let raw = rh_proto::codec::base32::decode(token.strip_prefix("RHR1-").or_else(|| token.strip_prefix("DRR1-"))?)?;
    let len = usize::from(*raw.first()?);
    let id = std::str::from_utf8(raw.get(1..1 + len)?).ok()?;
    (raw.len() == 1 + len + 20).then(|| id.to_string())
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let port: u16 = match env("PORT") {
            Some(p) => p.parse().map_err(|_| "PORT: a port number")?,
            None => 8080,
        };
        let ip: IpAddr = match env("DROPPEN_RELAY_BIND") {
            Some(b) => b.parse().map_err(|_| "DROPPEN_RELAY_BIND: an IP address")?,
            None => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };
        let origin = env("DROPPEN_RELAY_ORIGIN").ok_or("DROPPEN_RELAY_ORIGIN is required (e.g. wss://sin-1.relay.example)")?;
        let origin = self::origin(&origin).ok_or("DROPPEN_RELAY_ORIGIN: a wss:// origin (ws:// only on loopback)")?;
        let hub = match env("DROPPEN_RELAY_HUB") {
            Some(h) => Some(self::origin(&h).ok_or("DROPPEN_RELAY_HUB: an https:// (or wss://) origin")?),
            None => None,
        };
        let registration_token = env("DROPPEN_RELAY_REGISTRATION_TOKEN");
        let id = match (env("DROPPEN_RELAY_ID"), registration_token.as_deref().and_then(token_relay_id)) {
            (Some(id), _) | (None, Some(id)) => id,
            (None, None) => return Err("set DROPPEN_RELAY_ID (or DROPPEN_RELAY_REGISTRATION_TOKEN, which names it)".into()),
        };
        let pinned_keys = match env("DROPPEN_RELAY_TICKET_KEYS") {
            Some(k) => Some(
                parse_keys(&k)
                    .filter(|k| !k.is_empty())
                    .ok_or("DROPPEN_RELAY_TICKET_KEYS: kid:hex[,kid:hex…]")?,
            ),
            None => None,
        };
        if hub.is_none() && pinned_keys.is_none() {
            return Err("set DROPPEN_RELAY_HUB (or pin DROPPEN_RELAY_TICKET_KEYS to run without the Hub)".into());
        }
        let state_dir = env("DROPPEN_RELAY_STATE_DIR").map(PathBuf::from);
        let key = load_key(state_dir.as_deref())?;
        let trust_proxy = match env("DROPPEN_RELAY_TRUST_PROXY").as_deref() {
            None | Some("none") => TrustProxy::None,
            Some("fly") => TrustProxy::Fly,
            Some("railway") => TrustProxy::Railway,
            Some(other) => return Err(format!("DROPPEN_RELAY_TRUST_PROXY: {other:?} is fly, railway or none")),
        };
        Ok(Self {
            bind: SocketAddr::new(ip, port),
            origin,
            hub,
            id,
            key,
            registration_token,
            pinned_keys,
            state_dir,
            trust_proxy,
            max_conns: match env("DROPPEN_RELAY_MAX_CONNS") {
                Some(n) => n.parse().map_err(|_| "DROPPEN_RELAY_MAX_CONNS: a number")?,
                None => 20_000,
            },
            drain: Duration::from_secs(match env("DROPPEN_RELAY_DRAIN_SECS") {
                Some(n) => n.parse().map_err(|_| "DROPPEN_RELAY_DRAIN_SECS: a number")?,
                None => 30,
            }),
            timing: Timing::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_secure_unless_on_loopback() {
        assert_eq!(origin("https://Sin-1.Relay.Example:443/x").as_deref(), Some("wss://sin-1.relay.example"));
        assert_eq!(origin("ws://127.0.0.1:9000").as_deref(), Some("ws://127.0.0.1:9000"));
        assert_eq!(origin("ws://relay.example"), None);
        assert_eq!(http_of("wss://a.example"), "https://a.example");
        assert_eq!(http_of("ws://127.0.0.1:1"), "http://127.0.0.1:1");
    }

    #[test]
    fn a_registration_token_names_its_relay_with_either_prefix() {
        let mut raw = vec![5];
        raw.extend_from_slice(b"sin-1");
        raw.extend_from_slice(&[9; 20]);
        let body = rh_proto::codec::base32::encode(&raw);
        for prefix in ["RHR1-", "DRR1-"] {
            assert_eq!(token_relay_id(&format!(" {prefix}{body}\n")).as_deref(), Some("sin-1"), "{prefix}");
            assert_eq!(
                token_relay_id(&format!("{prefix}{}", &body[..body.len() - 2])),
                None,
                "{prefix} cut short"
            );
        }
        for other in ["rhr1-", "drr1-", "RHR2-", "DRR2-", "RHP1-", "DRP1-", "RH1-", "DR1-", "DRR1-RHR1-", ""] {
            assert_eq!(token_relay_id(&format!("{other}{body}")), None, "{other}");
        }
    }

    #[test]
    fn pinned_keys_parse() {
        let k = parse_keys(&format!("1:{},2:{}", "aa".repeat(32), "bb".repeat(32))).unwrap();
        assert_eq!(k.len(), 2);
        assert_eq!(k[1].0, 2);
        assert!(parse_keys("1:zz").is_none());
    }

    /// [`lookup`] over just `vars`.
    fn lookup_in(vars: &[(&str, &str)], name: &str) -> (Option<String>, Option<OldName>) {
        let vars: std::collections::HashMap<&str, &str> = vars.iter().copied().collect();
        lookup(name, |n| vars.get(n).map(|v| v.to_string()))
    }

    #[test]
    fn a_relay_variable_was_rh_relay_before_the_rename() {
        assert_eq!(old_name("DROPPEN_RELAY_KEY").as_deref(), Some("RH_RELAY_KEY"));
        assert_eq!(old_name("DROPPEN_RELAY_REGISTRATION_TOKEN").as_deref(), Some("RH_RELAY_REGISTRATION_TOKEN"));
        assert_eq!(old_name("PORT"), None);
        assert_eq!(old_name("DROPPEN_HUB_ORIGIN"), None, "a hub variable is not the relay's");
        assert_eq!(old_name("DROPPEN_HUB_API_TRAFFIC").as_deref(), Some("RH_HUB_API_TRAFFIC"), "the capture all binaries share");
    }

    #[test]
    fn the_new_name_wins_over_the_old_one() {
        let both = [("DROPPEN_RELAY_ID", "sin-2"), ("RH_RELAY_ID", "sin-1")];
        assert_eq!(
            lookup_in(&both, "DROPPEN_RELAY_ID"),
            (Some("sin-2".into()), Some(OldName::Overridden("RH_RELAY_ID".into())))
        );
        // The switch-over: both set to one value, nothing to say.
        let same = [("DROPPEN_RELAY_ID", "sin-1"), ("RH_RELAY_ID", " sin-1 ")];
        assert_eq!(lookup_in(&same, "DROPPEN_RELAY_ID"), (Some("sin-1".into()), None));
        assert_eq!(lookup_in(&[("DROPPEN_RELAY_ID", "sin-1")], "DROPPEN_RELAY_ID"), (Some("sin-1".into()), None));
    }

    #[test]
    fn the_old_name_still_works() {
        assert_eq!(
            lookup_in(&[("RH_RELAY_ID", " sin-1\n")], "DROPPEN_RELAY_ID"),
            (Some("sin-1".into()), Some(OldName::Used("RH_RELAY_ID".into())))
        );
        // An empty new name is unset, as an empty variable always was.
        assert_eq!(
            lookup_in(&[("DROPPEN_RELAY_ID", " "), ("RH_RELAY_ID", "sin-1")], "DROPPEN_RELAY_ID"),
            (Some("sin-1".into()), Some(OldName::Used("RH_RELAY_ID".into())))
        );
    }

    #[test]
    fn with_neither_name_set_it_is_unset() {
        assert_eq!(lookup_in(&[], "DROPPEN_RELAY_MAX_CONNS"), (None, None));
        assert_eq!(lookup_in(&[("RH_RELAY_MAX_CONNS", ""), ("DROPPEN_RELAY_MAX_CONNS", "")], "DROPPEN_RELAY_MAX_CONNS"), (None, None));
        // Names that were never renamed have nothing to fall back to.
        assert_eq!(lookup_in(&[("PORT", "9000")], "PORT"), (Some("9000".into()), None));
    }

    #[test]
    fn each_old_name_is_logged_once() {
        let noticed = std::sync::Mutex::new(std::collections::BTreeSet::new());
        let used = OldName::Used("RH_RELAY_KEY".into());
        let line = notice(&noticed, "DROPPEN_RELAY_KEY", &used).expect("the first time");
        assert_eq!(
            line,
            "RH_RELAY_KEY is deprecated: add DROPPEN_RELAY_KEY with the same value; RH_RELAY_KEY stops working in v0.2.135"
        );
        assert_eq!(notice(&noticed, "DROPPEN_RELAY_KEY", &used), None, "once");
        let line = notice(&noticed, "DROPPEN_RELAY_ID", &OldName::Overridden("RH_RELAY_ID".into())).expect("another name");
        assert_eq!(
            line,
            "DROPPEN_RELAY_ID and the deprecated RH_RELAY_ID are both set, to different values: DROPPEN_RELAY_ID is used; remove RH_RELAY_ID (it stops working in v0.2.135)"
        );
        assert_eq!(notice(&noticed, "DROPPEN_RELAY_ID", &OldName::Overridden("RH_RELAY_ID".into())), None);
    }
}
