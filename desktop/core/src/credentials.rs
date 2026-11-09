//! The local account's password: the app's own secret.
//!
//! On first launch the app makes up a strong random password, creates a local
//! account on its server instance with it ([`crate::accounts`]) and keeps it
//! here, so every later launch can log in without asking anybody anything.
//! The user never sees it and never needs to: the app can always reset it with
//! the server's own `user passwd`, because whoever runs the app already owns
//! the operating-system account the server runs as.
//!
//! Where it is kept is behind [`KeyStore`] — the seam the future `KeyStore`
//! interface of `docs/local-aggregator.md` §7.3 grows from:
//!
//! * `KeychainStore` (macOS): one generic-password item in the user's login
//!   keychain, service [`crate::BUNDLE_ID`], account `local-login:<instance>`.
//!   The app only ever reads and writes **this one item**, which it created.
//! * `SecretServiceStore` (Linux), the keychain there: one item in the Secret
//!   Service's default collection, its attributes `service` and `account` the
//!   same. It never prompts, and gives up on a Secret Service that does not
//!   answer in time (`credentials/secret_service.rs`).
//! * [`FileStore`]: `credentials.json`, 0600, in the app's 0700 support
//!   directory — the fallback when the keychain will not take or give the item.
//! * [`LayeredStore`] puts the two together: the keychain first, the file when
//!   it fails, and the file is removed once the keychain holds the secret.
//!
//! Tests and manual test runs turn the keychain off ([`keychain_enabled`]), so
//! nothing a test does can touch a real keychain or keyring.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use base64::Engine;
use serde::{Deserialize, Serialize};

#[cfg(not(target_os = "macos"))]
pub mod secret_service;
#[cfg(not(target_os = "macos"))]
pub use secret_service::SecretServiceStore;

/// The environment switch for the keychain (on Linux, the Secret Service):
/// `DROPPEN_DESKTOP_KEYCHAIN=0` (or `off`, `false`, `no`) keeps the password
/// in the file instead.
pub const KEYCHAIN_ENV: &str = "DROPPEN_DESKTOP_KEYCHAIN";

/// What this platform's keychain is called in the log: its store's
/// [`KeyStore::kind`].
pub const KEYCHAIN: &str = if cfg!(target_os = "macos") { "keychain" } else { "Secret Service" };

/// Whether the keychain may be used, from [`KEYCHAIN_ENV`]. On by default.
pub fn keychain_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|value| value.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// The local account the app logs in as.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// The account name on the server instance.
    pub username: String,
    /// Its password, made up by [`generate_password`].
    pub password: String,
}

impl std::fmt::Debug for Credential {
    // The password never reaches a log line, even through `{:?}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A new password: 32 random bytes, base64url — 43 characters, far past the
/// server's minimum, with nothing a shell or a terminal could mangle.
pub fn generate_password() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Why a store could not do what it was asked.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

/// Somewhere a [`Credential`] can be kept.
pub trait KeyStore: Send + Sync {
    /// A short name for the log: `keychain`, `file`.
    fn kind(&self) -> &'static str;
    /// The credential, `None` if there is none yet.
    fn load(&self) -> Result<Option<Credential>, StoreError>;
    /// Keep `credential`, replacing whatever was there.
    fn save(&self, credential: &Credential) -> Result<(), StoreError>;
    /// Forget it; not an error if there was nothing.
    fn delete(&self) -> Result<(), StoreError>;
}

/// `credentials.json`, 0600.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// A store at `path` (the support directory's `credentials.json`).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Where it writes.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl KeyStore for FileStore {
    fn kind(&self) -> &'static str {
        "file"
    }

    fn load(&self) -> Result<Option<Credential>, StoreError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // Whatever loosened it, it is 0600 again before anything else happens.
        let mode = std::fs::metadata(&self.path)?.permissions().mode() & 0o777;
        if mode != 0o600 {
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| StoreError(format!("{} is not a credential file: {error}", self.path.display())))
    }

    fn save(&self, credential: &Credential) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            crate::dirs::create_private_dir(parent)?;
        }
        let body = serde_json::to_vec(credential).map_err(|error| StoreError(error.to_string()))?;
        crate::state::write_atomically(&self.path, &body, 0o600)?;
        Ok(())
    }

    fn delete(&self) -> Result<(), StoreError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// The macOS keychain: one generic-password item the app itself created.
#[cfg(target_os = "macos")]
pub struct KeychainStore {
    service: String,
    account: String,
}

#[cfg(target_os = "macos")]
impl KeychainStore {
    /// The item for `instance`'s local login.
    pub fn for_instance(instance: &str) -> Self {
        Self {
            service: crate::BUNDLE_ID.to_string(),
            account: format!("local-login:{instance}"),
        }
    }
}

#[cfg(target_os = "macos")]
impl KeyStore for KeychainStore {
    fn kind(&self) -> &'static str {
        "keychain"
    }

    fn load(&self) -> Result<Option<Credential>, StoreError> {
        use security_framework::passwords::get_generic_password;
        /// `errSecItemNotFound`.
        const NOT_FOUND: i32 = -25300;
        match get_generic_password(&self.service, &self.account) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| StoreError(format!("the keychain item is not a credential: {error}"))),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(StoreError(format!("keychain: {error}"))),
        }
    }

    fn save(&self, credential: &Credential) -> Result<(), StoreError> {
        let body = serde_json::to_vec(credential).map_err(|error| StoreError(error.to_string()))?;
        security_framework::passwords::set_generic_password(&self.service, &self.account, &body)
            .map_err(|error| StoreError(format!("keychain: {error}")))
    }

    fn delete(&self) -> Result<(), StoreError> {
        /// `errSecItemNotFound`.
        const NOT_FOUND: i32 = -25300;
        match security_framework::passwords::delete_generic_password(&self.service, &self.account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(StoreError(format!("keychain: {error}"))),
        }
    }
}

/// This platform's keychain item for `instance`.
fn keychain_item(instance: &str) -> Box<dyn KeyStore> {
    #[cfg(target_os = "macos")]
    let item = KeychainStore::for_instance(instance);
    #[cfg(not(target_os = "macos"))]
    let item = SecretServiceStore::for_instance(instance);
    Box::new(item)
}

/// The keychain when there is one, the file when it fails.
pub struct LayeredStore {
    primary: Option<Box<dyn KeyStore>>,
    fallback: Box<dyn KeyStore>,
}

/// Where a load found the credential, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// In the primary store (the keychain).
    Primary,
    /// In the fallback file.
    Fallback,
    /// Nowhere: this is a first launch, or the secret is gone.
    Nowhere,
}

impl LayeredStore {
    /// `primary` (the keychain, or `None` when it is switched off) over `fallback`.
    pub fn new(primary: Option<Box<dyn KeyStore>>, fallback: Box<dyn KeyStore>) -> Self {
        Self { primary, fallback }
    }

    /// The app's store: the keychain item for `instance` when `keychain` is on
    /// (on Linux, the Secret Service's), over `file`.
    pub fn for_app(instance: &str, keychain: bool, file: PathBuf) -> Self {
        let primary = keychain.then(|| keychain_item(instance));
        Self::new(primary, Box::new(FileStore::new(file)))
    }

    /// The credential and where it was. A keychain that errors (locked,
    /// access refused) is logged and skipped, never fatal: the app can always
    /// make a new password and reset the account with it.
    pub fn load(&self) -> (Option<Credential>, Found) {
        if let Some(primary) = &self.primary {
            match primary.load() {
                Ok(Some(credential)) => return (Some(credential), Found::Primary),
                Ok(None) => {}
                Err(error) => log::warn!("cannot read the {} credential, trying the file: {error}", primary.kind()),
            }
        }
        match self.fallback.load() {
            Ok(Some(credential)) => (Some(credential), Found::Fallback),
            Ok(None) => (None, Found::Nowhere),
            Err(error) => {
                log::warn!("cannot read the {} credential: {error}", self.fallback.kind());
                (None, Found::Nowhere)
            }
        }
    }

    /// Keep `credential`: in the keychain if it takes it (then the file copy is
    /// removed, so the secret is in one place), otherwise in the file — and
    /// then the keychain's old item goes, if it lets it: [`LayeredStore::load`]
    /// reads the keychain first, and a stale login read back from there would
    /// stand for an account the app no longer signs in as.
    /// Returns the kind of store that holds it now.
    pub fn save(&self, credential: &Credential) -> Result<&'static str, StoreError> {
        if let Some(primary) = &self.primary {
            match primary.save(credential) {
                Ok(()) => {
                    if let Err(error) = self.fallback.delete() {
                        log::warn!("could not remove the old {} credential: {error}", self.fallback.kind());
                    }
                    return Ok(primary.kind());
                }
                Err(error) => log::warn!("cannot write the {} credential, using the file: {error}", primary.kind()),
            }
        }
        self.fallback.save(credential)?;
        if let Some(primary) = &self.primary {
            if let Err(error) = primary.delete() {
                log::warn!("could not remove the old {} credential, which is read first: {error}", primary.kind());
            }
        }
        Ok(self.fallback.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A primary store that can be told to fail, or to refuse only writes.
    #[derive(Default)]
    struct Memory {
        value: Mutex<Option<Credential>>,
        broken: bool,
        refuses_writes: bool,
    }

    impl KeyStore for Memory {
        fn kind(&self) -> &'static str {
            "memory"
        }
        fn load(&self) -> Result<Option<Credential>, StoreError> {
            if self.broken {
                return Err(StoreError("locked".into()));
            }
            Ok(self.value.lock().expect("lock").clone())
        }
        fn save(&self, credential: &Credential) -> Result<(), StoreError> {
            if self.broken || self.refuses_writes {
                return Err(StoreError("locked".into()));
            }
            *self.value.lock().expect("lock") = Some(credential.clone());
            Ok(())
        }
        fn delete(&self) -> Result<(), StoreError> {
            *self.value.lock().expect("lock") = None;
            Ok(())
        }
    }

    fn credential(password: &str) -> Credential {
        Credential {
            username: "ada".into(),
            password: password.into(),
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_keychain_item_is_this_builds_own() {
        let item = KeychainStore::for_instance(crate::INSTANCE);
        let expected = if cfg!(feature = "e2e") {
            ("app.droppen.desktop.e2e", "local-login:droppen-app-e2e")
        } else {
            ("app.droppen.desktop", "local-login:droppen-app")
        };
        assert_eq!((item.service.as_str(), item.account.as_str()), expected);
    }

    #[test]
    fn the_keychain_is_named_in_the_log_as_its_store_is() {
        assert_eq!(keychain_item(crate::INSTANCE).kind(), KEYCHAIN);
    }

    #[test]
    fn the_keychain_switch_is_on_unless_turned_off() {
        assert!(keychain_enabled(None));
        assert!(keychain_enabled(Some("1")));
        assert!(keychain_enabled(Some("")));
        for off in ["0", "off", "false", "no", " OFF "] {
            assert!(!keychain_enabled(Some(off)), "{off}");
        }
    }

    #[test]
    fn passwords_are_long_random_and_shell_safe() {
        let one = generate_password().expect("one");
        let two = generate_password().expect("two");
        assert_ne!(one, two);
        assert_eq!(one.len(), 43);
        assert!(one.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn debug_never_shows_the_password() {
        let shown = format!("{:?}", credential("hunter2-hunter2"));
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("ada"));
    }

    #[test]
    fn the_file_store_is_0600_and_tightens_a_loosened_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(root.path().join("support/credentials.json"));
        assert_eq!(store.load().expect("empty"), None);
        store.save(&credential("first-password-1234")).expect("save");
        let mode = |path: &std::path::Path| std::fs::metadata(path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode(store.path()), 0o600);
        assert_eq!(mode(store.path().parent().expect("parent")), 0o700);
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(store.load().expect("load"), Some(credential("first-password-1234")));
        assert_eq!(mode(store.path()), 0o600);
        store.delete().expect("delete");
        store.delete().expect("deleting nothing is fine");
        assert_eq!(store.load().expect("gone"), None);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_credential() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(root.path().join("credentials.json"));
        std::fs::write(store.path(), "garbage").expect("write");
        assert!(store.load().is_err());
        let layered = LayeredStore::new(None, Box::new(FileStore::new(store.path())));
        assert_eq!(layered.load(), (None, Found::Nowhere));
    }

    #[test]
    fn with_the_keychain_off_everything_goes_to_the_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = root.path().join("credentials.json");
        let store = LayeredStore::for_app("desktop", false, file.clone());
        assert_eq!(store.load(), (None, Found::Nowhere));
        assert_eq!(store.save(&credential("a-password-for-tests")).expect("save"), "file");
        assert!(file.is_file());
        assert_eq!(store.load(), (Some(credential("a-password-for-tests")), Found::Fallback));
    }

    #[test]
    fn a_working_primary_holds_the_secret_alone() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = FileStore::new(root.path().join("credentials.json"));
        file.save(&credential("old-file-password")).expect("seed the file");
        let store = LayeredStore::new(Some(Box::new(Memory::default())), Box::new(FileStore::new(file.path())));
        assert_eq!(store.load(), (Some(credential("old-file-password")), Found::Fallback));
        assert_eq!(store.save(&credential("new-password-xyz")).expect("save"), "memory");
        assert!(!file.path().exists(), "the file copy is removed once the primary has it");
        assert_eq!(store.load(), (Some(credential("new-password-xyz")), Found::Primary));
    }

    #[test]
    fn a_broken_primary_falls_back_to_the_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("credentials.json");
        let broken = Memory {
            broken: true,
            ..Memory::default()
        };
        let store = LayeredStore::new(Some(Box::new(broken)), Box::new(FileStore::new(&path)));
        assert_eq!(store.load(), (None, Found::Nowhere));
        assert_eq!(store.save(&credential("fallback-password")).expect("save"), "file");
        assert_eq!(store.load(), (Some(credential("fallback-password")), Found::Fallback));
    }

    #[test]
    fn a_login_the_keychain_would_not_take_leaves_no_stale_one_there() {
        // The keychain holds an old login and will not take the new one: the
        // new one goes to the file, and the old item, read first, goes too —
        // or the next start would read the old login back.
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("credentials.json");
        let stale = Credential {
            username: "ada".into(),
            password: "stale-password".into(),
        };
        let keychain = Memory {
            value: Mutex::new(Some(stale.clone())),
            refuses_writes: true,
            ..Memory::default()
        };
        let store = LayeredStore::new(Some(Box::new(keychain)), Box::new(FileStore::new(&path)));
        assert_eq!(store.load(), (Some(stale), Found::Primary));
        let new = Credential {
            username: "droppen-app".into(),
            password: "new-password-xyz".into(),
        };
        assert_eq!(store.save(&new).expect("save"), "file");
        assert_eq!(store.load(), (Some(new), Found::Fallback), "the new login, not the stale one");
    }
}
