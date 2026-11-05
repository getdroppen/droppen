//! Application configuration.
//!
//! gotcha reads `configurations/application.toml` plus the `APP_` environment
//! prefix, with `__` separating nested sections (`APP_SERVER__PORT`). The
//! framework's own bind address lives in the reserved `[server]` section, so:
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
//! The load is gotcha's [`ConfigBuilder`] with droppen's typed [`Defaults`]
//! underneath every file and environment source. Being typed is what makes
//! `APP_SERVER__PORT` work with no configuration file at all: mofa's environment
//! source can only produce strings, and its merge coerces one into a number only
//! where an earlier source already put a number at that path
//! (`mofa-0.2.0/src/utils.rs:122`), so `server.port`, `server.host` and the
//! numeric auth settings are integers in the defaults and an environment value
//! has something to coerce against. The defaults also carry the contract's
//! `127.0.0.1:8700` in place of gotcha's `3000`; as a real source rather than a
//! post-load fix-up, that cannot confuse "unset" with "deliberately 3000".
//!
//! `HOST` and `PORT` — the names every host and container runtime already sets —
//! are the one layer the builder has no slot for. They sit above the files and
//! below `APP_*`, and a source can only be added below the files (the typed
//! defaults) or above everything (`env`); [`ConfigBuilder`] has no
//! add-a-source-here method. [`LegacyEnv`] therefore applies them to the loaded
//! value, where "below `APP_*`" is a check for the framework-named variable.

use gotcha::config::{ConfigBuilder, ConfigError};
use gotcha::{config, ConfigWrapper, ServerConfig};
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
    /// exclusive lock on it at boot (see [`droppen_instance::instance`]). Two servers on
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
/// carry a typed empty-string default in [`Defaults`]. A plugin named in
/// `disable` is off whatever else says so; one named only in `enable` is on;
/// either way the switch in the settings is read-only for it
/// (the server's `plugins`).
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

/// What the server's `auth` reads out of the configuration.
///
/// Every field is optional and every default is expressed twice: once as the
/// accessor's fallback below, and once — typed — in [`Defaults`]. The second
/// copy is not redundant. mofa's environment source can only produce strings,
/// and its merge coerces one into a number only where an earlier source
/// already put a number (`mofa-0.2.0/src/utils.rs:122`), so without the typed
/// default `APP_AUTH__SESSION_TTL_DAYS=14` would arrive at serde as
/// `Value::String("14")` and fail the entire load. See the module docs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthSettings {
    /// Force authentication on (`true`) or off (`false`).
    ///
    /// Unset means on, the same as `true`: an instance with a user account asks
    /// for a login, one without is in its setup state (the server's `auth::setup`).
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
        // `DROPPEN_*` override in `droppen_instance::paths::Paths` read one.
        self.instance
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(droppen_instance::instance::DEFAULT_INSTANCE)
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
/// Every later source (file, `HOST`/`PORT`, then `APP_*`) merges on top, and a
/// typed value here is what lets an environment string be coerced into a
/// number at that path. `server` is the framework's reserved section; the rest
/// sits at the top level of the document.
#[derive(Serialize)]
struct Defaults {
    server: ServerConfig,
    auth: AuthDefaults,
    plugins: PluginDefaults,
}

/// The numeric auth defaults, typed so the environment can override them.
#[derive(Serialize)]
struct AuthDefaults {
    session_ttl_days: u32,
    lockout_attempts: u32,
    lockout_minutes: u32,
}

/// The plugin switches, present with their type so the section always has the
/// shape [`PluginSettings`] reads.
#[derive(Serialize)]
struct PluginDefaults {
    enable: String,
    disable: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: DEFAULT_HOST.to_string(),
                port: DEFAULT_PORT,
            },
            auth: AuthDefaults {
                session_ttl_days: DEFAULT_SESSION_TTL_DAYS,
                lockout_attempts: DEFAULT_LOCKOUT_ATTEMPTS,
                lockout_minutes: DEFAULT_LOCKOUT_MINUTES,
            },
            plugins: PluginDefaults {
                enable: String::new(),
                disable: String::new(),
            },
        }
    }
}

/// `PORT` and `HOST`, the names every host and container runtime already sets.
///
/// A fallback rather than an override: a variable that is not set changes
/// nothing, and the layer sits *below* `APP_*`, so `APP_SERVER__PORT` still
/// wins wherever both are given. It exists because "why is it not on the port I
/// set?" is the first thing anyone hits who runs the image behind a platform
/// that hands the port over in `PORT`.
///
/// Applied to the loaded configuration rather than added to the builder: it has
/// to sit between the profile file and `APP_*`, and gotcha's builder only takes
/// a source below the files or above every environment variable. See the
/// module docs.
struct LegacyEnv {
    host: Option<String>,
    port: Option<String>,
}

impl LegacyEnv {
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

    /// Apply the fallback: each value fills in only what the framework-named
    /// variable leaves alone.
    fn apply(self, server: &mut ServerConfig) -> Result<(), ConfigError> {
        let framework_names = |name: &str| std::env::var_os(name).is_some();
        if let Some(host) = self.host {
            if !framework_names("APP_SERVER__HOST") {
                server.host = host;
            }
        }
        if let Some(port) = self.port {
            // Parsed even when `APP_SERVER__PORT` wins: the loader used to
            // coerce this layer before the environment source was reached, so
            // a bad `PORT` refused to start either way.
            let parsed = port
                .trim()
                .parse::<u16>()
                .map_err(|_| ConfigError::Error(format!("PORT is a number between 0 and 65535, not {port:?}")))?;
            if !framework_names("APP_SERVER__PORT") {
                server.port = parsed;
            }
        }
        Ok(())
    }
}

/// Load defaults, then `configurations/application.toml`, then the profile's
/// file, then `PORT`/`HOST`, then `APP_*`, substituting `${VAR}` and path
/// placeholders last.
///
/// The profile name comes from `GOTCHA_ACTIVE_PROFILE`, matching the framework;
/// the caller reads it and passes it here.
pub fn load(profile: Option<String>) -> Result<ConfigWrapper<AppConfig>, ConfigError> {
    let builder = ConfigBuilder::framework_defaults().defaults(Defaults::default())?;
    let builder = builder.file_optional("configurations/application.toml");
    let builder = match profile {
        Some(profile) => builder.file_optional(format!("configurations/application_{profile}.toml")),
        None => builder,
    };
    let mut config: ConfigWrapper<AppConfig> = builder.env("APP").enable_vars().build()?;
    LegacyEnv::from_env().apply(&mut config.server)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gotcha::config::toml::Value;

    /// Run the test body again in a child process whose working directory is an
    /// empty temporary directory and whose configuration environment is exactly
    /// `env`.
    ///
    /// `load` reads `configurations/` from the working directory and the
    /// process environment; both are process-wide, and the harness runs tests
    /// on threads. A test that needs either therefore gives itself a process of
    /// its own: returns `true` in the child, whose caller runs the body, and
    /// `false` in the parent, whose caller returns after checking the child
    /// passed. The parent's own environment is inherited except for the names
    /// this module reads, which `env` sets from scratch.
    fn in_isolated_process(name: &str, env: &[(&str, &str)]) -> bool {
        const MARKER: &str = "DROPPEN_HTTP_CONFIG_TEST_CHILD";
        if std::env::var_os(MARKER).is_some() {
            return true;
        }
        let test = name.rsplit("::").next().unwrap_or(name);
        let dir = std::env::temp_dir().join(format!("droppen-http-config-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the child's directory");
        let mut command = std::process::Command::new(std::env::current_exe().expect("the test binary"));
        command.args(["--exact", name, "--nocapture"]).current_dir(&dir).env(MARKER, "1");
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("APP_") || matches!(key.as_ref(), "HOST" | "PORT" | "GOTCHA_ACTIVE_PROFILE") {
                command.env_remove(key.as_ref());
            }
        }
        command.envs(env.iter().copied());
        let output = command.output().expect("the child runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("test result: ok. 1 passed"),
            "the child running {name} failed or did not run:\n{stdout}\n{stderr}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        false
    }

    /// The defaults as they reach gotcha's builder.
    fn defaults_value() -> Value {
        Value::try_from(Defaults::default()).expect("the defaults hold only strings and integers")
    }

    /// Every layer at once: typed defaults, the base file, the profile file,
    /// `HOST`/`PORT`, and `APP_*` — each overriding the one before it, with
    /// `${VAR}` substituted last.
    #[test]
    fn the_sources_keep_their_precedence() {
        const NAME: &str = "config::tests::the_sources_keep_their_precedence";
        if !in_isolated_process(
            NAME,
            &[
                ("GOTCHA_ACTIVE_PROFILE", "staging"),
                ("DROPPEN_CONFIG_TEST_HOME", "/profile-from-variable"),
                ("HOST", "10.0.0.3"),
                ("PORT", "9001"),
                ("APP_SERVER__PORT", "9002"),
            ],
        ) {
            return;
        }
        std::fs::create_dir_all("configurations").expect("the configuration directory");
        std::fs::write(
            "configurations/application.toml",
            "home_dir = \"/base\"\n\n[server]\nhost = \"10.0.0.1\"\nport = 1234\n\n[plugins]\nenable = \"from-file\"\n",
        )
        .expect("the base file");
        std::fs::write(
            "configurations/application_staging.toml",
            "home_dir = \"${DROPPEN_CONFIG_TEST_HOME}\"\n\n[server]\nhost = \"10.0.0.2\"\nport = 2345\n\n[plugins]\ndisable = \"from-profile\"\n",
        )
        .expect("the profile file");

        let config = load(Some("staging".into())).expect("the configuration loads");

        assert_eq!(
            config.home(),
            std::path::PathBuf::from("/profile-from-variable"),
            "the profile file wins over the base file, and ${{VAR}} is substituted"
        );
        assert_eq!(
            config.plugins.enabled().into_iter().collect::<Vec<_>>(),
            ["from-file"],
            "a key the profile does not name keeps the base file's value"
        );
        assert_eq!(config.plugins.disabled().into_iter().collect::<Vec<_>>(), ["from-profile"]);
        assert_eq!(config.server.host, "10.0.0.3", "HOST wins over both files when APP_SERVER__HOST is unset");
        assert_eq!(config.server.port, 9002, "APP_SERVER__PORT is the top layer");
        assert_eq!(config.auth.session_ttl_days(), DEFAULT_SESSION_TTL_DAYS, "the typed defaults sit underneath");
    }

    /// With no configuration file at all, an environment value still reaches a
    /// typed field: the defaults give mofa an integer to coerce against.
    #[test]
    fn app_server_port_loads_with_no_configuration_file() {
        const NAME: &str = "config::tests::app_server_port_loads_with_no_configuration_file";
        if !in_isolated_process(NAME, &[("APP_SERVER__PORT", "9000")]) {
            return;
        }
        let config = load(None).expect("environment-only configuration loads");
        assert_eq!(config.server.port, 9000, "the typed default lets the environment value coerce");
        assert_eq!(config.server.host, DEFAULT_HOST, "unset fields keep their defaults");
    }

    #[test]
    fn port_and_host_fill_in_when_the_framework_names_are_unset() {
        const NAME: &str = "config::tests::port_and_host_fill_in_when_the_framework_names_are_unset";
        if !in_isolated_process(NAME, &[("HOST", "0.0.0.0"), ("PORT", "9000")]) {
            return;
        }
        let config = load(None).expect("the configuration loads");
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.host, "0.0.0.0");
    }

    #[test]
    fn the_framework_names_win_over_port_and_host() {
        const NAME: &str = "config::tests::the_framework_names_win_over_port_and_host";
        if !in_isolated_process(NAME, &[("HOST", "0.0.0.0"), ("PORT", "9000"), ("APP_SERVER__PORT", "8701")]) {
            return;
        }
        let config = load(None).expect("the configuration loads");
        assert_eq!(config.server.port, 8701, "APP_SERVER__PORT is the documented name");
        assert_eq!(config.server.host, "0.0.0.0", "and it only overrides what it names");
    }

    #[test]
    fn an_unset_port_and_host_leave_the_defaults_alone() {
        const NAME: &str = "config::tests::an_unset_port_and_host_leave_the_defaults_alone";
        if !in_isolated_process(NAME, &[]) {
            return;
        }
        let config = load(None).expect("the configuration loads");
        assert_eq!((config.server.host.as_str(), config.server.port), (DEFAULT_HOST, DEFAULT_PORT));
    }

    /// A `PORT` that is not a port refuses to start, even when
    /// `APP_SERVER__PORT` would have won: that is what the old layer did when
    /// it was merged, and a typo'd variable should not pass silently.
    #[test]
    fn an_unparsable_port_is_an_error() {
        const NAME: &str = "config::tests::an_unparsable_port_is_an_error";
        if !in_isolated_process(NAME, &[("PORT", "not-a-port"), ("APP_SERVER__PORT", "9000")]) {
            return;
        }
        assert!(load(None).is_err());
    }

    #[test]
    fn the_instance_name_defaults_and_treats_blank_as_unset() {
        assert_eq!(AppConfig::default().instance(), droppen_instance::instance::DEFAULT_INSTANCE);
        let blank = AppConfig {
            instance: Some("   ".into()),
            ..AppConfig::default()
        };
        assert_eq!(blank.instance(), droppen_instance::instance::DEFAULT_INSTANCE);
        let set = AppConfig {
            instance: Some(" staging ".into()),
            ..AppConfig::default()
        };
        assert_eq!(set.instance(), "staging", "the name is trimmed, like every other config string");
    }

    #[test]
    fn the_defaults_are_typed_so_a_string_port_can_coerce() {
        let value = defaults_value();
        let port = value.get("server").and_then(|s| s.get("port")).expect("server.port");
        assert!(port.is_integer(), "a string default would make APP_SERVER__PORT fail to deserialize");
        assert_eq!(port.as_integer(), Some(i64::from(DEFAULT_PORT)));
    }

    /// Same rule, one section over: without an integer to coerce against,
    /// `APP_AUTH__SESSION_TTL_DAYS=14` would fail the whole configuration load.
    #[test]
    fn the_numeric_auth_settings_have_typed_defaults() {
        let value = defaults_value();
        let auth = value.get("auth").expect("auth section");
        for key in ["session_ttl_days", "lockout_attempts", "lockout_minutes"] {
            assert!(auth.get(key).is_some_and(Value::is_integer), "{key} must be typed");
        }
        assert!(auth.get("enabled").is_none(), "three-state, so there is nothing to default it to");
    }

    #[test]
    fn the_auth_settings_default_when_nothing_is_configured() {
        const NAME: &str = "config::tests::the_auth_settings_default_when_nothing_is_configured";
        if !in_isolated_process(NAME, &[]) {
            return;
        }
        let config = load(None).expect("the configuration loads");
        assert_eq!(config.auth.enabled(), None, "unset means 'on when accounts exist'");
        assert_eq!(config.auth.session_ttl_days(), DEFAULT_SESSION_TTL_DAYS);
        assert_eq!(config.auth.lockout_attempts(), DEFAULT_LOCKOUT_ATTEMPTS);
        assert_eq!(config.auth.lockout_minutes(), DEFAULT_LOCKOUT_MINUTES);
    }

    /// Everything an environment variable can say arrives as a string, so both
    /// halves of the coercion story have to hold at once.
    #[test]
    fn the_auth_settings_survive_arriving_as_strings() {
        const NAME: &str = "config::tests::the_auth_settings_survive_arriving_as_strings";
        if !in_isolated_process(
            NAME,
            &[
                ("APP_AUTH__ENABLED", "true"),
                ("APP_AUTH__SESSION_TTL_DAYS", "14"),
                ("APP_AUTH__LOCKOUT_ATTEMPTS", "0"),
                ("APP_AUTH__LOCKOUT_MINUTES", "30"),
            ],
        ) {
            return;
        }
        let config = load(None).expect("the configuration loads");
        assert_eq!(config.auth.enabled(), Some(true));
        assert_eq!(config.auth.session_ttl_days(), 14);
        assert_eq!(config.auth.lockout_attempts(), 0, "zero is honoured: it means never lock");
        assert_eq!(config.auth.lockout_minutes(), 30);
    }

    #[test]
    fn a_real_boolean_in_the_config_file_works_too() {
        const NAME: &str = "config::tests::a_real_boolean_in_the_config_file_works_too";
        if !in_isolated_process(NAME, &[]) {
            return;
        }
        std::fs::create_dir_all("configurations").expect("the configuration directory");
        std::fs::write("configurations/application.toml", "[auth]\nenabled = false\n").expect("the configuration file");
        let config = load(None).expect("the configuration loads");
        assert_eq!(config.auth.enabled(), Some(false));
    }

    /// A nonsense value is a refusal at boot rather than a silent fallback: the
    /// one thing worse than an unreadable `auth.enabled` is one that quietly
    /// reads as "off".
    #[test]
    fn an_unparseable_enabled_is_an_error_not_a_default() {
        const NAME: &str = "config::tests::an_unparseable_enabled_is_an_error_not_a_default";
        if !in_isolated_process(NAME, &[]) {
            return;
        }
        std::fs::create_dir_all("configurations").expect("the configuration directory");
        std::fs::write("configurations/application.toml", "[auth]\nenabled = \"maybe\"\n").expect("the configuration file");
        assert!(load(None).is_err());
    }

    #[test]
    fn plugin_switches_default_to_nothing_and_read_comma_separated_strings() {
        assert!(PluginSettings::default().enabled().is_empty());
        assert!(PluginSettings::default().disabled().is_empty());
        let value = defaults_value();
        let plugins = value.get("plugins").expect("plugins section");
        for key in ["enable", "disable"] {
            assert!(plugins.get(key).is_some_and(Value::is_str), "{key} is a typed string");
        }

        let settings = PluginSettings {
            enable: Some(" tasks, notes ,,".into()),
            disable: Some("notes".into()),
        };
        assert_eq!(settings.enabled().into_iter().collect::<Vec<_>>(), ["notes", "tasks"]);
        assert_eq!(settings.disabled().into_iter().collect::<Vec<_>>(), ["notes"]);
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
