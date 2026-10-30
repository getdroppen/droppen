//! The server's alert channels, kept apart as two modules:
//!
//! * [`notify`]: Bark, the nudge a session decides to send (its settings, the
//!   token an injected `rh-notify` proves itself with, the skill and the
//!   `notify` command line);
//! * [`push`]: Web Push, the alerts the server decides to send (the VAPID key,
//!   the subscriptions, the payload encryption), and the fan-out that sends
//!   each of those alerts to Bark as well.
//!
//! The auth guard asks [`notify`] about a caller through droppen-http's
//! `NotifyCredential` port, and the runs, rotations and link events the
//! fan-out announces reach it through ports the entry fills.

pub mod notify;
pub mod push;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
