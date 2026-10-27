//! The account store: `<instance>/state.db`'s `users` table, mirrored to
//! `<instance>/users.json`.
//!
//! **Per-instance.** Two instances on one machine — a server for the machine's
//! owner and one handed to somebody else — must not share a door: with one
//! account set, the `user add` that puts a password on the second server puts
//! the owner's server behind it too, and the two can never have different
//! logins. Be exact about what that buys. Both servers run as the same
//! operating-system user, so either account can still read every file the
//! other can: this is **not** a sandbox and nothing here should ever be sold as
//! one — isolation needs two Unix accounts or two machines. What it buys is two
//! people, two passwords, either one revocable without touching the other, and
//! a server whose door is closed by *its own* operator.
//!
//! Sessions — the *tokens* — are per-instance too; see [`super::sessions`].
//!
//! Passwords are stored as argon2id PHC strings. The cost parameters are named
//! below rather than taken from `Params::DEFAULT`, because a login has to tell a
//! hash made at cheaper parameters from one made at these and re-hash on the way
//! past; a moving default would make that comparison meaningless.
//!
//! # The table is the store
//!
//! In [`crate::db`], at `<instance>/state.db` — the same database the runs, the
//! layouts and the schedule are in, so an account and this instance's state are
//! one file and one `fsync`. `remote-hand-server user add` runs **while a server
//! is up**, deliberately (see [`super::cli`]), and takes no lock the server
//! respects. Two processes cannot be serialised by a mutex private to each of
//! them; they can by SQLite's `busy_timeout` and a transaction. A file both
//! read-modify-wrote would let whoever renames last drop the other's edit
//! without a word — a password change that silently did not happen.
//!
//! # What an instance starts with
//!
//! Only its own accounts. It does **not** inherit the machine-wide set builds
//! before 0.2.37 kept (`<config>/state.db`'s `users` table, and
//! `<config>/users.json` beside it): an install that ran 0.2.37 or later has
//! already copied it into its instance, and one older than that is below the
//! 0.2.125 upgrade floor. Such an install, or a new instance, starts with no user
//! account and is in its setup state until the first one exists
//! ([`super::setup`]). That is fail-closed, not authentication off: no API route
//! opens, and the ways in are `droppen user add`, `droppen open` or the setup
//! code.
//!
//! Nothing here reads, writes or deletes the machine-wide set: the schema is
//! additive-only, it is the user's data, and it is a manual recovery source.
//! Every release up to 0.2.133 still seeds from it, so the supervisor's `.prev`
//! does too, but only into an instance without [`SEED_MARKER`]. The accepted
//! cost: an account revoked on an instance stays in that set, so such a build
//! can put it back into an instance whose marker was lost with its `state.db`;
//! with nothing to seed, that instance would be in its setup state instead.
//!
//! # App-internal accounts
//!
//! The Droppen desktop app signs its own window in with an account it makes
//! with a random password ([`UsersStore::add_app`]). That account is marked
//! app-internal, in the stamps payload beside the row, and is not a *user*
//! account: [`UsersStore::has_users`] does not count it, so it does not end the
//! setup state, and a person on that device can still open the page as its
//! owner (`remote-hand-server open`) or make the first account of their own. A
//! binary that predates the mark reads the row and not the mark, and takes it
//! for an ordinary account: the fail-closed direction.
//!
//! # Why this store, alone, still writes a JSON file
//!
//! The mirror is a **second opinion**, and [`UsersStore::has_users`] and
//! [`UsersStore::is_empty`] are its only readers after the one-time import.
//! Everywhere else in this server an empty table means an empty store. Here it
//! means *authentication is switched off*, on a server that may be published to
//! the internet through a tunnel, and the two things that could empty it — a
//! database set aside as corrupt, a restore that brought back the wrong file —
//! both look exactly like
//! "the operator has not configured any accounts yet". So a table that says
//! nobody is on file is believed only when `users.json` agrees. Other stores have
//! no JSON mirror; do not drop this one to match them.
//!
//! It is written on `user add`, `user passwd` and `user rm` — a handful of times
//! in a machine's life — plus once more when the store finds accounts in the
//! database and no file, so the second opinion survives somebody deleting it. A
//! failure to write it is logged rather than returned: the account exists
//! either way, and refusing the operation would not put the file back.
//!
//! The failure this buys is a lockout rather than an exposure: an account in the
//! file with no row behind it keeps the door shut and says so loudly.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::{self, Db};
use crate::sessions::{iso8601, Paths};

/// Memory cost, in KiB. 64 MiB — the OWASP baseline for argon2id at `t=3`.
pub const MEMORY_KIB: u32 = 65536;
/// Time cost (passes).
pub const TIME_COST: u32 = 3;
/// Lanes. One: the verify already runs on a blocking thread and is serialized
/// against every other verify, so extra lanes buy latency at the cost of cores
/// this server would rather spend on the CLIs it drives.
pub const PARALLELISM: u32 = 1;

/// Shortest accepted password, in **code points** rather than bytes, so a
/// passphrase in a non-Latin script is not held to a stricter rule than the same
/// number of ASCII characters would be.
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Longest accepted password, in bytes. argon2 has no practical input limit; this
/// is only here so an accidental paste of a file cannot become a hashing job.
pub const MAX_PASSWORD_BYTES: usize = 256;
/// Longest accepted user name.
pub const MAX_USERNAME: usize = 32;

/// The name this store's own JSON file goes by in `kv`'s record of what has
/// already been drained. Also the scope its rows are staged in on the way
/// through; see [`UsersStore::ensure_imported`].
const IMPORT_MARKER: &str = "users.json";

/// The marker of the machine-wide *seed*: written — once, with no rows — the
/// first time an instance's store is opened, though this store never seeds.
///
/// A binary that still seeds (the supervisor's `.prev`, or an image-baked
/// build) copies the machine-wide accounts into any instance that lacks this
/// marker; with it, an instance that deliberately started without them stays
/// that way. Not a path, so it cannot be confused with one: nothing is drained
/// from a file called this.
const SEED_MARKER: &str = "machine-wide-users";

/// Where the two ISO stamps live, keyed by user name.
///
/// `users` is `(name, hash, created_at)` and [`crate::db::SCHEMA`] is
/// append-only, so `updatedAt` — a *password* event, not a creation one — has no
/// column to go in and never will have one that this build could rely on. It
/// goes in `kv` beside the row, written in the same transaction, which is the
/// column-versus-payload split the schema note draws rather than a workaround:
/// the column is the part a query could ever want, the payload is the meaning.
/// Verbatim rather than derived from the column, because these two strings are
/// what `user list` prints and what `GET /api/auth/users` returns, and a
/// round-trip through whole seconds would move a time somebody had read.
const STAMPS_SCOPE: &str = "users";

/// Passwords that pass the length rule and are still the first thing anyone
/// guesses. Not a dictionary — a dictionary belongs in a separate data file and
/// would still be a rounding error against a real cracking list. What this stops
/// is the specific failure of a twelve-character rule being satisfied by
/// `123456789012`, which is the shape a hurried person reaches for.
const WORST_PASSWORDS: &[&str] = &[
    "123456789012",
    "1234567890123",
    "123456789abc",
    "111111111111",
    "000000000000",
    "121212121212",
    "123123123123",
    "abcdefghijkl",
    "qwertyuiopas",
    "qwertyuiop12",
    "1qaz2wsx3edc",
    "qazwsxedcrfv",
    "zaq12wsxcde3",
    "password1234",
    "passwordpassword",
    "administrator",
    "adminadmin12",
    "letmein12345",
    "iloveyou1234",
    "welcome12345",
    "changeme1234",
    "monkey123456",
    "dragon123456",
    "sunshine1234",
    "football1234",
    "trustno112345",
    "remotehand123",
];

/// A pre-computed argon2id hash of a value nobody knows, at this build's cost
/// parameters.
///
/// Verified against when the named user does not exist or is locked out, so that
/// all three refusals — no such user, wrong password, locked — cost one argon2
/// verification and take the same wall-clock time. A `OnceLock` that hashed
/// something on first use would make the *first* refusal of a process visibly
/// cheaper than the rest, which is the leak this exists to close, so the string
/// is a constant. `the_fake_hash_is_a_usable_phc_string_at_current_cost` pins
/// that it still parses and still matches the parameters above.
const FAKE_PHC: &str = "$argon2id$v=19$m=65536,t=3,p=1$c2FsdHNhbHRzYWx0c2FsdA$P4RJDPnCPDrQCJDPZLYYBI6WVOsBSTiUAJgYqe4rTGw";

/// Why a user operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum UserError {
    /// The name or the password does not meet the rules.
    #[error("{0}")]
    Invalid(String),
    /// No user by that name.
    #[error("no such user: {0}")]
    NotFound(String),
    /// A user by that name is already on file.
    #[error("user already exists: {0}")]
    Duplicate(String),
    /// The store could not be read or written.
    #[error("{0}")]
    Io(String),
}

/// One account, as it sits on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredUser {
    /// The login name; unique, and always already lower-case (see [`valid_name`]).
    pub name: String,
    /// The argon2id PHC string. Never leaves this module.
    pub password_phc: String,
    /// When the account was created, ISO-8601.
    pub created_at: String,
    /// When the password was last changed, ISO-8601.
    pub updated_at: String,
    /// An app-internal account (see [`UsersStore::add_app`]). Absent — false —
    /// in every file an earlier build wrote.
    #[serde(default, skip_serializing_if = "is_false")]
    pub app: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// `users.json` as a whole.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersFile {
    /// Every account, in the order they were created.
    #[serde(default)]
    pub users: Vec<StoredUser>,
}

/// One account, with nothing secret in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    /// The login name.
    pub name: String,
    /// When the account was created, ISO-8601.
    pub created_at: String,
    /// When the password was last changed, ISO-8601.
    pub updated_at: String,
    /// Whether it is an app-internal account ([`UsersStore::add_app`]).
    pub app: bool,
}

/// The two stamps that have no column of their own; see [`STAMPS_SCOPE`] —
/// and, beside them, the one fact about an account that is not a time: whether
/// it is app-internal. In the payload rather than a column for the same reason
/// the stamps are: the schema is append-only, and nothing ever selects on it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stamps {
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default, skip_serializing_if = "is_false")]
    app: bool,
}

/// The account store, owning its database handle and its two locks.
///
/// Two `Mutex`es with different jobs. `write_lock` keeps the *mirror* honest
/// (the transaction already makes each change atomic, against the CLI's process
/// as well as this one): the mirror is a whole-file rewrite of everything, so
/// two writers that commit in one order and then mirror in the other would leave
/// `users.json` describing neither. It is held across the commit and the copy.
/// `verify_lock` serializes *hashing*: each argon2id verification allocates 64
/// MiB, so N concurrent logins would allocate N × 64 MiB. Holding it across the
/// hash caps that at one, and doubles as the crudest possible rate limit — a
/// caller cannot make the server hash faster than it hashes.
pub struct UsersStore {
    path: PathBuf,
    db: Arc<Db>,
    /// The import below runs once per process at most; the marker it leaves in
    /// the database makes it once per *instance*, which is the half that
    /// matters.
    imported: std::sync::Once,
    write_lock: tokio::sync::Mutex<()>,
    verify_lock: tokio::sync::Mutex<()>,
}

/// What a login attempt found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The password matched.
    Ok,
    /// It did not, or there is no such user. The caller must not tell them apart.
    No,
}

impl UsersStore {
    /// This instance's store: `<instance>/users.json` over `<instance>/state.db`.
    ///
    /// The server and the CLI both build one of these, which is what makes
    /// `user add --instance bob` and the server running instance `bob` the same
    /// account set.
    pub fn new(paths: &Paths) -> Self {
        Self::over(paths.instance_dir().join("users.json"), paths.instance_db())
    }

    /// A store over an explicit path.
    ///
    /// The database is the one beside the file, so this names an instance's
    /// store by pointing at `<instance>/users.json`, and the machine-wide store
    /// builds before 0.2.37 kept by pointing at `<config>/users.json`. Nothing is
    /// inherited either way: a store holds only the accounts in its own
    /// database.
    ///
    /// The CLI opening a file the server has open is left to SQLite, the only
    /// thing that can arbitrate it.
    pub fn at(path: PathBuf) -> Self {
        let db = path.parent().unwrap_or_else(|| Path::new(".")).join("state.db");
        Self::over(path, db)
    }

    fn over(path: PathBuf, db: PathBuf) -> Self {
        Self {
            path,
            db: db::at(db),
            imported: std::sync::Once::new(),
            write_lock: tokio::sync::Mutex::new(()),
            verify_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Where the mirror of the store lives. Still the file everything outside
    /// this module names the store by, including the CLI's own output.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Fill this instance's table, once ever, before the first read: from the
    /// file beside it.
    ///
    /// Two hops rather than one, because [`Db::import_once`] writes rows into
    /// `kv` and these rows belong in `users`. The first hop is the one that must
    /// not happen twice — it is the one that could import over an edit somebody
    /// has since made — and it is marked in the same transaction as the rows it
    /// writes. The second is a plain `INSERT OR IGNORE` and a `DELETE` in one
    /// transaction, so it is idempotent: a crash between the two hops leaves
    /// rows staged rather than lost, and the next boot finishes the job.
    ///
    /// What is left at the end is the mirror, when there are accounts and no
    /// file holding them — see the comment there for the failure that closes.
    fn ensure_imported(&self) {
        self.imported.call_once(|| {
            let path = self.path.clone();
            self.db.import_once(IMPORT_MARKER, || staged_from_json(&path));
            self.db.drain_staged_users(IMPORT_MARKER);

            // "Considered, nothing to copy": see [`SEED_MARKER`].
            self.db.import_once(SEED_MARKER, || Vec::new());

            // Accounts in the table and no file beside them is the one shape
            // that reads as "this server has no accounts" — an open API — if
            // the database is ever set aside as corrupt. An instance whose
            // mirror somebody deleted is exactly that shape, so the condition is
            // the shape rather than the cause. Written here rather than awaited,
            // because this runs inside a `Once` on the first read and `rows`
            // deliberately does not re-enter it.
            let rows = self.rows();
            if !rows.users.is_empty() && !self.path.exists() {
                write_mirror(&self.path, &rows);
            }
        });
    }

    /// Whether this instance has no accounts at all, app-internal ones
    /// included. The guard asks [`UsersStore::has_users`] instead; this is for
    /// the CLI's bookkeeping.
    ///
    /// The one read in this file that does not trust the database on its own.
    /// "No accounts" is not an ordinary answer here — it switches authentication
    /// off on a server published to the internet — so when the database says
    /// nobody is on file the mirror gets a vote too, and an account in the file
    /// with no row behind it keeps the door shut while somebody works out why.
    /// It cannot fire in the ordinary course of events: the import puts every
    /// user in the file into the table, and every write updates both. A file
    /// that does not parse still reads as no accounts: this module's deliberate
    /// fail-open.
    pub async fn is_empty(&self) -> bool {
        if self.count() > 0 {
            return false;
        }
        let mirrored = read_mirror(&self.path).users.len();
        if mirrored > 0 {
            tracing::error!(
                path = %self.path.display(),
                database = %self.db.path().display(),
                accounts_in_the_file = mirrored,
                "the accounts file has accounts the state database does not; keeping authentication on rather than opening the API"
            );
            return false;
        }
        true
    }

    /// Whether a user by this name is on file.
    pub async fn exists(&self, name: &str) -> bool {
        self.ensure_imported();
        self.db
            .read(|conn| {
                conn.query_row("SELECT 1 FROM users WHERE name = ?1", (name,), |row| row.get::<_, i64>(0))
                    .optional()
            })
            .unwrap_or(None)
            .is_some()
    }

    /// Every account, newest last, with nothing secret in it.
    pub async fn list(&self) -> Vec<UserSummary> {
        self.snapshot()
            .users
            .into_iter()
            .map(|user| UserSummary {
                name: user.name,
                created_at: user.created_at,
                updated_at: user.updated_at,
                app: user.app,
            })
            .collect()
    }

    /// How many accounts there are, and zero when the question cannot be asked.
    fn count(&self) -> i64 {
        self.ensure_imported();
        self.db
            .read(|conn| conn.query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0)))
            .unwrap_or_else(|error| {
                tracing::error!(database = %self.db.path().display(), %error, "cannot count the accounts");
                0
            })
    }

    /// Every row, in the order the file kept them.
    ///
    /// `ORDER BY rowid`, because the JSON this mirrors is a list in creation
    /// order, [`UsersStore::list`] promises "newest last", and a rowid is the
    /// insertion order. An `UPDATE` keeps its rowid, which is what leaves a
    /// password change where it was in the list.
    fn snapshot(&self) -> UsersFile {
        self.ensure_imported();
        self.rows()
    }

    /// [`UsersStore::snapshot`] without the import, for the one caller that is
    /// *inside* the import: [`UsersStore::ensure_imported`] mirrors what the
    /// database holds, and a `std::sync::Once` re-entered from its own closure
    /// deadlocks.
    fn rows(&self) -> UsersFile {
        let users = self
            .db
            .read(|conn| {
                let mut statement = conn.prepare(
                    "SELECT u.name, u.hash, u.created_at, k.value
                       FROM users u
                       LEFT JOIN kv k ON k.scope = ?1 AND k.key = u.name
                      ORDER BY u.rowid",
                )?;
                let rows = statement.query_map((STAMPS_SCOPE,), |row| {
                    Ok(user_from_row(row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?;
                Ok(rows.filter_map(Result::ok).collect::<Vec<_>>())
            })
            .unwrap_or_else(|error| {
                tracing::error!(database = %self.db.path().display(), %error, "cannot read the accounts out of the state database");
                Vec::new()
            });
        UsersFile { users }
    }

    /// Write `users.json` from what the database now says, and carry on if it
    /// cannot be written.
    ///
    /// Called with `write_lock` held, always after the commit and never before:
    /// a mirror written first would describe a change that then failed. On a
    /// blocking thread, because the write itself is synchronous — the repair
    /// path in [`UsersStore::ensure_imported`] has no `await` available and one
    /// implementation of the `0600` rule is worth a `spawn_blocking` here.
    async fn mirror(&self) {
        let file = self.snapshot();
        let path = self.path.clone();
        let _ = tokio::task::spawn_blocking(move || write_mirror(&path, &file)).await;
    }

    /// Create an account.
    ///
    /// The name is the primary key, so "already exists" is not a check followed
    /// by an insert — it *is* the insert: two `user add`s of one name in two
    /// processes produce one account and one refusal, never two accounts of
    /// which one vanishes.
    pub async fn add(&self, name: &str, password: &str) -> Result<(), UserError> {
        self.add_kind(name, password, false).await
    }

    /// Create an **app-internal** account: the Droppen desktop app's own login
    /// to the server it runs, with a random password nobody is shown.
    ///
    /// It logs in like any account, and is not a *user* account: it does not
    /// count towards [`UsersStore::has_users`] — so it does not end the setup
    /// state — `user list` leaves it out, its password is not changed through
    /// the API, and it is nobody's way to other devices on its own
    /// (`crate::api::cloud`).
    pub async fn add_app(&self, name: &str, password: &str) -> Result<(), UserError> {
        self.add_kind(name, password, true).await
    }

    /// Hold every write to the store until the guard goes: for a test that
    /// needs a request caught between its check and its write.
    #[cfg(test)]
    pub(crate) async fn hold_writes(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().await
    }

    async fn add_kind(&self, name: &str, password: &str, app: bool) -> Result<(), UserError> {
        let name = check_name(name)?;
        check_password(&name, password)?;
        let phc = hash_password(password)?;
        let _guard = self.write_lock.lock().await;
        self.ensure_imported();
        let now = iso8601(SystemTime::now());
        let stamps = Stamps {
            created_at: now.clone(),
            updated_at: now,
            app,
        };
        self.db
            .write(|tx| {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO users (name, hash, created_at) VALUES (?1, ?2, ?3)",
                    (&name, &phc, epoch_seconds(&stamps.created_at)),
                )?;
                if inserted == 0 {
                    return Ok(Err(UserError::Duplicate(name.clone())));
                }
                tx.write_user_stamps(&name, &stamps)?;
                Ok(Ok(()))
            })
            .map_err(store_error)??;
        self.mirror().await;
        Ok(())
    }

    /// Replace an account's password.
    pub async fn set_password(&self, name: &str, password: &str) -> Result<(), UserError> {
        let name = check_name(name)?;
        check_password(&name, password)?;
        let phc = hash_password(password)?;
        let _guard = self.write_lock.lock().await;
        self.ensure_imported();
        let now = iso8601(SystemTime::now());
        self.db
            .write(|tx| {
                let existing: Option<String> = tx
                    .query_row("SELECT value FROM kv WHERE scope = ?1 AND key = ?2", (STAMPS_SCOPE, &name), |row| {
                        row.get(0)
                    })
                    .optional()?;
                if tx.execute("UPDATE users SET hash = ?2 WHERE name = ?1", (&name, &phc))? == 0 {
                    return Ok(Err(UserError::NotFound(name.clone())));
                }
                let mut stamps = existing.and_then(|raw| serde_json::from_str::<Stamps>(&raw).ok()).unwrap_or_default();
                // A row whose stamps went missing is not a new account: it keeps
                // whatever creation time it has, and only acquires today's if
                // there is nothing at all to keep.
                if stamps.created_at.is_empty() {
                    stamps.created_at = now.clone();
                }
                stamps.updated_at = now.clone();
                tx.write_user_stamps(&name, &stamps)?;
                Ok(Ok(()))
            })
            .map_err(store_error)??;
        self.mirror().await;
        Ok(())
    }

    /// Make an existing account app-internal ([`UsersStore::add_app`]),
    /// password and all.
    ///
    /// For an install whose desktop-app login is an ordinary account, made by a
    /// build without the mark: the app marks it on its next start, and the
    /// instance is left with the user accounts it really has.
    pub async fn mark_app(&self, name: &str) -> Result<(), UserError> {
        let name = check_name(name)?;
        let _guard = self.write_lock.lock().await;
        self.ensure_imported();
        self.db
            .write(|tx| {
                let exists: Option<i64> = tx
                    .query_row("SELECT 1 FROM users WHERE name = ?1", (&name,), |row| row.get(0))
                    .optional()?;
                if exists.is_none() {
                    return Ok(Err(UserError::NotFound(name.clone())));
                }
                let existing: Option<String> = tx
                    .query_row("SELECT value FROM kv WHERE scope = ?1 AND key = ?2", (STAMPS_SCOPE, &name), |row| {
                        row.get(0)
                    })
                    .optional()?;
                let mut stamps = existing.and_then(|raw| serde_json::from_str::<Stamps>(&raw).ok()).unwrap_or_default();
                stamps.app = true;
                tx.write_user_stamps(&name, &stamps)?;
                Ok(Ok(()))
            })
            .map_err(store_error)??;
        self.mirror().await;
        Ok(())
    }

    /// Whether the account by this name is app-internal.
    pub async fn is_app(&self, name: &str) -> bool {
        self.snapshot().users.iter().any(|user| user.name == name && user.app)
    }

    /// Whether this instance has at least one **user** account — one that is
    /// not app-internal. With none the instance is in its setup state
    /// ([`super::Auth::setup_pending`]), and a local-owner session is valid
    /// ([`super::Auth::still_valid`]).
    ///
    /// The second-opinion rule of [`UsersStore::is_empty`] holds here too: the
    /// database saying "none" is believed only when the mirror agrees, so a
    /// state database set aside as corrupt keeps the door shut rather than
    /// opening it. A row whose payload is missing or unreadable counts as a
    /// user account — the direction an error is allowed to point in.
    pub async fn has_users(&self) -> bool {
        if self.snapshot().users.iter().any(|user| !user.app) {
            return true;
        }
        let mirrored = read_mirror(&self.path).users.iter().filter(|user| !user.app).count();
        if mirrored > 0 {
            tracing::error!(
                path = %self.path.display(),
                database = %self.db.path().display(),
                accounts_in_the_file = mirrored,
                "the accounts file has accounts the state database does not; keeping authentication on rather than opening the API"
            );
            return true;
        }
        false
    }

    /// Delete an account, from this instance.
    ///
    /// Its live sessions on this instance die with it: every token check
    /// re-confirms that the name is still on file, so a removed user cannot be
    /// resurrected by a token that was minted before the removal. Another
    /// instance's account of the same name is a different account and is not
    /// touched — that is what per-instance means, and why [`super::cli`]
    /// revokes this instance's sessions only.
    ///
    /// It does not reach the machine-wide set on disk either; see the module
    /// note on what a build that still seeds can put back.
    pub async fn remove(&self, name: &str) -> Result<(), UserError> {
        let name = check_name(name)?;
        let _guard = self.write_lock.lock().await;
        self.ensure_imported();
        self.db
            .write(|tx| {
                if tx.execute("DELETE FROM users WHERE name = ?1", (&name,))? == 0 {
                    return Ok(Err(UserError::NotFound(name.clone())));
                }
                crate::db::kv_remove_in(tx, STAMPS_SCOPE, &name)?;
                Ok(Ok(()))
            })
            .map_err(store_error)??;
        self.mirror().await;
        Ok(())
    }

    /// Check a password, in constant *shape*: an unknown name costs one argon2id
    /// verification against [`FAKE_PHC`], exactly as a wrong password costs one
    /// against the real hash.
    ///
    /// A match whose stored hash is weaker than this build's parameters is
    /// re-hashed on the way past — the one moment the plaintext is in hand.
    pub async fn verify(&self, name: &str, password: &str) -> Verdict {
        let stored = self.hash_of(name);
        let phc = stored.clone().unwrap_or_else(|| FAKE_PHC.to_string());
        let matched = self.verify_hash(phc.clone(), password.to_string()).await;
        // `stored.is_none()` is checked *after* the hash, not instead of it.
        if stored.is_none() || !matched {
            return Verdict::No;
        }
        if needs_rehash(&phc) {
            if let Err(error) = self.set_password(name, password).await {
                tracing::warn!(%error, user = name, "could not re-hash a password at the current cost");
            }
        }
        Verdict::Ok
    }

    /// Burn one argon2id verification without there being anything to verify.
    ///
    /// The lockout path calls this: a locked account must not answer faster than
    /// an unlocked one, or the lockout itself becomes an oracle for which names
    /// exist.
    pub async fn burn_a_verification(&self) {
        let _ = self.verify_hash(FAKE_PHC.to_string(), String::new()).await;
    }

    /// One hash, on a blocking thread, one at a time.
    async fn verify_hash(&self, phc: String, password: String) -> bool {
        let _guard = self.verify_lock.lock().await;
        tokio::task::spawn_blocking(move || verify_sync(&phc, &password)).await.unwrap_or(false)
    }

    /// The stored PHC string for a name, whatever state it is in.
    ///
    /// Deliberately *not* filtered: a row whose hash is unparseable is returned
    /// as it stands, fails to verify like any wrong password, and — the point —
    /// still counts as an account. Dropping it as an unusable row would be the
    /// tolerant thing to do anywhere else and is the wrong thing here, because
    /// dropping the last one turns authentication off.
    fn hash_of(&self, name: &str) -> Option<String> {
        self.ensure_imported();
        self.db
            .read(|conn| {
                conn.query_row("SELECT hash FROM users WHERE name = ?1", (name,), |row| row.get::<_, String>(0))
                    .optional()
            })
            .unwrap_or(None)
    }
}

fn store_error(error: rusqlite::Error) -> UserError {
    UserError::Io(error.to_string())
}

/// The `created_at` column's value for an ISO stamp.
///
/// Seconds, matching the `unixepoch()` the column defaults to. Nothing selects
/// on it — the payload beside it is what the API reads back — so a stamp that
/// will not parse falls back to now rather than dropping an account, which is
/// the one outcome this file must never produce.
fn epoch_seconds(stamp: &str) -> i64 {
    crate::record::replay::parse_iso8601(stamp)
        .map(|millis| millis / 1_000)
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_secs() as i64)
                .unwrap_or_default()
        })
}

/// The stamps payload, written beside a `users` row in the same transaction.
trait UserStampsExt {
    fn write_user_stamps(&self, name: &str, stamps: &Stamps) -> rusqlite::Result<()>;
}

impl UserStampsExt for rusqlite::Transaction<'_> {
    fn write_user_stamps(&self, name: &str, stamps: &Stamps) -> rusqlite::Result<()> {
        let value = serde_json::to_string(stamps).unwrap_or_else(|_| "{}".to_string());
        crate::db::kv_set_in(self, STAMPS_SCOPE, name, &value)
    }
}

/// One row, plus whatever of its payload survived, as the file's shape.
///
/// A missing or unreadable payload is not a missing account: both stamps fall
/// back to the column, which is a second's worth of the truth rather than none
/// of it.
fn user_from_row(name: String, hash: String, created_at: i64, payload: Option<String>) -> StoredUser {
    let stamps = payload.and_then(|raw| serde_json::from_str::<Stamps>(&raw).ok()).unwrap_or_default();
    let from_column = || iso8601(UNIX_EPOCH + Duration::from_secs(created_at.max(0) as u64));
    let created = if stamps.created_at.is_empty() {
        from_column()
    } else {
        stamps.created_at
    };
    let updated = if stamps.updated_at.is_empty() {
        created.clone()
    } else {
        stamps.updated_at
    };
    StoredUser {
        name,
        password_phc: hash,
        created_at: created,
        updated_at: updated,
        app: stamps.app,
    }
}

/// Read a mirror — this instance's, usually — tolerating everything.
///
/// Synchronous because every caller is: the import runs once inside a `Once`,
/// and [`UsersStore::is_empty`]'s safety net only reaches the file when the
/// database has already said there is nothing — on the overwhelmingly common
/// path that is one failed `open` of a file that is not there.
fn read_mirror(path: &Path) -> UsersFile {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<UsersFile>(&raw).unwrap_or_default(),
        Err(_) => UsersFile::default(),
    }
}

/// The rows [`Db::import_once`] stages, keyed by name.
fn staged_from_json(path: &Path) -> Vec<(String, String)> {
    read_mirror(path)
        .users
        .into_iter()
        .filter(|user| !user.name.is_empty())
        .filter_map(|user| Some((user.name.clone(), serde_json::to_string(&user).ok()?)))
        .collect()
}

/// Write the mirror `0600`, through a tmp file, and carry on if it cannot be
/// written.
///
/// Synchronous: [`UsersStore::mirror`] hands it to a blocking thread and the
/// repair in [`UsersStore::ensure_imported`] calls it where there is no `await`.
fn write_mirror(path: &Path, file: &UsersFile) {
    if let Some(dir) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(dir) {
            tracing::error!(path = %dir.display(), %error, "cannot make the directory the accounts file lives in");
            return;
        }
    }
    let value = serde_json::to_value(file).unwrap_or(serde_json::Value::Null);
    if let Err(error) = super::write_json_private(path, &value) {
        tracing::error!(
            path = %path.display(),
            %error,
            "cannot mirror the accounts to JSON; a state database lost after this would read as an instance with no accounts and open its API"
        );
    }
}

impl Db {
    /// Move whatever is staged into `users`, and forget it was ever staged.
    ///
    /// `INSERT OR IGNORE` rather than `REPLACE`, for [`Db::import_once`]'s
    /// reason: a row that is already there was written by this build and is
    /// newer than a file this build has never read from twice — a crash
    /// between staging and draining must not let the older copy overwrite an
    /// edit made since. Ordered by creation stamp because `kv` hands rows back
    /// by key — alphabetically by name — and the order the rowids are handed
    /// out in is the order the list will come back in for ever after.
    ///
    /// Answers with how many accounts it moved, which is zero on every boot
    /// after the first.
    fn drain_staged_users(&self, scope: &str) -> usize {
        let staged: Vec<String> = self
            .read(|conn| {
                let mut statement = conn.prepare("SELECT value FROM kv WHERE scope = ?1")?;
                let rows = statement.query_map((scope,), |row| row.get::<_, String>(0))?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default();
        if staged.is_empty() {
            return 0;
        }
        let mut users: Vec<StoredUser> = staged.iter().filter_map(|raw| serde_json::from_str(raw).ok()).collect();
        users.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        self.commit("the imported accounts", |tx| {
            for user in &users {
                tx.execute(
                    "INSERT OR IGNORE INTO users (name, hash, created_at) VALUES (?1, ?2, ?3)",
                    (&user.name, &user.password_phc, epoch_seconds(&user.created_at)),
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO kv (scope, key, value) VALUES (?1, ?2, ?3)",
                    (
                        STAMPS_SCOPE,
                        &user.name,
                        serde_json::to_string(&Stamps {
                            created_at: user.created_at.clone(),
                            updated_at: user.updated_at.clone(),
                            app: user.app,
                        })
                        .unwrap_or_else(|_| "{}".to_string()),
                    ),
                )?;
            }
            tx.execute("DELETE FROM kv WHERE scope = ?1", (scope,))?;
            Ok(())
        });
        users.len()
    }
}

/// This build's argon2id, at the parameters named at the top of the module.
fn argon2() -> Result<Argon2<'static>, UserError> {
    let params = Params::new(MEMORY_KIB, TIME_COST, PARALLELISM, None).map_err(|error| UserError::Io(error.to_string()))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hash a password into a PHC string, salt and all.
pub fn hash_password(password: &str) -> Result<String, UserError> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2()?
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| UserError::Io(error.to_string()))?;
    Ok(hash.to_string())
}

/// Verify against a PHC string. The cost parameters come from the *hash*, which
/// is what lets an older, cheaper hash still be checked before it is replaced.
fn verify_sync(phc: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

/// Whether a stored hash is cheaper than what this build would produce.
///
/// Only ever upgrades. A hash made by a *newer*, more expensive build is left
/// alone: rolling a deployment back should not quietly weaken the file it shares
/// with the build it was rolled back from.
fn needs_rehash(phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return true;
    };
    if parsed.algorithm.as_str() != Algorithm::Argon2id.ident().as_str() {
        return true;
    }
    let Ok(params) = Params::try_from(&parsed) else {
        return true;
    };
    params.m_cost() < MEMORY_KIB || params.t_cost() < TIME_COST || params.p_cost() < PARALLELISM
}

/// Whether `name` is a legal user name: `[a-z0-9._-]{1,32}`.
///
/// Refused rather than sanitised, in the spirit of
/// [`crate::instance::valid_name`]: a name that is silently lower-cased is a name
/// the person who typed it cannot find again, and one that is silently stripped
/// is two accounts that collapse into one.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_USERNAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-')
}

fn check_name(name: &str) -> Result<String, UserError> {
    if valid_name(name) {
        Ok(name.to_string())
    } else {
        Err(UserError::Invalid(format!(
            "a user name is 1-{MAX_USERNAME} characters of a-z, 0-9, '.', '_' or '-': {name:?}"
        )))
    }
}

/// The password rules, all of them.
///
/// No character-class rule on purpose: "one upper, one digit, one symbol" pushes
/// people towards `Password1!`, which is shorter *and* more guessable than the
/// twelve characters this asks for instead. Length plus a list of the answers
/// everyone gives is the whole policy.
pub fn check_password(name: &str, password: &str) -> Result<(), UserError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(UserError::Invalid(format!("a password is at least {MIN_PASSWORD_CHARS} characters long")));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(UserError::Invalid(format!("a password is at most {MAX_PASSWORD_BYTES} bytes long")));
    }
    // Trimmed rather than rejected is how a password becomes un-typeable
    // somewhere else: the terminal that ate the space and the browser that kept
    // it would then disagree about what the password is.
    if password.trim() != password {
        return Err(UserError::Invalid("a password may not start or end with whitespace".into()));
    }
    if password.eq_ignore_ascii_case(name) {
        return Err(UserError::Invalid("a password may not be the user name".into()));
    }
    let lowered = password.to_lowercase();
    if WORST_PASSWORDS.contains(&lowered.as_str()) {
        return Err(UserError::Invalid("that password is one of the most common ones; pick another".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, UsersStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = UsersStore::at(dir.path().join("users.json"));
        (dir, store)
    }

    /// The store an instance's server and its CLI both build, over a home
    /// directory the test owns.
    fn instance(home: &Path, name: &str) -> UsersStore {
        UsersStore::new(&Paths::for_instance(home.to_path_buf(), name))
    }

    /// The machine-wide store builds before 0.2.37 kept: the table in
    /// `<config>/state.db`, mirrored to `<config>/users.json`.
    fn machine_wide(home: &Path) -> UsersStore {
        UsersStore::at(home.join(".config").join("remote-hand").join("users.json"))
    }

    /// The whole timing-equalization story rests on this string being a real
    /// hash at the real cost. A typo in it would turn every unknown-user login
    /// into an instant refusal and hand an attacker a user enumeration oracle.
    #[test]
    fn the_fake_hash_is_a_usable_phc_string_at_current_cost() {
        let parsed = PasswordHash::new(FAKE_PHC).expect("the fake hash parses");
        assert_eq!(parsed.algorithm.as_str(), "argon2id");
        let params = Params::try_from(&parsed).expect("params");
        assert_eq!((params.m_cost(), params.t_cost(), params.p_cost()), (MEMORY_KIB, TIME_COST, PARALLELISM));
        assert!(!needs_rehash(FAKE_PHC), "the fake hash must not look stale to the login path");
    }

    #[test]
    fn a_name_is_refused_rather_than_sanitised() {
        assert!(valid_name("ada"));
        assert!(valid_name("ada.lovelace_1-x"));
        assert!(!valid_name(""));
        assert!(!valid_name("Ada"), "an upper-case name would collide case-insensitively");
        assert!(!valid_name("ada lovelace"));
        assert!(!valid_name("../etc"));
        assert!(!valid_name(&"a".repeat(MAX_USERNAME + 1)));
    }

    #[test]
    fn the_password_rules_each_have_their_own_refusal() {
        assert!(check_password("ada", "correct horse battery").is_ok());
        assert!(check_password("ada", "short").is_err(), "under twelve characters");
        assert!(
            check_password("ada", &"x".repeat(MAX_PASSWORD_BYTES + 1)).is_err(),
            "over the byte ceiling"
        );
        assert!(check_password("ada", " leading space pw").is_err());
        assert!(check_password("ada", "trailing space ").is_err());
        assert!(check_password("averylongusername", "AVeryLongUsername").is_err(), "the name, in any case");
        assert!(check_password("ada", "123456789012").is_err(), "on the worst-password list");
        assert!(check_password("ada", "PASSWORD1234").is_err(), "the list is case-insensitive");
    }

    /// Twelve *code points*, not twelve bytes: a four-character CJK passphrase is
    /// twelve bytes in UTF-8 and would otherwise sail past the rule.
    #[test]
    fn the_length_rule_counts_characters_not_bytes() {
        assert!(check_password("ada", "密码密码").is_err(), "four characters is four characters");
        assert!(check_password("ada", "密码密码密码密码密码密码").is_ok());
    }

    #[tokio::test]
    async fn a_password_round_trips_and_a_wrong_one_does_not() {
        let (_dir, store) = store();
        assert!(store.is_empty().await);
        store.add("ada", "correct horse battery").await.expect("add");
        assert!(!store.is_empty().await);
        assert_eq!(store.verify("ada", "correct horse battery").await, Verdict::Ok);
        assert_eq!(store.verify("ada", "correct horse batteri").await, Verdict::No);
        assert_eq!(store.verify("grace", "correct horse battery").await, Verdict::No);
    }

    #[tokio::test]
    async fn the_plaintext_is_never_in_the_file() {
        let (dir, store) = store();
        store.add("ada", "correct horse battery").await.expect("add");
        let raw = std::fs::read_to_string(dir.path().join("users.json")).expect("read");
        assert!(!raw.contains("correct horse battery"));
        assert!(raw.contains("$argon2id$"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn the_file_is_never_world_readable_not_even_for_an_instant() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, store) = store();
        // SAFETY: `umask` is always safe to call; the previous value is restored.
        let previous = unsafe { libc::umask(0o022) };
        let result = store.add("ada", "correct horse battery").await;
        // SAFETY: as above.
        unsafe { libc::umask(previous) };
        result.expect("add");
        let mode = std::fs::metadata(dir.path().join("users.json")).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[tokio::test]
    async fn adding_twice_is_a_refusal_and_removing_twice_is_too() {
        let (_dir, store) = store();
        store.add("ada", "correct horse battery").await.expect("add");
        assert!(matches!(store.add("ada", "another good one").await, Err(UserError::Duplicate(_))));
        store.remove("ada").await.expect("remove");
        assert!(matches!(store.remove("ada").await, Err(UserError::NotFound(_))));
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn changing_a_password_retires_the_old_one_and_moves_updated_at() {
        let (_dir, store) = store();
        store.add("ada", "correct horse battery").await.expect("add");
        let before = store.list().await.first().cloned().expect("one user");
        store.set_password("ada", "a different long one").await.expect("passwd");
        assert_eq!(store.verify("ada", "correct horse battery").await, Verdict::No);
        assert_eq!(store.verify("ada", "a different long one").await, Verdict::Ok);
        let after = store.list().await.first().cloned().expect("one user");
        assert_eq!(after.created_at, before.created_at, "creation is not a password event");
        assert_ne!(after.updated_at, before.updated_at);
    }

    /// A hash written by a build with cheaper parameters is upgraded the next
    /// time the password is typed — the only moment the plaintext exists.
    #[tokio::test]
    async fn a_weaker_stored_hash_is_re_hashed_on_login() {
        let (_dir, store) = store();
        store.add("ada", "correct horse battery").await.expect("add");

        let weak = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(8, 1, 1, None).expect("params"))
            .hash_password(b"correct horse battery", &SaltString::generate(&mut OsRng))
            .expect("hash")
            .to_string();
        assert!(needs_rehash(&weak));
        // Seeded through the database rather than through the file, because the
        // database is what a login reads: an older build's row is what this is
        // pretending to be, and that build wrote a row too.
        store
            .db
            .write(|tx| {
                tx.execute("UPDATE users SET hash = ?1 WHERE name = 'ada'", (&weak,))?;
                Ok(())
            })
            .expect("seed a cheaper hash");

        assert_eq!(
            store.verify("ada", "correct horse battery").await,
            Verdict::Ok,
            "the old hash still verifies"
        );
        assert!(
            !needs_rehash(&store.hash_of("ada").expect("still there")),
            "and is replaced at the current cost"
        );
        let file: UsersFile = serde_json::from_str(&std::fs::read_to_string(store.path()).expect("read")).expect("parse");
        assert!(!needs_rehash(&file.users[0].password_phc), "in the mirror as well as in the database");
    }

    #[tokio::test]
    async fn an_unparseable_file_reads_as_no_users_rather_than_locking_everyone_out() {
        let (dir, store) = store();
        std::fs::write(dir.path().join("users.json"), "{ not json").expect("write");
        assert!(store.is_empty().await);
    }

    /// Accounts in `users.json` with no table behind them — what a build
    /// without the table wrote — are imported once, and the file is not read
    /// again.
    #[tokio::test]
    async fn an_existing_accounts_file_is_imported_once_and_then_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("users.json");
        let existing = UsersFile {
            users: vec![
                StoredUser {
                    name: "ada".into(),
                    password_phc: hash_password("correct horse battery").expect("hash"),
                    created_at: "2024-01-01T00:00:00.000Z".into(),
                    updated_at: "2024-02-02T00:00:00.000Z".into(),
                    app: false,
                },
                StoredUser {
                    name: "grace".into(),
                    password_phc: hash_password("a different long one").expect("hash"),
                    created_at: "2023-01-01T00:00:00.000Z".into(),
                    updated_at: "2023-01-01T00:00:00.000Z".into(),
                    app: false,
                },
            ],
        };
        std::fs::write(&path, serde_json::to_string(&existing).expect("encode")).expect("seed");

        let store = UsersStore::at(path.clone());
        assert_eq!(store.verify("ada", "correct horse battery").await, Verdict::Ok, "the hash came across");
        let listed = store.list().await;
        assert_eq!(
            listed.iter().map(|user| user.name.as_str()).collect::<Vec<_>>(),
            vec!["grace", "ada"],
            "creation order, not the alphabetical order the staging table hands them back in"
        );
        assert_eq!(
            listed[1].created_at, "2024-01-01T00:00:00.000Z",
            "the stamps are the file's, to the millisecond"
        );
        assert_eq!(listed[1].updated_at, "2024-02-02T00:00:00.000Z");

        // The file changes after the import (a hand edit, or a build without
        // the table): it is not the source of truth, and re-reading it would
        // import over whatever has happened since.
        let mut behind = existing.clone();
        behind.users[0].name = "someone-else".into();
        std::fs::write(&path, serde_json::to_string(&behind).expect("encode")).expect("rewrite");
        let reopened = UsersStore::at(path.clone());
        assert_eq!(
            reopened.list().await.len(),
            2,
            "the file is drained exactly once, however many stores are built over it"
        );
        assert!(!reopened.exists("someone-else").await);
    }

    /// The hash outlives the process that made it, in the database and not
    /// only in the mirror.
    #[tokio::test]
    async fn a_hash_survives_a_reopen_of_the_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = UsersStore::at(dir.path().join("users.json"));
        store.add("ada", "correct horse battery").await.expect("add");
        let stored = store.hash_of("ada").expect("hashed");

        // A second connection to the same file, as the CLI in its own process
        // would open it.
        let reopened = crate::db::Db::open(dir.path().join("state.db"));
        let (hash, created): (String, i64) = reopened
            .read(|conn| {
                conn.query_row("SELECT hash, created_at FROM users WHERE name = 'ada'", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
            })
            .expect("the row is on disk");
        assert_eq!(hash, stored);
        assert!(created > 1_700_000_000, "the column is epoch seconds, not a formatted stamp");
        assert!(verify_sync(&hash, "correct horse battery"), "and it is still the password's hash");
    }

    /// Every change reaches the mirror, the one that empties it included: it is
    /// the second opinion [`UsersStore::is_empty`] asks before "no accounts"
    /// switches authentication off.
    #[tokio::test]
    async fn every_change_reaches_the_mirror_including_the_one_that_empties_it() {
        let (_dir, store) = store();
        store.add("ada", "correct horse battery").await.expect("add");
        let mirrored =
            |store: &UsersStore| -> UsersFile { serde_json::from_str(&std::fs::read_to_string(store.path()).expect("read")).expect("parse") };
        assert_eq!(mirrored(&store).users[0].name, "ada");

        store.set_password("ada", "a different long one").await.expect("passwd");
        assert!(
            verify_sync(&mirrored(&store).users[0].password_phc, "a different long one"),
            "the new hash, not the old one"
        );

        store.remove("ada").await.expect("remove");
        assert!(store.path().exists(), "the file stays, empty; a missing one would read as 'no accounts'");
        assert!(mirrored(&store).users.is_empty());
    }

    /// A file that still lists accounts the database has never heard of is the
    /// one shape of damage that must not be read as "this machine is open".
    #[tokio::test]
    async fn accounts_in_the_file_but_not_the_database_keep_the_door_shut() {
        let (_dir, store) = store();
        store.add("ada", "correct horse battery").await.expect("add");
        store
            .db
            .write(|tx| {
                tx.execute("DELETE FROM users", ())?;
                Ok(())
            })
            .expect("lose the row");
        assert!(!store.is_empty().await, "the mirror still names an account, so authentication stays on");
        assert!(
            store.list().await.is_empty(),
            "which is not the same as pretending the account still works"
        );
    }

    /// Two instances, two account sets, no crosstalk in either direction: a
    /// `user add` for the instance somebody else was given leaves the owner's
    /// server as it was.
    #[tokio::test]
    async fn two_instances_on_one_machine_keep_their_own_accounts() {
        let home = tempfile::tempdir().expect("tempdir");
        let owner = instance(home.path(), "alice");
        let guest = instance(home.path(), "bob");

        guest.add("bob", "correct horse battery").await.expect("add");
        assert!(!guest.is_empty().await, "the instance that was given a password requires one");
        assert!(
            owner.is_empty().await,
            "and the one that was not is left exactly as it was, which is the whole bug"
        );

        owner.add("ada", "a different long one").await.expect("add");
        assert!(!owner.exists("bob").await);
        assert!(!guest.exists("ada").await);
        assert_eq!(owner.verify("bob", "correct horse battery").await, Verdict::No);
        assert_eq!(guest.verify("bob", "correct horse battery").await, Verdict::Ok);
        assert_ne!(owner.path(), guest.path(), "two files, not one");
    }

    /// An account the machine-wide set still names is not this instance's
    /// account, so the instance starts in its setup state, not open.
    #[tokio::test]
    async fn an_instance_does_not_inherit_the_machine_wide_accounts() {
        let home = tempfile::tempdir().expect("tempdir");
        let machine = machine_wide(home.path());
        machine.add("ada", "correct horse battery").await.expect("seed the machine-wide set");

        let fresh = instance(home.path(), "alice");
        assert!(fresh.is_empty().await);
        assert!(!fresh.has_users().await, "so its server is in its setup state");
        assert_eq!(fresh.verify("ada", "correct horse battery").await, Verdict::No);

        // And the machine-wide account is still there: a rollback, or a person
        // adding the same login by hand, still finds it.
        assert_eq!(
            machine.verify("ada", "correct horse battery").await,
            Verdict::Ok,
            "the machine-wide set is left exactly as it was"
        );
        let file = std::fs::read_to_string(home.path().join(".config/remote-hand/users.json")).expect("still there");
        assert!(file.contains("ada"), "and the file an older build reads still names it");
    }

    /// Nothing is seeded, but the seed's marker is written — once, with no rows
    /// — so a fallback to a binary that still seeds does not copy the
    /// machine-wide accounts into an instance created here.
    #[tokio::test]
    async fn a_fresh_instance_records_the_seed_marker_without_importing() {
        let home = tempfile::tempdir().expect("tempdir");
        let machine = machine_wide(home.path());
        machine.add("ada", "correct horse battery").await.expect("seed the machine-wide set");

        let fresh = instance(home.path(), "alice");
        assert!(fresh.is_empty().await, "reading the store is what writes the marker");
        assert_eq!(
            fresh.db.kv_get(db::scope::IMPORTED, SEED_MARKER).as_deref(),
            Some("0"),
            "considered, with nothing to copy"
        );
        assert!(!fresh.exists("ada").await, "and no machine-wide account came with it");
    }

    /// A mirror somebody deleted — tidying, a restore, a `rm` that meant
    /// something else — is written again the next time the store is opened.
    /// The condition is "accounts, and no file naming them", so the damage is
    /// repaired whatever produced it.
    #[tokio::test]
    async fn a_mirror_that_goes_missing_is_written_again_on_the_next_open() {
        let home = tempfile::tempdir().expect("tempdir");
        let store = instance(home.path(), "alice");
        store.add("ada", "correct horse battery").await.expect("add");
        std::fs::remove_file(store.path()).expect("delete the mirror");

        let reopened = instance(home.path(), "alice");
        assert!(!reopened.is_empty().await);
        let mirrored: UsersFile = serde_json::from_str(&std::fs::read_to_string(reopened.path()).expect("written again")).expect("parse");
        assert_eq!(mirrored.users.first().map(|user| user.name.as_str()), Some("ada"));
    }

    /// A machine that never had an account keeps the documented fail-open, one
    /// server at a time: an instance with no account of its own is open until
    /// its first `user add`, and looking at it creates no configuration
    /// database on the machine.
    #[tokio::test]
    async fn an_instance_on_a_machine_with_no_accounts_is_open_until_its_first_user_add() {
        let home = tempfile::tempdir().expect("tempdir");
        let fresh = instance(home.path(), "alice");
        assert!(fresh.is_empty().await);
        assert!(
            !home.path().join(".config/remote-hand/state.db").exists(),
            "and no configuration database is created just by looking for one"
        );
        fresh.add("ada", "correct horse battery").await.expect("add");
        assert!(!fresh.is_empty().await);
    }

    /// Burning a verification must never panic, whatever it is handed: it runs
    /// on the refusal path, where a panic would be a 500 that says which names
    /// exist.
    #[tokio::test]
    async fn burning_a_verification_is_infallible() {
        let (_dir, store) = store();
        store.burn_a_verification().await;
        assert!(!verify_sync("not a phc string at all", "x"));
        assert!(!verify_sync("", ""));
    }
}
