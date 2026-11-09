//! The Secret Service (`org.freedesktop.secrets` on the session bus: GNOME
//! Keyring, KWallet 5.97 and later, KeePassXC): Linux's keychain.
//!
//! One item in the default collection, the app's own: attributes `service`
//! ([`crate::BUNDLE_ID`]) and `account` (`local-login:<instance>`), the macOS
//! keychain item's service and account. The app reads, replaces and removes
//! that item only, and only in that collection.
//!
//! It never asks anybody anything. It opens no prompt, so it neither unlocks a
//! locked collection nor makes one where there is none: both ask for a
//! password, and a launch at login must not. And each load, save or delete has
//! [`DEADLINE`] in all, connecting to the bus included, after which the store
//! gives up for the rest of the run. Where it cannot do what it is asked — no
//! session bus (headless), no Secret Service (a minimal desktop), no default
//! collection, a locked one, one that does not answer — it says why, and
//! [`super::LayeredStore`] keeps the password in the file instead.
//!
//! The secret crosses the bus in the clear: the Secret Service's `plain`
//! algorithm, which every implementation has. The session bus is this user's
//! alone, and whatever of theirs could listen on it could as well ask for the
//! item (GNOME Keyring hands any client of the session an unlocked one) or read
//! `credentials.json`. The other algorithm, a Diffie-Hellman exchange and AES,
//! would keep it from nobody else, for a big-number and cipher stack more.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;
use zbus::zvariant::{DynamicDeserialize, DynamicType, OwnedObjectPath, OwnedValue, Value};

use super::{Credential, KeyStore, StoreError};

/// How long one load, save or delete may take in all. A healthy Secret Service
/// answers in milliseconds, and one D-Bus starts at the first call is up well
/// within it.
pub const DEADLINE: Duration = Duration::from_secs(5);

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_INTERFACE: &str = "org.freedesktop.Secret.Collection";
const ITEM_INTERFACE: &str = "org.freedesktop.Secret.Item";

/// The object path that stands for none: no collection, no prompt.
const NONE: &str = "/";

/// A secret on the wire: its session, the algorithm's parameters (none for
/// `plain`), the value, and its content type.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// The app's item in the Secret Service's default collection.
pub struct SecretServiceStore {
    attributes: HashMap<String, String>,
    label: String,
    /// The bus: `None` for the user's session bus; a test gives an address.
    address: Option<String>,
    deadline: Duration,
    /// Set once an operation ran past the deadline: the next ones fail at once
    /// rather than wait as long again, each, in the same start.
    stalled: AtomicBool,
}

impl SecretServiceStore {
    /// The item for `instance`'s local login.
    pub fn for_instance(instance: &str) -> Self {
        Self::on_bus(None, instance, DEADLINE)
    }

    fn on_bus(address: Option<String>, instance: &str, deadline: Duration) -> Self {
        Self {
            attributes: HashMap::from([
                ("service".to_string(), crate::BUNDLE_ID.to_string()),
                ("account".to_string(), format!("local-login:{instance}")),
            ]),
            // What a keyring manager (Seahorse, KWalletManager) lists it as.
            label: format!("{} local login ({instance})", crate::identity::THIS.name),
            address,
            deadline,
            stalled: AtomicBool::new(false),
        }
    }

    /// Run `operation` on a connection of its own, on a thread of its own, and
    /// wait for it until the deadline. One that outlives the deadline is left
    /// to finish or fail on its own; what it would still do is what the
    /// caller asked for, so nothing it does later contradicts what the
    /// [`super::LayeredStore`] did instead.
    fn run<T: Send + 'static>(&self, what: &str, operation: impl FnOnce(&Connection) -> Result<T, String> + Send + 'static) -> Result<T, StoreError> {
        let seconds = self.deadline.as_secs_f32();
        if self.stalled.load(Ordering::SeqCst) {
            return Err(StoreError(format!(
                "the Secret Service did not answer within {seconds}s earlier in this run"
            )));
        }
        let (address, deadline) = (self.address.clone(), self.deadline);
        let (done, outcome) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("secret-service".into())
            .spawn(move || {
                let _ = done.send(connect(address.as_deref(), deadline).and_then(|bus| operation(&bus)));
            })
            .map_err(|error| StoreError(format!("Secret Service: {error}")))?;
        match outcome.recv_timeout(self.deadline) {
            Ok(result) => result.map_err(|error| StoreError(format!("Secret Service: {error}"))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.stalled.store(true, Ordering::SeqCst);
                Err(StoreError(format!("the Secret Service did not answer the {what} within {seconds}s")))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(StoreError(format!("Secret Service: the {what} ended without an answer"))),
        }
    }
}

impl KeyStore for SecretServiceStore {
    fn kind(&self) -> &'static str {
        "Secret Service"
    }

    fn load(&self) -> Result<Option<Credential>, StoreError> {
        let attributes = self.attributes.clone();
        let value = self.run("read", move |bus| {
            let Some(collection) = default_collection(bus)? else {
                return Ok(None);
            };
            let items: Vec<OwnedObjectPath> = call(bus, collection.as_str(), COLLECTION_INTERFACE, "SearchItems", &(attributes,))?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            let session = open_session(bus)?;
            let (_, _, value, _): Secret = call(bus, item.as_str(), ITEM_INTERFACE, "GetSecret", &(session,))?;
            Ok(Some(value))
        })?;
        value
            .map(|value| serde_json::from_slice(&value).map_err(|error| StoreError(format!("the Secret Service item is not a credential: {error}"))))
            .transpose()
    }

    fn save(&self, credential: &Credential) -> Result<(), StoreError> {
        let value = serde_json::to_vec(credential).map_err(|error| StoreError(error.to_string()))?;
        let (attributes, label) = (self.attributes.clone(), self.label.clone());
        self.run("write", move |bus| {
            let collection =
                default_collection(bus)?.ok_or("there is no default collection, and making one would ask for a password: not making one")?;
            let session = open_session(bus)?;
            let properties = HashMap::from([
                ("org.freedesktop.Secret.Item.Label", Value::from(label)),
                ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes)),
            ]);
            let secret: Secret = (session, Vec::new(), value, "application/json".into());
            // Replacing the item with the same attributes, if there is one.
            let (item, _prompt): (OwnedObjectPath, OwnedObjectPath) =
                call(bus, collection.as_str(), COLLECTION_INTERFACE, "CreateItem", &(properties, secret, true))?;
            if item.as_str() == NONE {
                return Err("the Secret Service would ask first: not asking".into());
            }
            Ok(())
        })
    }

    fn delete(&self) -> Result<(), StoreError> {
        let attributes = self.attributes.clone();
        self.run("removal", move |bus| {
            let Some(collection) = default_collection(bus)? else {
                return Ok(());
            };
            let items: Vec<OwnedObjectPath> = call(bus, collection.as_str(), COLLECTION_INTERFACE, "SearchItems", &(attributes,))?;
            for item in items {
                let prompt: OwnedObjectPath = call(bus, item.as_str(), ITEM_INTERFACE, "Delete", &())?;
                if prompt.as_str() != NONE {
                    return Err("the Secret Service would ask before removing the item: not asking".into());
                }
            }
            Ok(())
        })
    }
}

/// The session bus (or the one at `address`), whose calls give up too, so
/// that one the store stopped waiting for does end — after twice `deadline`,
/// never before the store's own wait does: that is what tells it to wait no
/// more this run.
fn connect(address: Option<&str>, deadline: Duration) -> Result<Connection, String> {
    match address {
        Some(address) => Builder::address(address),
        None => Builder::session(),
    }
    .and_then(|builder| builder.method_timeout(deadline * 2).build())
    .map_err(|error| format!("no session bus: {error}"))
}

/// The default collection, unlocked; `None` when there is none.
fn default_collection(bus: &Connection) -> Result<Option<OwnedObjectPath>, String> {
    let collection: OwnedObjectPath = call(bus, SERVICE_PATH, SERVICE_INTERFACE, "ReadAlias", &("default",))?;
    if collection.as_str() == NONE {
        return Ok(None);
    }
    let locked: OwnedValue = call(
        bus,
        collection.as_str(),
        "org.freedesktop.DBus.Properties",
        "Get",
        &(COLLECTION_INTERFACE, "Locked"),
    )?;
    if bool::try_from(locked).map_err(|error| format!("the default collection's Locked: {error}"))? {
        return Err("the default collection is locked, and unlocking it would ask for a password: not unlocking it".into());
    }
    Ok(Some(collection))
}

/// A session for the `plain` algorithm. It ends with the connection.
fn open_session(bus: &Connection) -> Result<OwnedObjectPath, String> {
    let (_, session): (OwnedValue, OwnedObjectPath) = call(bus, SERVICE_PATH, SERVICE_INTERFACE, "OpenSession", &("plain", Value::from("")))?;
    Ok(session)
}

/// Call `method` on the Secret Service's object at `path`, and read the reply.
fn call<R>(bus: &Connection, path: &str, interface: &str, method: &str, body: &(impl serde::Serialize + DynamicType)) -> Result<R, String>
where
    R: for<'d> DynamicDeserialize<'d>,
{
    let reply = bus
        .call_method(Some(SERVICE), path, Some(interface), method, body)
        .map_err(|error| match &error {
            zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => {
                "no Secret Service on the session bus".to_string()
            }
            _ => format!("{method}: {error}"),
        })?;
    reply.body().deserialize().map_err(|error| format!("{method}'s answer: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn credential() -> Credential {
        Credential {
            username: "droppen-app".into(),
            password: "a-password-for-tests".into(),
        }
    }

    #[test]
    fn the_item_is_this_builds_own() {
        let store = SecretServiceStore::for_instance(crate::INSTANCE);
        let (service, account, label) = if cfg!(feature = "e2e") {
            (
                "app.droppen.desktop.e2e",
                "local-login:droppen-app-e2e",
                "Droppen E2E local login (droppen-app-e2e)",
            )
        } else {
            ("app.droppen.desktop", "local-login:droppen-app", "Droppen local login (droppen-app)")
        };
        assert_eq!(store.attributes.len(), 2);
        assert_eq!(store.attributes["service"], service);
        assert_eq!(store.attributes["account"], account);
        assert_eq!(store.label, label);
        assert_eq!(store.address, None, "the user's session bus");
    }

    #[test]
    fn no_session_bus_is_an_error_at_once() {
        let root = tempfile::tempdir().expect("tempdir");
        let address = format!("unix:path={}", root.path().join("no-bus").display());
        let store = SecretServiceStore::on_bus(Some(address), "test", DEADLINE);
        let started = Instant::now();
        assert!(store.load().expect_err("no bus").0.contains("no session bus"));
        assert!(store.save(&credential()).is_err());
        assert!(store.delete().is_err());
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn a_bus_that_never_answers_is_given_up_on_after_the_deadline() {
        // Takes the connection and says nothing, not even to the handshake.
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("silent-bus");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let address = format!("unix:path={}", path.display());
        let store = SecretServiceStore::on_bus(Some(address), "test", Duration::from_millis(300));
        let started = Instant::now();
        let error = store.load().expect_err("no answer");
        assert!(error.0.contains("did not answer the read"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        // The rest of the run does not wait again.
        let again = Instant::now();
        assert!(store.save(&credential()).expect_err("stalled").0.contains("earlier in this run"));
        assert!(store.delete().is_err());
        assert!(again.elapsed() < Duration::from_millis(100), "{:?}", again.elapsed());
        drop(listener);
    }

    /// Against a real Secret Service on a throwaway session bus — never the
    /// user's: `DROPPEN_TEST_SECRET_SERVICE_BUS` names its address, e.g.
    /// `dbus-run-session -- sh -c 'echo -n pw | gnome-keyring-daemon --unlock
    /// --components=secrets; DROPPEN_TEST_SECRET_SERVICE_BUS=$DBUS_SESSION_BUS_ADDRESS
    /// cargo test -p droppen-desktop-core -- --ignored --exact credentials::secret_service::tests::a_real_secret_service_keeps_the_login'`.
    #[test]
    #[ignore = "needs a Secret Service on a throwaway session bus"]
    fn a_real_secret_service_keeps_the_login() {
        let address = std::env::var("DROPPEN_TEST_SECRET_SERVICE_BUS").expect("DROPPEN_TEST_SECRET_SERVICE_BUS: a throwaway session bus");
        let store = SecretServiceStore::on_bus(Some(address), "secret-service-test", DEADLINE);
        store.delete().expect("start clean");
        assert_eq!(store.load().expect("load"), None);
        store.save(&credential()).expect("save");
        assert_eq!(store.load().expect("load"), Some(credential()));
        let newer = Credential {
            password: "a-newer-password".into(),
            ..credential()
        };
        store.save(&newer).expect("replace");
        assert_eq!(store.load().expect("load"), Some(newer), "replaced, not added beside");
        store.delete().expect("delete");
        assert_eq!(store.load().expect("load"), None);
        store.delete().expect("deleting nothing is fine");
    }
}
