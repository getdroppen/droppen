//! The routing table of a blind relay: which channel on which connection is
//! paired with which, and the credit each sender has left. Sans-IO — it takes
//! frames and returns the frames to deliver — so the routing rules are tested
//! without sockets. Whether a device may reach a machine at all is the
//! caller's verdict, passed to [`Relay::open`].
//!
//! # Channels
//!
//! A device names a channel with a `ch` of its own choosing on its own
//! connection (`OPEN {ch, peer: machine}`). Its numbers must increase and are
//! never reused (`feed/mux.rs`'s rule for `sub`), and `ch` 0 is the
//! connection's control channel. The relay allocates the agent's side of the
//! pair from the agent connection's own counter, also never reused, and
//! forwards `OPEN {ch: agent's ch, peer: device id}`: each end learns who is on
//! the other side from the relay, and the Noise prologue then binds both names,
//! so a relay that lies about them only gets a failed handshake.
//!
//! After that, `DATA`, `CREDIT` and `CLOSE` on a paired `ch` are rewritten to
//! the other side's `ch` and forwarded; `DATA` payloads are copied and never
//! parsed. A `CLOSE` from either side ends the pair. A connection that goes
//! away closes every channel it had, with [`CLOSE_PEER_GONE`].
//!
//! # Credit
//!
//! Each direction of each channel has a [`SendWindow`] (`rh_proto::flow`): a
//! sender that sends past it, or a receiver that grants more than it was sent,
//! has its channel closed with [`CLOSE_FLOW`]. What one channel can make the
//! relay hold is therefore bounded by the window, whatever either end does.

use std::collections::HashMap;

use rh_proto::flow::SendWindow;
use rh_proto::frame::Frame;

pub type ConnId = u64;

/// The machine is not connected (or not an agent of this account).
pub const CLOSE_OFFLINE: u16 = 4404;
/// The device or the machine is not current in the account's device chain.
pub const CLOSE_FORBIDDEN: u16 = 4403;
/// A malformed request: a reused or backward `ch`, an agent-side `OPEN`.
pub const CLOSE_PROTOCOL: u16 = 4400;
/// A sender overran its credit, or a receiver over-granted.
pub const CLOSE_FLOW: u16 = 4408;
/// Too many channels on one connection.
pub const CLOSE_TOO_MANY: u16 = 4409;
/// The other end's connection went away (a restart, a network change).
pub const CLOSE_PEER_GONE: u16 = 4410;

/// Who is on a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peer {
    Device { account: String, device: String },
    Agent { account: String, machine: String },
}

impl Peer {
    pub fn account(&self) -> &str {
        match self {
            Peer::Device { account, .. } | Peer::Agent { account, .. } => account,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Peer::Device { device, .. } => device,
            Peer::Agent { machine, .. } => machine,
        }
    }
}

/// A frame for a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub to: ConnId,
    pub frame: Frame,
}

#[derive(Debug)]
struct Link {
    peer: ConnId,
    peer_ch: u32,
    /// What this side may still send on the channel.
    window: SendWindow,
}

#[derive(Debug)]
struct Conn {
    peer: Peer,
    links: HashMap<u32, Link>,
    /// Devices: the highest `ch` opened. Agents: the last `ch` allocated.
    last_ch: u32,
    max_channels: usize,
}

/// The routing tables.
#[derive(Debug, Default)]
pub struct Relay {
    conns: HashMap<ConnId, Conn>,
    /// The live connection of each machine, by `(account, machine)`: a
    /// machine id is the account's own name for it, so another account's
    /// machine of the same id is another machine and never replaces it.
    agents: HashMap<(String, String), ConnId>,
}

fn close(to: ConnId, ch: u32, code: u16, reason: &str) -> Delivery {
    Delivery {
        to,
        frame: Frame::Close {
            ch,
            code,
            reason: reason.into(),
        },
    }
}

impl Relay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a signed-in connection. For an agent, returns the connection
    /// it replaces (the same machine of the same account signed in again),
    /// which the caller closes.
    pub fn add(&mut self, id: ConnId, peer: Peer, max_channels: usize) -> Option<ConnId> {
        let replaced = match &peer {
            Peer::Agent { account, machine } => self.agents.insert((account.clone(), machine.clone()), id),
            Peer::Device { .. } => None,
        };
        self.conns.insert(
            id,
            Conn {
                peer,
                links: HashMap::new(),
                last_ch: 0,
                max_channels,
            },
        );
        replaced
    }

    pub fn peer(&self, id: ConnId) -> Option<&Peer> {
        self.conns.get(&id).map(|c| &c.peer)
    }

    /// The connection `account`'s machine is on, if it is connected.
    pub fn agent(&self, account: &str, machine: &str) -> Option<ConnId> {
        self.agents.get(&(account.to_string(), machine.to_string())).copied()
    }

    pub fn channels(&self, id: ConnId) -> usize {
        self.conns.get(&id).map_or(0, |c| c.links.len())
    }

    /// A connection went away: close every channel it had on the other side.
    pub fn remove(&mut self, id: ConnId) -> Vec<Delivery> {
        let Some(conn) = self.conns.remove(&id) else {
            return Vec::new();
        };
        if let Peer::Agent { account, machine } = &conn.peer {
            let key = (account.clone(), machine.clone());
            if self.agents.get(&key) == Some(&id) {
                self.agents.remove(&key);
            }
        }
        let mut out = Vec::new();
        for link in conn.links.values() {
            if let Some(other) = self.conns.get_mut(&link.peer) {
                other.links.remove(&link.peer_ch);
                out.push(close(link.peer, link.peer_ch, CLOSE_PEER_GONE, "the other end disconnected"));
            }
        }
        out
    }

    /// `OPEN {ch, peer: machine}` from connection `from`. `allowed` is the
    /// caller's verdict on whether this device may reach this machine (both
    /// current in the account's chain); refusals are a `CLOSE` back.
    pub fn open(&mut self, from: ConnId, ch: u32, machine: &str, allowed: Result<(), &'static str>) -> Vec<Delivery> {
        let Some(conn) = self.conns.get_mut(&from) else {
            return Vec::new();
        };
        let Peer::Device { account, device } = conn.peer.clone() else {
            return vec![close(from, ch, CLOSE_PROTOCOL, "agents do not open channels")];
        };
        if ch == 0 || ch <= conn.last_ch {
            return vec![close(from, ch, CLOSE_PROTOCOL, "channel numbers increase and are never reused")];
        }
        // The number is spent whatever happens next.
        conn.last_ch = ch;
        if conn.links.len() >= conn.max_channels {
            return vec![close(from, ch, CLOSE_TOO_MANY, "too many channels")];
        }
        if let Err(reason) = allowed {
            return vec![close(from, ch, CLOSE_FORBIDDEN, reason)];
        }
        let Some(agent_id) = self.agent(&account, machine) else {
            return vec![close(from, ch, CLOSE_OFFLINE, "the device is offline")];
        };
        let agent = self.conns.get_mut(&agent_id).expect("indexed agents are registered");
        if agent.peer.account() != account {
            return vec![close(from, ch, CLOSE_OFFLINE, "the device is offline")];
        }
        if agent.links.len() >= agent.max_channels {
            return vec![close(from, ch, CLOSE_TOO_MANY, "the device has too many channels")];
        }
        let Some(agent_ch) = agent.last_ch.checked_add(1) else {
            return vec![close(from, ch, CLOSE_TOO_MANY, "the device's connection ran out of channel numbers")];
        };
        agent.last_ch = agent_ch;
        agent.links.insert(
            agent_ch,
            Link {
                peer: from,
                peer_ch: ch,
                window: SendWindow::new(),
            },
        );
        self.conns.get_mut(&from).expect("present").links.insert(
            ch,
            Link {
                peer: agent_id,
                peer_ch: agent_ch,
                window: SendWindow::new(),
            },
        );
        vec![Delivery {
            to: agent_id,
            frame: Frame::Open { ch: agent_ch, peer: device },
        }]
    }

    /// Tear down the pair `(from, ch)`, telling both sides why.
    fn fail(&mut self, from: ConnId, ch: u32, code: u16, reason: &str) -> Vec<Delivery> {
        let mut out = vec![close(from, ch, code, reason)];
        if let Some(link) = self.conns.get_mut(&from).and_then(|c| c.links.remove(&ch)) {
            if let Some(other) = self.conns.get_mut(&link.peer) {
                other.links.remove(&link.peer_ch);
            }
            out.push(close(link.peer, link.peer_ch, code, reason));
        }
        out
    }

    /// `DATA` on `(from, ch)`: spend the sender's credit and forward the
    /// payload, untouched, to the other side. A frame for a channel that is
    /// not open (one that closed while it was in flight) is dropped.
    pub fn data(&mut self, from: ConnId, ch: u32, payload: Vec<u8>) -> Vec<Delivery> {
        let Some(link) = self.conns.get_mut(&from).and_then(|c| c.links.get_mut(&ch)) else {
            return Vec::new();
        };
        if link.window.consume(payload.len()).is_err() {
            return self.fail(from, ch, CLOSE_FLOW, "sent past the channel's credit");
        }
        vec![Delivery {
            to: link.peer,
            frame: Frame::Data { ch: link.peer_ch, payload },
        }]
    }

    /// `CREDIT` on `(from, ch)`: the other side may send `bytes` more.
    pub fn credit(&mut self, from: ConnId, ch: u32, bytes: u32) -> Vec<Delivery> {
        let Some((peer, peer_ch)) = self.conns.get(&from).and_then(|c| c.links.get(&ch)).map(|l| (l.peer, l.peer_ch)) else {
            return Vec::new();
        };
        let granted = self
            .conns
            .get_mut(&peer)
            .and_then(|c| c.links.get_mut(&peer_ch))
            .map(|l| l.window.grant(bytes));
        match granted {
            Some(Ok(())) => vec![Delivery {
                to: peer,
                frame: Frame::Credit { ch: peer_ch, bytes },
            }],
            Some(Err(_)) => self.fail(from, ch, CLOSE_FLOW, "granted more credit than was sent"),
            None => Vec::new(),
        }
    }

    /// `CLOSE` on `(from, ch)`: forward it and forget the pair.
    pub fn close(&mut self, from: ConnId, ch: u32, code: u16, reason: String) -> Vec<Delivery> {
        let Some(link) = self.conns.get_mut(&from).and_then(|c| c.links.remove(&ch)) else {
            return Vec::new();
        };
        if let Some(other) = self.conns.get_mut(&link.peer) {
            other.links.remove(&link.peer_ch);
        }
        vec![Delivery {
            to: link.peer,
            frame: Frame::Close {
                ch: link.peer_ch,
                code,
                reason,
            },
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rh_proto::flow::INITIAL_CREDIT;

    const DEV: ConnId = 1;
    const AGENT: ConnId = 2;

    fn relay() -> Relay {
        let mut r = Relay::new();
        r.add(
            DEV,
            Peer::Device {
                account: "a-1".into(),
                device: "d-1".into(),
            },
            4,
        );
        r.add(
            AGENT,
            Peer::Agent {
                account: "a-1".into(),
                machine: "m-1".into(),
            },
            1024,
        );
        r
    }

    fn closed(d: &[Delivery]) -> Vec<(ConnId, u32, u16)> {
        d.iter()
            .filter_map(|d| match &d.frame {
                Frame::Close { ch, code, .. } => Some((d.to, *ch, *code)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_channel_is_paired_rewritten_and_closed() {
        let mut r = relay();
        assert_eq!(
            r.open(DEV, 7, "m-1", Ok(())),
            vec![Delivery {
                to: AGENT,
                frame: Frame::Open { ch: 1, peer: "d-1".into() }
            }]
        );
        assert_eq!(
            r.data(DEV, 7, b"opaque".to_vec()),
            vec![Delivery {
                to: AGENT,
                frame: Frame::Data {
                    ch: 1,
                    payload: b"opaque".to_vec()
                }
            }]
        );
        assert_eq!(
            r.data(AGENT, 1, b"back".to_vec()),
            vec![Delivery {
                to: DEV,
                frame: Frame::Data {
                    ch: 7,
                    payload: b"back".to_vec()
                }
            }]
        );
        assert_eq!(
            r.close(AGENT, 1, 1000, "bye".into()),
            vec![Delivery {
                to: DEV,
                frame: Frame::Close {
                    ch: 7,
                    code: 1000,
                    reason: "bye".into()
                }
            }]
        );
        assert!(r.data(DEV, 7, b"late".to_vec()).is_empty(), "a late frame is dropped");
        assert_eq!(r.channels(DEV) + r.channels(AGENT), 0);
        // The agent's numbers keep counting up.
        r.open(DEV, 8, "m-1", Ok(()));
        assert!(matches!(
            r.data(DEV, 8, vec![1]).as_slice(),
            [Delivery {
                frame: Frame::Data { ch: 2, .. },
                ..
            }]
        ));
    }

    #[test]
    fn channel_numbers_are_never_reused_and_refusals_are_closes() {
        let mut r = relay();
        r.open(DEV, 5, "m-1", Ok(()));
        assert_eq!(closed(&r.open(DEV, 5, "m-1", Ok(()))), vec![(DEV, 5, CLOSE_PROTOCOL)]);
        assert_eq!(closed(&r.open(DEV, 3, "m-1", Ok(()))), vec![(DEV, 3, CLOSE_PROTOCOL)]);
        assert_eq!(closed(&r.open(DEV, 0, "m-1", Ok(()))), vec![(DEV, 0, CLOSE_PROTOCOL)]);
        assert_eq!(closed(&r.open(DEV, 6, "m-2", Ok(()))), vec![(DEV, 6, CLOSE_OFFLINE)]);
        assert_eq!(closed(&r.open(DEV, 9, "m-1", Err("not in the chain"))), vec![(DEV, 9, CLOSE_FORBIDDEN)]);
        assert_eq!(closed(&r.open(AGENT, 9, "d-1", Ok(()))), vec![(AGENT, 9, CLOSE_PROTOCOL)]);
        // Device limit is 4 channels.
        for ch in 10..13 {
            r.open(DEV, ch, "m-1", Ok(()));
        }
        assert_eq!(r.channels(DEV), 4);
        assert_eq!(closed(&r.open(DEV, 20, "m-1", Ok(()))), vec![(DEV, 20, CLOSE_TOO_MANY)]);
    }

    #[test]
    fn another_accounts_machine_looks_offline() {
        let mut r = relay();
        r.add(
            3,
            Peer::Device {
                account: "a-2".into(),
                device: "d-2".into(),
            },
            4,
        );
        assert_eq!(closed(&r.open(3, 1, "m-1", Ok(()))), vec![(3, 1, CLOSE_OFFLINE)]);
    }

    #[test]
    fn another_accounts_machine_of_the_same_id_replaces_nothing() {
        let mut r = relay();
        let other_agent = Peer::Agent {
            account: "a-2".into(),
            machine: "m-1".into(),
        };
        assert_eq!(r.add(3, other_agent, 8), None, "a-1's m-1 stays signed in");
        r.add(
            4,
            Peer::Device {
                account: "a-2".into(),
                device: "d-2".into(),
            },
            4,
        );
        assert_eq!((r.agent("a-1", "m-1"), r.agent("a-2", "m-1")), (Some(AGENT), Some(3)));
        // Each account's device reaches its own machine.
        assert!(matches!(r.open(DEV, 1, "m-1", Ok(())).as_slice(), [Delivery { to: AGENT, .. }]));
        assert!(matches!(r.open(4, 1, "m-1", Ok(())).as_slice(), [Delivery { to: 3, .. }]));
        // One going away leaves the other, and its channel, alone.
        assert_eq!(closed(&r.remove(3)), vec![(4, 1, CLOSE_PEER_GONE)]);
        assert_eq!((r.agent("a-1", "m-1"), r.agent("a-2", "m-1")), (Some(AGENT), None));
        assert_eq!(r.channels(AGENT), 1);
    }

    #[test]
    fn credit_is_enforced_both_ways() {
        let mut r = relay();
        r.open(DEV, 1, "m-1", Ok(()));
        let chunk = 60_000;
        let mut sent = 0;
        while sent + chunk <= INITIAL_CREDIT as usize {
            assert_eq!(r.data(DEV, 1, vec![0; chunk]).len(), 1);
            sent += chunk;
        }
        // The agent hands some back; the device may send that much more.
        assert_eq!(
            r.credit(AGENT, 1, chunk as u32),
            vec![Delivery {
                to: DEV,
                frame: Frame::Credit { ch: 1, bytes: chunk as u32 }
            }]
        );
        assert_eq!(r.data(DEV, 1, vec![0; chunk]).len(), 1);
        // Past the window: both sides are told and the pair is gone.
        assert_eq!(
            closed(&r.data(DEV, 1, vec![0; chunk])),
            vec![(DEV, 1, CLOSE_FLOW), (AGENT, 1, CLOSE_FLOW)]
        );
        assert_eq!(r.channels(DEV) + r.channels(AGENT), 0);

        // A receiver cannot inflate the sender's window either.
        r.open(DEV, 2, "m-1", Ok(()));
        assert_eq!(closed(&r.credit(AGENT, 2, 1)), vec![(AGENT, 2, CLOSE_FLOW), (DEV, 2, CLOSE_FLOW)]);
    }

    #[test]
    fn a_connection_that_goes_closes_its_channels_and_a_new_agent_login_replaces_the_old() {
        let mut r = relay();
        r.open(DEV, 1, "m-1", Ok(()));
        r.open(DEV, 2, "m-1", Ok(()));
        let mut gone = closed(&r.remove(AGENT));
        gone.sort();
        assert_eq!(gone, vec![(DEV, 1, CLOSE_PEER_GONE), (DEV, 2, CLOSE_PEER_GONE)]);
        assert_eq!(r.agent("a-1", "m-1"), None);
        assert_eq!(
            r.add(
                5,
                Peer::Agent {
                    account: "a-1".into(),
                    machine: "m-1".into()
                },
                8
            ),
            None
        );
        assert_eq!(
            r.add(
                6,
                Peer::Agent {
                    account: "a-1".into(),
                    machine: "m-1".into()
                },
                8
            ),
            Some(5),
            "the old connection is the caller's to close"
        );
        assert!(r.remove(5).is_empty());
        assert_eq!(r.agent("a-1", "m-1"), Some(6), "removing the replaced connection leaves the new one");
    }
}
