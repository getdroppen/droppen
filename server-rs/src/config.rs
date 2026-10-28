//! Application configuration.
//!
//! gotcha reads `configurations/application.toml` plus the `APP_` environment
//! prefix through `mofa`, with `__` separating nested sections
//! (`gotcha-0.4.1/src/config.rs:222`). The framework's own bind address lives in
//! the reserved `[server]` section, so:
//!
//! ```toml
//! # configurations/application.toml
//! home_dir = "/Users/me"
//!
//! [server]
//! host = "127.0.0.1"
//! port = 8700
//! ```
//!
//! and `APP_SERVER__PORT=9000` overrides the port. remote-hand's own settings sit
//! at the top level next to `home_dir` — `instance = "staging"`, or
//! `APP_INSTANCE=staging` — rather than in `[server]`, which is the framework's
//! own struct and cannot grow a field of ours.
//!
//! The loader is assembled here rather than through
//! [`GotchaConfigLoader::load`](gotcha::GotchaConfigLoader::load), for one
//! reason: **`APP_SERVER__PORT` does not work under the stock loader**. mofa's
//! environment source can only produce strings, and its merge coerces a string
//! into a number only when an earlier source already put a number at that path
//! (`mofa-0.2.0/src/utils.rs:122`). gotcha's loader has no defaults source, so
//! on a machine with no `configurations/application.toml` the port arrives at
//! serde as `Value::String("9000")` and the entire load fails with
//! `invalid type: string "9000", expected u16`.
//!
//! Putting a typed [`DefaultsSource`] in front of the file and environment
//! sources fixes that and removes the need for a config file at all — which is
//! also how the contract's `127.0.0.1:8700` default gets expressed, instead of
//! fixing up gotcha's `3000` afterwards (a fix-up that cannot tell whether a
//! port was left untouched or deliberately set to `3000`).
//!
//! The trait API is used rather than the `Gotcha` builder because the builder
//! swallows a configuration error and silently falls back to `Default`
//! (`gotcha-0.4.1/src/builder.rs`), which would hide a typo'd config file.

use std::path::Path;

use gotcha::config::ConfigError;
use gotcha::{config, ConfigWrapper};
// `config` is the `#[config]` attribute macro that makes `AppConfig` extractable
// as `State<AppConfig>`; it is applied below.
use mofa::toml::{Table, Value};
use mofa::{ConfigLoader, EnvironmentSource, FileSource, Source};
use serde::{Deserialize, Serialize};

/// remote-hand's own settings, read from the top level of the config file.
#[config]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    /// Home directory to discover sessions under. Defaults to `$HOME`.
    ///
    /// This exists mainly so the integration test can point the server at a
    /// synthetic `~/.claude` tree without touching the real one.
    #[serde(default)]
    pub home_dir: Option<String>,
    /// Which instance this server owns; defaults to `default`.
    ///
    /// Everything adoptable — surviving terminals now, runs and apps later —
    /// lives under `<data>/instances/<instance>/`, and a server takes an
    /// exclusive lock on it at boot (see [`crate::instance`]). Two servers on
    /// one machine must therefore be given different names, which is what this
    /// is for: `APP_INSTANCE=staging`, or `instance = "staging"` at the top
    /// level of `configurations/application.toml`.
    #[serde(default)]
    pub instance: Option<String>,
    /// Local accounts: see [`AuthSettings`].
    ///
    /// A section of its own at the *top* level — `[auth]`, `APP_AUTH__*` — and
    /// deliberately not a few more fields in `[server]`, which is gotcha's own
    /// struct and cannot grow a field of ours.
    #[serde(default)]
    pub auth: AuthSettings,
    /// Plugins the configuration pins on or off: see [`PluginSettings`].
    #[serde(default)]
    pub plugins: PluginSettings,
}

/// `[plugins]` — `enable = "a,b"`, `disable = "c"`, or `APP_PLUGINS__ENABLE` /
/// `APP_PLUGINS__DISABLE`.
///
/// Comma-separated strings rather than arrays because mofa's environment
/// source can only produce strings (see the module docs), and an emergency
/// switch that works in the file but not in the environment is not one. Both
/// carry a typed empty-string default in [`DefaultsSource`]. A plugin named in
/// `disable` is off whatever else says so; one named only in `enable` is on;
/// either way the switch in the settings is read-only for it
/// ([`crate::plugins`]).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginSettings {
    /// Plugin ids to force on.
    #[serde(default)]
    pub enable: Option<String>,
    /// Plugin ids to force off. Beats `enable`.
    #[serde(default)]
    pub disable: Option<String>,
}

impl PluginSettings {
    /// The ids `enable` names, trimmed, blanks dropped.
    pub fn enabled(&self) -> std::collections::BTreeSet<String> {
        split_ids(self.enable.as_deref())
    }

    /// The ids `disable` names, trimmed, blanks dropped.
    pub fn disabled(&self) -> std::collections::BTreeSet<String> {
        split_ids(self.disable.as_deref())
    }
}

fn split_ids(list: Option<&str>) -> std::collections::BTreeSet<String> {
    list.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// What [`crate::auth`] reads out of the configuration.
///
/// Every field is optional and every default is expressed twice: once as the
/// accessor's fallback below, and once — typed — in [`DefaultsSource`]. The
/// second copy is not redundant. mofa's environment source can only produce
/// strings, and its merge coerces one into a number only where an earlier source
/// already put a number (`mofa-0.2.0/src/utils.rs:122`), so without the typed
/// default `APP_AUTH__SESSION_TTL_DAYS=14` would arrive at serde as
/// `Value::String("14")` and fail the entire load. See the module docs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthSettings {
    /// Force authentication on (`true`) or off (`false`).
    ///
    /// Unset means on, the same as `true`: an instance with a user account asks
    /// for a login, one without is in its setup state ([`crate::auth::setup`]).
    /// `false` switches authentication off: development and tests only.
    ///
    /// The one field with no typed default — there is no TOML value meaning
    /// "unset" — so it carries its own lenient deserializer instead, and accepts
    /// the string an environment variable is bound to arrive as.
    #[serde(default, deserialize_with = "lenient_bool")]
    pub enabled: Option<bool>,
    /// How long a login lasts, in days. Absolute: using a token never extends it.
    #[serde(default)]
    pub session_ttl_days: Option<u32>,
    /// Consecutive failed logins that lock an account; `0` disables the lockout.
    #[serde(default)]
    pub lockout_attempts: Option<u32>,
    /// How long that lock holds, in minutes.
    #[serde(default)]
    pub lockout_minutes: Option<u32>,
}

/// The default session lifetime, in days.
pub const DEFAULT_SESSION_TTL_DAYS: u32 = 7;
/// The default number of failures before an account is locked.
pub const DEFAULT_LOCKOUT_ATTEMPTS: u32 = 5;
/// The default lockout duration, in minutes.
pub const DEFAULT_LOCKOUT_MINUTES: u32 = 5;

impl AuthSettings {
    /// The three-state switch, exactly as configured.
    pub fn enabled(&self) -> Option<bool> {
        self.enabled
    }

    /// How long a login lasts, defaulted.
    pub fn session_ttl_days(&self) -> u32 {
        // Zero would mint tokens that are already expired, which is a very
        // confusing way to spell "nobody may log in".
        self.session_ttl_days.filter(|days| *days > 0).unwrap_or(DEFAULT_SESSION_TTL_DAYS)
    }

    /// Failures before a lock, defaulted. `Some(0)` is honoured: it means never.
    pub fn lockout_attempts(&self) -> u32 {
        self.lockout_attempts.unwrap_or(DEFAULT_LOCKOUT_ATTEMPTS)
    }

    /// Lockout duration in minutes, defaulted.
    pub fn lockout_minutes(&self) -> u32 {
        self.lockout_minutes.filter(|minutes| *minutes > 0).unwrap_or(DEFAULT_LOCKOUT_MINUTES)
    }
}

/// Deserialize a boolean that may have arrived as the string an environment
/// variable is.
///
/// The typed-defaults trick the rest of this module uses cannot express "absent"
/// for a three-state flag, so this field meets the environment source where it
/// is. The accepted spellings are mofa's own (`mofa-0.2.0/src/utils.rs:95`), so
/// `APP_AUTH__ENABLED=1` means what it would mean for any other boolean.
fn lenient_bool<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrText {
        Bool(bool),
        Text(String),
    }
    match Option::<BoolOrText>::deserialize(deserializer)? {
        None => Ok(None),
        Some(BoolOrText::Bool(value)) => Ok(Some(value)),
        Some(BoolOrText::Text(raw)) => match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Some(true)),
            "false" | "0" => Ok(Some(false)),
            other => Err(serde::de::Error::custom(format!("auth.enabled is true or false, not {other:?}"))),
        },
    }
}

impl AppConfig {
    /// The home directory to use, falling back to the process environment.
    pub fn home(&self) -> std::path::PathBuf {
        self.home_dir
            .clone()
            .or_else(|| std::env::var("HOME").ok())
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
    }

    /// The instance name to own, defaulted. Validated where it is used, so a
    /// bad name is one clear refusal at boot rather than a silently sanitised
    /// directory somebody later cannot find.
    pub fn instance(&self) -> String {
        // An empty value means "unset", matching how `home_dir` and every
        // `DROPPEN_*` override in `crate::sessions::Paths` read one.
        self.instance
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(crate::instance::DEFAULT_INSTANCE)
            .to_string()
    }
}

/// gotcha's `ServerConfig` defaults to port 3000; remote-hand's contract is 8700 on
/// the loopback interface, which is what `web/vite.config.ts` proxies to.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// The port `web/vite.config.ts` proxies `/api` to.
pub const DEFAULT_PORT: u16 = 8700;

/// The lowest-priority source: remote-hand's defaults, correctly typed.
///
/// Being typed is the point — see the module docs. Every later source (file,
/// then environment) merges on top.
struct DefaultsSource;

impl Source for DefaultsSource {
    fn load(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let mut server = Table::new();
        server.insert("host".into(), Value::String(DEFAULT_HOST.into()));
        server.insert("port".into(), Value::Integer(i64::from(DEFAULT_PORT)));
        // Typed, so `APP_AUTH__SESSION_TTL_DAYS=14` — which arrives as a string,
        // like every environment value — has an integer to coerce against.
        // `auth.enabled` is absent on purpose: it is three-state and carries its
        // own lenient deserializer instead.
        let mut auth = Table::new();
        auth.insert("session_ttl_days".into(), Value::Integer(i64::from(DEFAULT_SESSION_TTL_DAYS)));
        auth.insert("lockout_attempts".into(), Value::Integer(i64::from(DEFAULT_LOCKOUT_ATTEMPTS)));
        auth.insert("lockout_minutes".into(), Value::Integer(i64::from(DEFAULT_LOCKOUT_MINUTES)));
        // Strings already, and typed anyway so the section always exists with
        // the shape `PluginSettings` reads.
        let mut plugins = Table::new();
        plugins.insert("enable".into(), Value::String(String::new()));
        plugins.insert("disable".into(), Value::String(String::new()));
        let mut root = Table::new();
        root.insert("server".into(), Value::Table(server));
        root.insert("auth".into(), Value::Table(auth));
        root.insert("plugins".into(), Value::Table(plugins));
        Ok(Value::Table(root))
    }
}

/// `PORT` and `HOST`, the names every host and container runtime already sets.
///
/// A fallback rather than an override: nothing is emitted for a variable that is
/// not set, and the source sits *below* `APP_*`, so `APP_SERVER__PORT` still
/// wins wherever both are given. It exists because "why is it not on the port I
/// set?" is the first thing anyone hits who runs the image behind a platform
/// that hands the port over in `PORT`.
struct LegacyEnvSource {
    host: Option<String>,
    port: Option<String>,
}

impl LegacyEnvSource {
    fn from_env() -> Self {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
        let source = Self {
            host: read("HOST"),
            port: read("PORT"),
        };
        if source.host.is_some() || source.port.is_some() {
            tracing::warn!("using PORT/HOST; the framework's own names are APP_SERVER__PORT / APP_SERVER__HOST and take precedence");
        }
        source
    }
}

impl Source for LegacyEnvSource {
    fn load(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let mut server = Table::new();
        if let Some(host) = &self.host {
            server.insert("host".into(), Value::String(host.clone()));
        }
        if let Some(port) = &self.port {
            server.insert("port".into(), Value::String(port.clone()));
        }
        let mut root = Table::new();
        // An empty `[server]` would still merge cleanly, but leaving it out
        // keeps "nothing was set" indistinguishable from "this source is absent".
        if !server.is_empty() {
            root.insert("server".into(), Value::Table(server));
        }
        Ok(Value::Table(root))
    }
}

/// Load defaults, then `configurations/application.toml`, then `PORT`/`HOST`,
/// then `APP_*`.
///
/// The profile name comes from `GOTCHA_ACTIVE_PROFILE`, matching the framework.
pub fn load(profile: Option<String>) -> Result<ConfigWrapper<AppConfig>, ConfigError> {
    let mut loader = ConfigLoader::new();
    loader.add_source(DefaultsSource);
    loader.add_file_if_present("configurations/application.toml");
    if let Some(profile) = profile {
        loader.add_file_if_present(&format!("configurations/application_{profile}.toml"));
    }
    loader.add_source(LegacyEnvSource::from_env());
    loader.add_source(EnvironmentSource::new("APP"));
    loader.enable_path_variable_processor();
    loader.enable_environment_variable_processor();
    loader.construct().map_err(|error| ConfigError::Error(error.to_string()))
}

/// A file source, added only when the file is there.
trait AddFileSourceExt {
    /// Add `path` as a source when it exists; a configuration file that is not
    /// there is not an error.
    fn add_file_if_present(&mut self, path: &str);
}

impl AddFileSourceExt for ConfigLoader {
    fn add_file_if_present(&mut self, path: &str) {
        if Path::new(path).exists() {
            self.add_source(FileSource::new(std::path::PathBuf::from(path)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for `EnvironmentSource`, which reads the real environment and
    /// so cannot be pointed at a fixture.
    struct StubSource(Value);

    impl Source for StubSource {
        fn load(&self) -> Result<Value, Box<dyn std::error::Error>> {
            Ok(self.0.clone())
        }
    }

    fn server_table(port: &str) -> Value {
        let mut server = Table::new();
        server.insert("port".into(), Value::String(port.into()));
        let mut root = Table::new();
        root.insert("server".into(), Value::Table(server));
        Value::Table(root)
    }

    /// `load`'s source order, with the `APP_*` source stubbed.
    fn resolve(legacy: LegacyEnvSource, app: Option<Value>) -> ConfigWrapper<AppConfig> {
        let mut loader = ConfigLoader::new();
        loader.add_source(DefaultsSource);
        loader.add_source(legacy);
        if let Some(app) = app {
            loader.add_source(StubSource(app));
        }
        loader.construct().expect("construct")
    }

    #[test]
    fn port_and_host_fill_in_when_the_framework_names_are_unset() {
        let config = resolve(
            LegacyEnvSource {
                host: Some("0.0.0.0".into()),
                port: Some("9000".into()),
            },
            None,
        );
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.host, "0.0.0.0");
    }

    #[test]
    fn the_framework_names_win_over_port_and_host() {
        let config = resolve(
            LegacyEnvSource {
                host: Some("0.0.0.0".into()),
                port: Some("9000".into()),
            },
            Some(server_table("8701")),
        );
        assert_eq!(config.server.port, 8701, "APP_SERVER__PORT is the documented name");
        assert_eq!(config.server.host, "0.0.0.0", "and it only overrides what it names");
    }

    #[test]
    fn an_unset_port_and_host_leave_the_defaults_alone() {
        let config = resolve(LegacyEnvSource { host: None, port: None }, None);
        assert_eq!((config.server.host.as_str(), config.server.port), (DEFAULT_HOST, DEFAULT_PORT));
    }

    #[test]
    fn the_instance_name_defaults_and_treats_blank_as_unset() {
        assert_eq!(AppConfig::default().instance(), crate::instance::DEFAULT_INSTANCE);
        let blank = AppConfig {
            instance: Some("   ".into()),
            ..AppConfig::default()
        };
        assert_eq!(blank.instance(), crate::instance::DEFAULT_INSTANCE);
        let set = AppConfig {
            instance: Some(" staging ".into()),
            ..AppConfig::default()
        };
        assert_eq!(set.instance(), "staging", "the name is trimmed, like every other config string");
    }

    #[test]
    fn the_defaults_source_is_typed_so_a_string_port_can_coerce() {
        let value = DefaultsSource.load().expect("defaults load");
        let port = value.get("server").and_then(|s| s.get("port")).expect("server.port");
        assert!(port.is_integer(), "a string default would make APP_SERVER__PORT fail to deserialize");
        assert_eq!(port.as_integer(), Some(i64::from(DEFAULT_PORT)));
    }

    /// Same rule, one section over: without an integer to coerce against,
    /// `APP_AUTH__SESSION_TTL_DAYS=14` would fail the whole configuration load.
    #[test]
    fn the_numeric_auth_settings_have_typed_defaults() {
        let value = DefaultsSource.load().expect("defaults load");
        let auth = value.get("auth").expect("auth section");
        for key in ["session_ttl_days", "lockout_attempts", "lockout_minutes"] {
            assert!(auth.get(key).is_some_and(mofa::toml::Value::is_integer), "{key} must be typed");
        }
        assert!(auth.get("enabled").is_none(), "three-state, so there is nothing to default it to");
    }

    fn auth_table(pairs: &[(&str, Value)]) -> Value {
        let mut auth = Table::new();
        for (key, value) in pairs {
            auth.insert((*key).into(), value.clone());
        }
        let mut root = Table::new();
        root.insert("auth".into(), Value::Table(auth));
        Value::Table(root)
    }

    #[test]
    fn the_auth_settings_default_when_nothing_is_configured() {
        let config = resolve(LegacyEnvSource { host: None, port: None }, None);
        assert_eq!(config.app.auth.enabled(), None, "unset means 'on when accounts exist'");
        assert_eq!(config.app.auth.session_ttl_days(), DEFAULT_SESSION_TTL_DAYS);
        assert_eq!(config.app.auth.lockout_attempts(), DEFAULT_LOCKOUT_ATTEMPTS);
        assert_eq!(config.app.auth.lockout_minutes(), DEFAULT_LOCKOUT_MINUTES);
    }

    /// Everything an environment variable can say arrives as a string, so both
    /// halves of the coercion story have to hold at once.
    #[test]
    fn the_auth_settings_survive_arriving_as_strings() {
        let config = resolve(
            LegacyEnvSource { host: None, port: None },
            Some(auth_table(&[
                ("enabled", Value::String("true".into())),
                ("session_ttl_days", Value::String("14".into())),
                ("lockout_attempts", Value::String("0".into())),
                ("lockout_minutes", Value::String("30".into())),
            ])),
        );
        assert_eq!(config.app.auth.enabled(), Some(true));
        assert_eq!(config.app.auth.session_ttl_days(), 14);
        assert_eq!(config.app.auth.lockout_attempts(), 0, "zero is honoured: it means never lock");
        assert_eq!(config.app.auth.lockout_minutes(), 30);
    }

    #[test]
    fn a_real_boolean_in_the_config_file_works_too() {
        let config = resolve(
            LegacyEnvSource { host: None, port: None },
            Some(auth_table(&[("enabled", Value::Boolean(false))])),
        );
        assert_eq!(config.app.auth.enabled(), Some(false));
    }

    /// A nonsense value is a refusal at boot rather than a silent fallback: the
    /// one thing worse than an unreadable `auth.enabled` is one that quietly
    /// reads as "off".
    #[test]
    fn an_unparseable_enabled_is_an_error_not_a_default() {
        let mut loader = ConfigLoader::new();
        loader.add_source(DefaultsSource);
        loader.add_source(StubSource(auth_table(&[("enabled", Value::String("maybe".into()))])));
        assert!(loader.construct::<ConfigWrapper<AppConfig>>().is_err());
    }

    #[test]
    fn plugin_switches_default_to_nothing_and_read_comma_separated_strings() {
        let config = resolve(LegacyEnvSource { host: None, port: None }, None);
        assert!(config.app.plugins.enabled().is_empty());
        assert!(config.app.plugins.disabled().is_empty());
        let value = DefaultsSource.load().expect("defaults load");
        let plugins = value.get("plugins").expect("plugins section");
        for key in ["enable", "disable"] {
            assert!(plugins.get(key).is_some_and(mofa::toml::Value::is_str), "{key} is a typed string");
        }

        let mut section = Table::new();
        section.insert("enable".into(), Value::String(" tasks, notes ,,".into()));
        section.insert("disable".into(), Value::String("notes".into()));
        let mut root = Table::new();
        root.insert("plugins".into(), Value::Table(section));
        let config = resolve(LegacyEnvSource { host: None, port: None }, Some(Value::Table(root)));
        assert_eq!(config.app.plugins.enabled().into_iter().collect::<Vec<_>>(), ["notes", "tasks"]);
        assert_eq!(config.app.plugins.disabled().into_iter().collect::<Vec<_>>(), ["notes"]);
    }

    /// Zero days would mint tokens that have already expired; the accessor
    /// treats it as unset rather than as "nobody may ever log in".
    #[test]
    fn a_zero_session_lifetime_falls_back_rather_than_expiring_every_token() {
        let settings = AuthSettings {
            session_ttl_days: Some(0),
            lockout_minutes: Some(0),
            ..AuthSettings::default()
        };
        assert_eq!(settings.session_ttl_days(), DEFAULT_SESSION_TTL_DAYS);
        assert_eq!(settings.lockout_minutes(), DEFAULT_LOCKOUT_MINUTES);
    }
}
