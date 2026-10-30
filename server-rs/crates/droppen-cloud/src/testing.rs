//! What the server's tests reach of cloud beyond what the server itself
//! calls: the link control's running link, restart hook and enrollment
//! expiry; whether `cloud.json` wants the aggregator; cloud's paths, its
//! `cloud.json` and the sign-in in flight; the status, the manage rule and the
//! local accounts as the routes read them; the hub-web discovery switch; and
//! droppen.app's side of a hand-off and a hub that answers discovery. Only
//! with `test-support`, which only the server's `[dev-dependencies]` turn on;
//! none of it is in a normal or release build.
//!
//! Each method is cloud's own, under the same name. `cloud.json` and the
//! attempt are read through copies of their own ([`TestCloudFile`],
//! [`TestAttempt`]), field for field, so cloud's types stay its own.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::MutexGuard;

use droppen_agent::link::Running;
use droppen_agent::rotation::Restart;
use droppen_http::error::ApiError;
use droppen_instance::paths::Paths;

use crate::file::CloudFile;
use crate::link::Attempt;
use crate::link_handoff::Handoff;
use crate::{Cloud, CloudState, CloudStatus, LinkFile};

pub use crate::hub_web::{hub_answering, port};
pub use crate::link_handoff::answer;

/// The link control, as the server's tests read and nudge it.
pub trait ControlTesting {
    /// The link this control runs.
    fn running_link(&self) -> MutexGuard<'_, Option<Running>>;
    /// The restart hook it hands the link.
    fn link_restart_hook(&self) -> Restart;
    /// Let enrollment `generation`'s code run out now, as its expiry does.
    fn expire_enrollment_now(&self, generation: u64) -> impl Future<Output = ()>;
}

/// `cloud.json`'s switch, as the server's tests read it.
pub trait LinkFileTesting {
    /// Whether the owner wants this computer to control the account's other
    /// devices.
    fn wants_aggregator(&self) -> bool;
}

impl LinkFileTesting for LinkFile {
    fn wants_aggregator(&self) -> bool {
        LinkFile::wants_aggregator(self)
    }
}

/// Cloud, as the server's tests read and set it.
pub trait CloudTesting {
    /// This instance's paths.
    fn paths(&self) -> &Paths;
    /// `cloud.json`, or its default where it does not read.
    fn read_file(&self) -> TestCloudFile;
    /// `cloud.json`, or `None` where it does not read.
    fn read_file_strict(&self) -> Option<TestCloudFile>;
    /// Change `cloud.json` under its lock, and write it.
    fn update_file(&self, f: impl FnOnce(&mut TestCloudFile)) -> Result<TestCloudFile, ApiError>;
    /// The sign-in in flight, if any (an expired one is dropped).
    fn live_attempt(&self) -> Option<TestAttempt>;
}

impl CloudTesting for Cloud {
    fn paths(&self) -> &Paths {
        &self.paths
    }
    fn read_file(&self) -> TestCloudFile {
        Cloud::read_file(self).into()
    }
    fn read_file_strict(&self) -> Option<TestCloudFile> {
        Cloud::read_file_strict(self).map(Into::into)
    }
    fn update_file(&self, f: impl FnOnce(&mut TestCloudFile)) -> Result<TestCloudFile, ApiError> {
        Cloud::update_file(self, |file| {
            let mut copy = TestCloudFile::from(file.clone());
            f(&mut copy);
            *file = copy.into();
        })
        .map(Into::into)
    }
    fn live_attempt(&self) -> Option<TestAttempt> {
        Cloud::live_attempt(self).map(Into::into)
    }
}

/// The routes' state, as the server's tests ask it what the routes ask.
pub trait CloudStateTesting {
    /// Where this instance stands, as `GET /api/cloud` tells `user`.
    fn status_for(&self, user: Option<&str>) -> impl Future<Output = CloudStatus>;
    /// Whether `user` may manage the link `file` describes.
    fn may_manage(&self, file: &TestCloudFile, user: &str) -> impl Future<Output = bool>;
    /// Each local account's `createdAt`, by name.
    fn accounts(&self) -> impl Future<Output = BTreeMap<String, String>>;
}

impl CloudStateTesting for CloudState {
    async fn status_for(&self, user: Option<&str>) -> CloudStatus {
        crate::status_for(self, user).await
    }
    async fn may_manage(&self, file: &TestCloudFile, user: &str) -> bool {
        crate::access::may_manage(self, &file.clone().into(), user).await
    }
    async fn accounts(&self) -> BTreeMap<String, String> {
        crate::access::accounts(self).await
    }
}

/// Whether the sign-in asks the hub where its app is, as the server's tests
/// turn it on.
pub trait HubWebsTesting {
    /// Look hubs up (or not).
    fn set_discover(&self, on: bool);
}

/// `<instance>/cloud.json`, field for field.
#[derive(Debug, Clone, Default)]
pub struct TestCloudFile {
    /// The hub's web origin it was linked through.
    pub hub: Option<String>,
    /// The local account that signed this computer in.
    pub owner: Option<String>,
    /// Whether the sign-in asked for the aggregator.
    pub aggregate: bool,
    /// The owner turned the aggregator off.
    pub aggregator_off: bool,
    /// When (ms) the sign-in finished.
    pub linked_at: Option<u64>,
    /// The account's e-mail address as the popup said it.
    pub email: Option<String>,
    /// Local account → may see and use other machines.
    pub remote_access: BTreeMap<String, bool>,
    /// Local account → its `createdAt` when it was made the owner or allowed.
    pub accounts_since: BTreeMap<String, String>,
}

impl From<CloudFile> for TestCloudFile {
    fn from(file: CloudFile) -> Self {
        let CloudFile {
            hub,
            owner,
            aggregate,
            aggregator_off,
            linked_at,
            email,
            remote_access,
            accounts_since,
        } = file;
        Self {
            hub,
            owner,
            aggregate,
            aggregator_off,
            linked_at,
            email,
            remote_access,
            accounts_since,
        }
    }
}

impl From<TestCloudFile> for CloudFile {
    fn from(file: TestCloudFile) -> Self {
        let TestCloudFile {
            hub,
            owner,
            aggregate,
            aggregator_off,
            linked_at,
            email,
            remote_access,
            accounts_since,
        } = file;
        Self {
            hub,
            owner,
            aggregate,
            aggregator_off,
            linked_at,
            email,
            remote_access,
            accounts_since,
        }
    }
}

/// A sign-in in flight, field for field.
#[derive(Debug, Clone)]
pub struct TestAttempt {
    /// Its id.
    pub id: String,
    /// The nonce `finish` must bring back.
    pub nonce: String,
    /// The local user who opened it.
    pub owner: String,
    /// The hub, as the agent signs in to it (`wss://…`).
    pub hub: String,
    /// Where a browser signs in to the hub.
    pub hub_web: String,
    /// Whether the sign-in asks for the aggregator.
    pub aggregate: bool,
    /// When (ms) it runs out.
    pub expires_ms: u64,
    /// The browser hand-off's one-off key and state, when a page opened it.
    pub handoff: Option<Handoff>,
}

impl From<Attempt> for TestAttempt {
    fn from(attempt: Attempt) -> Self {
        let Attempt {
            id,
            nonce,
            owner,
            hub,
            hub_web,
            aggregate,
            expires_ms,
            handoff,
        } = attempt;
        Self {
            id,
            nonce,
            owner,
            hub,
            hub_web,
            aggregate,
            expires_ms,
            handoff,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cloud.json` copied and back is the file it was, field for field: the
    /// server's tests read and write it only through the copy.
    #[test]
    fn a_copy_of_cloud_json_reads_back_as_the_file() {
        let file = CloudFile {
            hub: Some("https://hub.example".into()),
            owner: Some("alice".into()),
            aggregate: true,
            aggregator_off: true,
            linked_at: Some(7),
            email: Some("alice@example.com".into()),
            remote_access: [("bob".to_string(), true)].into_iter().collect(),
            accounts_since: [("alice".to_string(), "2026-10-19T00:00:00.000Z".to_string())].into_iter().collect(),
        };
        let back = CloudFile::from(TestCloudFile::from(file.clone()));
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&file).unwrap());
    }

    /// The attempt copied is the attempt, field for field: the server's tests
    /// read the sign-in in flight only through the copy. Every field differs
    /// from its default, so a field the copy dropped or swapped shows.
    #[test]
    fn a_copy_of_the_attempt_reads_as_the_attempt() {
        let machine = crate::link_handoff::HandoffMachine {
            hostname: "laptop".into(),
            sign_key: [1; 32],
            dh_key: [2; 32],
            enrolled: None,
        };
        let attempt = Attempt {
            id: "a1".into(),
            nonce: "n1".into(),
            owner: "alice".into(),
            hub: "wss://hub.example".into(),
            hub_web: "https://app.example".into(),
            aggregate: true,
            expires_ms: 1_000,
            handoff: Some(Handoff::new("https://app.example", "a1", 1_000, "http://127.0.0.1:8700", &machine)),
        };
        let copy = TestAttempt::from(attempt.clone());
        assert_eq!(copy.id, attempt.id);
        assert_eq!(copy.nonce, attempt.nonce);
        assert_eq!(copy.owner, attempt.owner);
        assert_eq!(copy.hub, attempt.hub);
        assert_eq!(copy.hub_web, attempt.hub_web);
        assert_eq!(copy.aggregate, attempt.aggregate);
        assert_eq!(copy.expires_ms, attempt.expires_ms);
        // A hand-off's URL carries its own one-off key and state, so the same
        // URL is the same hand-off.
        assert_eq!(copy.handoff.as_ref().map(Handoff::url), attempt.handoff.as_ref().map(Handoff::url));
        assert!(copy.handoff.is_some());
    }
}
