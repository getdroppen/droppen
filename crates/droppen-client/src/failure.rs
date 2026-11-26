//! Why a request to a host did not get through, sorted into what a caller can
//! act on ([`Failure`]) — the a2a proxy maps each to an A2A error
//! (`docs/a2a.md` §7.4).
//!
//! Some refusals cannot be judged from the code alone: the agent closes a
//! hello it will not take with 4401 whether the device was removed or the
//! host does not accept it; a relay or a hub refuses with 4403 / `403` for
//! the same two reasons; the bridge answers 412 `lapsed` both for a stale
//! view and for a device that may no longer control the host. Those are
//! [`Failure::Refused`] and [`Failure::Lapsed`], and the caller decides from
//! its own verified chain ([`crate::verified`]), never from the far end's word.
//!
//! Where each code comes from:
//!
//! | code | who | here |
//! |---|---|---|
//! | `CLOSE 4404` on `OPEN`, presence `offline` | relay (`droppen-relay-core` `route.rs`) | [`Failure::Offline`] |
//! | `CLOSE 4410` on a channel, a lost relay link, 1012/1013 | relay | [`Failure::Offline`] |
//! | websocket close 4419 | relay: a ticket that ran out | [`Failure::Offline`] |
//! | handshake `Stale`, `CLOSE 4412` | agent (`handshake.rs`, `lapse.rs`) | [`Failure::Stale`] |
//! | `CLOSE 4401` (hello refused), `4403` (removed / no longer accepted) | agent (`admit.rs`, `lapse.rs`) | [`Failure::Refused`] |
//! | `CLOSE 4403` on `OPEN`, websocket close 4403 | relay (the ticket's `peers`, a revocation) | [`Failure::Refused`] |
//! | rpc `CLOSE 4403` before a head | bridge (a path its class may not reach) | [`Failure::Refused`] |
//! | rpc `412 stale` `lapsed` | bridge (`bridge.rs`) | [`Failure::Lapsed`] (from [`crate::rpc::refusal_of`]) |
//! | `403 device_removed`, `410 account_deleted` on a session | hub | [`Failure::Removed`] |

/// Who said no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refuser {
    /// The hub: a ticket or a session refused.
    Hub,
    /// A relay: an `OPEN` its ticket does not allow, a revoked connection.
    Relay,
    /// The host's agent: a hello it would not take, a channel it closed.
    Agent,
    /// The host's bridge: a stream it would not serve.
    Bridge,
}

/// Why a request to a host did not get through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The host cannot be reached now: no relay has it signed in, the hub
    /// that would say where it is cannot be reached, or the channel or relay
    /// connection under the request went away.
    Offline(String),
    /// The host takes no channel now because no admin device vouched for its
    /// view of the account within seven days.
    Stale(String),
    /// Refused, by `by` with `code`: removed from the account, or not
    /// accepted by the host — which, is for the caller's chain to say.
    Refused { by: Refuser, code: u16, why: String },
    /// The bridge's `412 stale` `lapsed`: the host takes no control from this
    /// device now — stale, removed or no longer accepted, for the caller's
    /// chain to say.
    Lapsed(String),
    /// The hub refused this device's session: removed from the account
    /// (`403 device_removed`), or the account is gone (`410`).
    Removed(String),
    /// The device chain: a fork between this device's view and another's, a
    /// view that does not verify, or one that does not name the host. An
    /// alarm, never retried.
    Chain(String),
    /// Anything else: a protocol error, a request that could not be made.
    Other(String),
}

impl Failure {
    /// How a channel ended (its `CLOSE`), as a failure. `replied`: the agent
    /// had answered the hello (a 4403 before that is the relay refusing the
    /// `OPEN`, after it the agent closing the channel).
    pub fn from_channel_close(code: u16, reason: &str, replied: bool) -> Failure {
        let why = closed(code, reason);
        match code {
            4412 => Failure::Stale(why),
            4401 => Failure::Refused {
                by: Refuser::Agent,
                code,
                why,
            },
            4403 => Failure::Refused {
                by: if replied { Refuser::Agent } else { Refuser::Relay },
                code,
                why,
            },
            // A malformed request, an overrun window, a channel number reused:
            // a fault on one side, not a state of the host.
            4400 | 4408 => Failure::Other(why),
            // 4404 not on this relay, 4410 the other end gone, 4409 too many
            // channels, 4503 a host not added yet, 4507 a host that cannot
            // save who connects, anything else: not reachable now.
            _ => Failure::Offline(why),
        }
    }

    /// How a relay link ended (the websocket's close frame, `None` for none),
    /// as a failure of the channels on it.
    pub fn from_link_close(close: Option<(u16, String)>) -> Failure {
        match close {
            // A revocation (removed from the account), or a sign-in refused.
            Some((code @ (4401 | 4403), reason)) => Failure::Refused {
                by: Refuser::Relay,
                code,
                why: format!("the relay connection {}", closed(code, &reason)),
            },
            // 4419 a ticket that ran out, 4429 over a limit, 1012/1013 a relay
            // restarting or full, 4002 no heartbeat, anything else.
            Some((code, reason)) => Failure::Offline(format!("the relay connection {}", closed(code, &reason))),
            None => Failure::Offline("the relay connection was lost".into()),
        }
    }

    /// A short word for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Failure::Offline(_) => "offline",
            Failure::Stale(_) => "stale",
            Failure::Refused { .. } => "refused",
            Failure::Lapsed(_) => "lapsed",
            Failure::Removed(_) => "removed",
            Failure::Chain(_) => "chain",
            Failure::Other(_) => "other",
        }
    }
}

fn closed(code: u16, reason: &str) -> String {
    if reason.is_empty() {
        format!("closed with {code}")
    } else {
        format!("closed with {code}: {reason}")
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Offline(why) => write!(f, "the device is not reachable: {why}"),
            Failure::Stale(why) => write!(f, "the device's view of the account is not vouched for: {why}"),
            Failure::Refused { by, code, why } => write!(f, "refused by the {by:?} ({code}): {why}"),
            Failure::Lapsed(why) => write!(f, "the device takes no control from this one now: {why}"),
            Failure::Removed(why) => write!(f, "this device was removed: {why}"),
            Failure::Chain(why) => write!(f, "the device chain: {why}"),
            Failure::Other(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_codes_sort_into_what_a_caller_acts_on() {
        assert!(matches!(
            Failure::from_channel_close(4404, "the device is offline", false),
            Failure::Offline(_)
        ));
        assert!(matches!(Failure::from_channel_close(4410, "", true), Failure::Offline(_)));
        assert!(matches!(Failure::from_channel_close(4503, "", false), Failure::Offline(_)));
        assert!(matches!(Failure::from_channel_close(4412, "stale chain view", true), Failure::Stale(_)));
        assert_eq!(
            Failure::from_channel_close(4401, "handshake refused", false),
            Failure::Refused {
                by: Refuser::Agent,
                code: 4401,
                why: "closed with 4401: handshake refused".into()
            }
        );
        assert!(matches!(
            Failure::from_channel_close(4403, "x", false),
            Failure::Refused {
                by: Refuser::Relay,
                code: 4403,
                ..
            }
        ));
        assert!(matches!(
            Failure::from_channel_close(4403, "x", true),
            Failure::Refused {
                by: Refuser::Agent,
                code: 4403,
                ..
            }
        ));
        assert!(matches!(Failure::from_channel_close(4408, "", true), Failure::Other(_)));
        assert!(matches!(
            Failure::from_link_close(Some((4419, "the ticket expired".into()))),
            Failure::Offline(_)
        ));
        assert!(matches!(Failure::from_link_close(Some((1012, String::new()))), Failure::Offline(_)));
        assert!(matches!(Failure::from_link_close(None), Failure::Offline(_)));
        assert!(matches!(
            Failure::from_link_close(Some((4403, "removed from the account".into()))),
            Failure::Refused {
                by: Refuser::Relay,
                code: 4403,
                ..
            }
        ));
    }
}
