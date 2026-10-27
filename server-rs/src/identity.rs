//! Who this server is, in a form a rename cannot change.
//!
//! A server is named by its instance ([`crate::sessions::ServerId`]), and a name
//! is the right handle for almost everything: it is a path segment, it is in
//! `APP_INSTANCE`, and it is what a person says when they mean "the one on the
//! laptop". What a name cannot do is *prove* anything. Two machines may both be
//! running the `default` instance, and one machine's instance may be renamed
//! between one boot and the next — so a session row that says it came from
//! `alice` is a claim that only holds as long as nobody else says the same word.
//!
//! So each instance also mints a uuid, once, and keeps it. The pair is the
//! identity: the uuid says *which* server, for ever; the name says what that
//! server is currently called, so a log line or a UI has something to show.
//!
//! **The uuid is what a machine key is bound to.** [`crate::agent`] refuses a
//! stored key whose recorded uuid differs, so a key cannot be carried onto a
//! server wearing another identity. It is otherwise local: not on the wire, not
//! in any row but its own.
//!
//! # What changes and what does not
//!
//! The uuid is minted at the first boot that finds `server_identity` empty and
//! is never rewritten — not on a rename, not on an upgrade, not when the
//! database moves to a new machine with the user's home directory. The name is
//! kept current, because it is the instance's name and the instance can be
//! renamed; a stale one would be a second, worse answer to a question
//! [`crate::sessions::ServerId::local`] already answers.
//!
//! Per instance, in [`crate::sessions::Paths::instance_db`], because a server
//! *is* an instance: two servers on one machine are two servers, and one row in
//! the machine-wide database could not say that.

use crate::sessions::{Paths, ServerId};

/// One server's durable identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerIdentity {
    /// The uuid minted at this instance's first boot. Never changes.
    pub uuid: String,
    /// What the instance is called today.
    pub name: ServerId,
}

/// Mint this instance's identity if it has none, and keep its name current.
///
/// Returns what the row says afterwards, or `None` if the database could not be
/// read or written — which is survivable in the way everything else in
/// [`crate::db`] is: the next boot mints it.
///
/// Called once, from the boot path, under the instance lock. It is idempotent,
/// so calling it twice in one process is a read and at most one small write.
pub fn ensure(paths: &Paths) -> Option<ServerIdentity> {
    let db = crate::db::at(paths.instance_db());
    let local = ServerId::local(paths);
    let outcome = db.write(|tx| {
        // The oldest row wins if there is somehow more than one. There cannot
        // be — the insert below only fires on an empty table, inside this
        // transaction — but "pick one and keep picking it" is cheaper than a
        // constraint that would have to be migrated in later.
        let existing: Option<(String, String)> = tx
            .query_row("SELECT uuid, name FROM server_identity ORDER BY created_at, uuid LIMIT 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .ok();
        let uuid = match existing {
            Some((uuid, name)) => {
                // A rename updates the name and leaves the uuid alone: this is
                // still the same server, answering to something else.
                if name != local.as_str() {
                    tx.execute("UPDATE server_identity SET name = ?1 WHERE uuid = ?2", (local.as_str(), uuid.as_str()))?;
                    tracing::info!(%uuid, from = %name, to = %local, "this server's instance has been renamed");
                }
                uuid
            }
            None => {
                let uuid = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO server_identity (uuid, name) VALUES (?1, ?2)",
                    (uuid.as_str(), local.as_str()),
                )?;
                tracing::info!(%uuid, name = %local, "minted this server's identity");
                uuid
            }
        };
        Ok(ServerIdentity { uuid, name: local.clone() })
    });
    match outcome {
        Ok(identity) => Some(identity),
        Err(error) => {
            tracing::warn!(path = %paths.instance_db().display(), %error, "cannot record this server's identity; it will be minted at the next boot");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minted once and kept: the uuid is what a later slice will trust, so a
    /// second boot inventing a new one would make it worthless.
    #[test]
    fn the_identity_is_minted_once_and_survives_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "alice");

        let first = ensure(&paths).expect("minted");
        assert_eq!(first.name.as_str(), "alice");
        assert_eq!(
            uuid::Uuid::parse_str(&first.uuid).map(|parsed| parsed.to_string()).as_deref(),
            Ok(first.uuid.as_str()),
            "a uuid, not a name that happens to be unique"
        );

        let second = ensure(&paths).expect("already minted");
        assert_eq!(second, first, "a second boot finds the row rather than making one");

        let rows: i64 = crate::db::at(paths.instance_db())
            .read(|conn| conn.query_row("SELECT count(*) FROM server_identity", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(rows, 1, "one server, one row");
    }

    /// A rename is the case the uuid exists for: the server is the same server.
    ///
    /// An instance's database lives in the instance's own directory, so
    /// "renaming" it means the directory came along — a `mv` under `instances/`,
    /// or a backup restored under another name. That is exactly the case where
    /// a name proves nothing and the uuid has to: the rows in that database are
    /// this server's rows, whatever it is now called.
    #[test]
    fn a_rename_keeps_the_uuid_and_moves_the_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let renamed = Paths::for_instance(dir.path().to_path_buf(), "bob");
        let db = crate::db::at(renamed.instance_db());
        let minted = uuid::Uuid::new_v4().to_string();
        db.commit("an instance that arrived under another name", |tx| {
            tx.execute("INSERT INTO server_identity (uuid, name) VALUES (?1, 'alice')", (minted.as_str(),))?;
            Ok(())
        });

        let after = ensure(&renamed).expect("read back");
        assert_eq!(after.uuid, minted, "the same server, renamed");
        assert_eq!(after.name.as_str(), "bob");

        let (uuid, name): (String, String) = db
            .read(|conn| conn.query_row("SELECT uuid, name FROM server_identity", [], |row| Ok((row.get(0)?, row.get(1)?))))
            .expect("the one row");
        assert_eq!(
            (uuid.as_str(), name.as_str()),
            (minted.as_str(), "bob"),
            "and that is what is on disk"
        );
    }

    /// Two instances on one machine are two servers, with two identities.
    #[test]
    fn two_instances_mint_two_identities() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = ensure(&Paths::for_instance(dir.path().to_path_buf(), "a")).expect("minted");
        let b = ensure(&Paths::for_instance(dir.path().to_path_buf(), "b")).expect("minted");
        assert_ne!(a.uuid, b.uuid);
    }
}
