//! Adding a second device (architecture doc §1 "新设备如何拿到钥").
//!
//! **QR** ([`Offer`]): the new device — signed in to the account at the hub
//! and registered there, but not in the chain — shows its id, both public keys
//! and a one-time secret `s` as `RHP1-<base32>` (`DRP1-` is read too), as a
//! QR code or as text to paste. A full device that reads it signs "add device"
//! for exactly those keys and leaves a [`Delivery`] in the new device's
//! mailbox: every `UK` epoch, sealed to the new device from its own static
//! key, with the account's fingerprint and its own id and keys, all MAC'd
//! with `s`.
//!
//! **Numeric code** ([`Hello`], [`Answer`], [`crate::pake`]): the new device
//! shows 8 digits instead; the two run CPace through the hub, the new device's
//! keys travel in its answer under the responder's confirmation key, and the
//! delivery is MAC'd with the run's pairing secret in place of `s`.
//!
//! Either way the new device knows nothing about the account beforehand. The
//! MAC tells it the delivery came from whoever saw its screen (or typed its
//! code), not from the hub; then it fetches the chain and accepts the delivery
//! only if the chain verifies, its account key has the delivery's fingerprint,
//! the sender is a current full device with the keys the delivery names, and
//! it lists the new device itself with its own keys ([`Delivery::open`]). Only
//! then does it say "paired".

use hmac::{Hmac, Mac};
use rand_core::{CryptoRng, RngCore};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::chain::{ChainState, MAX_ID_LEN};
use crate::codec::{base32, put_str16, DecodeError, Reader};
use crate::identity::fingerprint;
use crate::keys::DhKeypair;
use crate::wrap::{self, pairing_tag, verify_pairing_tag, Purpose, SealContext, SealedKey};

/// What a QR code's text starts with (what [`Offer::to_text`] writes).
pub const OFFER_PREFIX: &str = "RHP1-";
/// What [`Offer::from_text`] accepts in front of the base32, in any case:
/// [`OFFER_PREFIX`] and the newer `DRP1-` for the same version.
pub const OFFER_PREFIXES: [&str; 2] = [OFFER_PREFIX, "DRP1-"];
const VERSION: u8 = 1;
const CI_LABEL: &[u8] = b"rh/v1 pair-ci\0";
const ANSWER_LABEL: &[u8] = b"rh/v1 pake answer\0";
/// The most `UK` epochs one delivery carries.
pub const MAX_KEYS: usize = 256;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PairingError {
    #[error("not a pairing code")]
    BadOffer,
    #[error("the delivery was not made by whoever saw this device's code")]
    BadTag,
    #[error("the delivery is for another account than the chain the hub serves")]
    WrongAccount,
    #[error("the account key does not have the fingerprint the delivery names")]
    Fingerprint,
    #[error("the sender is not a current full device with the keys the delivery names")]
    Sender,
    #[error("the chain does not list this device with its own keys (yet)")]
    NotListed,
    #[error("the answer was not made with the same code")]
    BadAnswer,
    #[error(transparent)]
    Wrap(#[from] wrap::WrapError),
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

/// What a new device shows: who it is, and the secret that MACs its delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub account: String,
    pub device: String,
    pub sign_key: [u8; 32],
    pub dh_key: [u8; 32],
    pub secret: [u8; 32],
}

impl Offer {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![VERSION];
        put_str16(&mut out, &self.account);
        put_str16(&mut out, &self.device);
        out.extend_from_slice(&self.sign_key);
        out.extend_from_slice(&self.dh_key);
        out.extend_from_slice(&self.secret);
        out
    }

    /// `RHP1-` and base32: what the QR code holds, or what is pasted.
    pub fn to_text(&self) -> String {
        format!("{OFFER_PREFIX}{}", base32::encode(&self.encode()))
    }

    /// Reads [`Offer::to_text`]'s text, whitespace anywhere and any case, with
    /// any of [`OFFER_PREFIXES`].
    pub fn from_text(text: &str) -> Result<Self, PairingError> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let body = OFFER_PREFIXES
            .iter()
            .find_map(|prefix| {
                text.get(..prefix.len())
                    .filter(|p| p.eq_ignore_ascii_case(prefix))
                    .map(|_| &text[prefix.len()..])
            })
            .and_then(base32::decode)
            .ok_or(PairingError::BadOffer)?;
        let mut r = Reader::new(&body);
        if r.u8()? != VERSION {
            return Err(PairingError::BadOffer);
        }
        let offer = Self {
            account: r.str16("account id", MAX_ID_LEN)?,
            device: r.str16("device id", MAX_ID_LEN)?,
            sign_key: r.array()?,
            dh_key: r.array()?,
            secret: r.array()?,
        };
        r.finish()?;
        Ok(offer)
    }
}

/// The CPace channel identifier for pairing `device` into `account`.
pub fn channel_id(account: &str, device: &str) -> Vec<u8> {
    [CI_LABEL, account.as_bytes(), b"\0", device.as_bytes()].concat()
}

/// The numeric run's first message (existing device → new device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub sid: [u8; crate::pake::SID_LEN],
    pub share: [u8; 32],
}

impl Hello {
    pub fn encode(&self) -> Vec<u8> {
        [&self.sid[..], &self.share].concat()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PairingError> {
        let mut r = Reader::new(bytes);
        let hello = Self {
            sid: r.array()?,
            share: r.array()?,
        };
        r.finish()?;
        Ok(hello)
    }
}

/// The numeric run's answer (new device → existing device): its share, and
/// its id and keys under the responder's confirmation key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub share: [u8; 32],
    pub device: String,
    pub sign_key: [u8; 32],
    pub dh_key: [u8; 32],
    pub tag: [u8; 32],
}

impl Answer {
    fn mac_body(sid: &[u8], initiator_share: &[u8; 32], share: &[u8; 32], device: &str, sign_key: &[u8; 32], dh_key: &[u8; 32]) -> Vec<u8> {
        let mut out = ANSWER_LABEL.to_vec();
        out.extend_from_slice(sid);
        out.extend_from_slice(initiator_share);
        out.extend_from_slice(share);
        put_str16(&mut out, device);
        out.extend_from_slice(sign_key);
        out.extend_from_slice(dh_key);
        out
    }

    /// The new device's answer to `hello`, after finishing its run (`keys`).
    pub fn new(hello: &Hello, share: [u8; 32], device: &str, sign_key: [u8; 32], dh_key: [u8; 32], keys: &crate::pake::Keys) -> Self {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(keys.confirm_responder.as_ref()).expect("any key length");
        mac.update(&Self::mac_body(&hello.sid, &hello.share, &share, device, &sign_key, &dh_key));
        Self {
            share,
            device: device.into(),
            sign_key,
            dh_key,
            tag: mac.finalize().into_bytes().into(),
        }
    }

    /// The existing device's check: the same code made it.
    pub fn verify(&self, hello: &Hello, keys: &crate::pake::Keys) -> Result<(), PairingError> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(keys.confirm_responder.as_ref()).expect("any key length");
        mac.update(&Self::mac_body(
            &hello.sid,
            &hello.share,
            &self.share,
            &self.device,
            &self.sign_key,
            &self.dh_key,
        ));
        mac.verify_slice(&self.tag).map_err(|_| PairingError::BadAnswer)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.share.to_vec();
        put_str16(&mut out, &self.device);
        out.extend_from_slice(&self.sign_key);
        out.extend_from_slice(&self.dh_key);
        out.extend_from_slice(&self.tag);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PairingError> {
        let mut r = Reader::new(bytes);
        let answer = Self {
            share: r.array()?,
            device: r.str16("device id", MAX_ID_LEN)?,
            sign_key: r.array()?,
            dh_key: r.array()?,
            tag: r.array()?,
        };
        r.finish()?;
        Ok(answer)
    }
}

/// `UK` epochs as a delivery opens them: `(epoch, key)`.
pub type UserKeys = Vec<(u32, Zeroizing<Vec<u8>>)>;

/// The keys an existing device hands a new one, MAC'd with the pairing secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub account: String,
    pub fingerprint: [u8; 16],
    pub sender: String,
    pub sender_sign: [u8; 32],
    pub sender_dh: [u8; 32],
    /// `(UK epoch, sealed UK)`, oldest first.
    pub keys: Vec<(u32, Vec<u8>)>,
    pub tag: [u8; 32],
}

impl Delivery {
    fn body(&self) -> Vec<u8> {
        let mut out = vec![VERSION];
        put_str16(&mut out, &self.account);
        out.extend_from_slice(&self.fingerprint);
        put_str16(&mut out, &self.sender);
        out.extend_from_slice(&self.sender_sign);
        out.extend_from_slice(&self.sender_dh);
        out.extend_from_slice(&(self.keys.len() as u16).to_be_bytes());
        for (epoch, sealed) in &self.keys {
            out.extend_from_slice(&epoch.to_be_bytes());
            out.extend_from_slice(&(sealed.len() as u16).to_be_bytes());
            out.extend_from_slice(sealed);
        }
        out
    }

    /// Seal every `(epoch, UK)` in `user_keys` from `sender` (a full device of
    /// `chain`, with static key `sender_dh`) to `recipient` with key
    /// `recipient_dh`, and MAC it all with `secret`.
    #[allow(clippy::too_many_arguments)]
    pub fn create<R: RngCore + CryptoRng>(
        chain: &ChainState,
        sender: &str,
        sender_sign: [u8; 32],
        sender_dh: &DhKeypair,
        recipient: &str,
        recipient_dh: &[u8; 32],
        user_keys: &[(u32, &[u8])],
        secret: &[u8; 32],
        rng: &mut R,
    ) -> Result<Self, PairingError> {
        let mut keys = Vec::with_capacity(user_keys.len());
        for (epoch, uk) in user_keys {
            let ctx = SealContext {
                purpose: Purpose::UserKey,
                account: chain.account().into(),
                sender: sender.into(),
                recipient: recipient.into(),
                epoch: *epoch,
            };
            keys.push((*epoch, wrap::seal_to(sender_dh, recipient_dh, &ctx, uk, rng)?.encode()));
        }
        let mut delivery = Self {
            account: chain.account().into(),
            fingerprint: fingerprint(&chain.account_key()),
            sender: sender.into(),
            sender_sign,
            sender_dh: sender_dh.public(),
            keys,
            tag: [0; 32],
        };
        delivery.tag = pairing_tag(secret, &[b"delivery", &delivery.body()]);
        Ok(delivery)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.body();
        out.extend_from_slice(&self.tag);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PairingError> {
        let mut r = Reader::new(bytes);
        if r.u8()? != VERSION {
            return Err(PairingError::Decode(DecodeError::Invalid("pairing delivery version")));
        }
        let account = r.str16("account id", MAX_ID_LEN)?;
        let fingerprint = r.array()?;
        let sender = r.str16("device id", MAX_ID_LEN)?;
        let sender_sign = r.array()?;
        let sender_dh = r.array()?;
        let count = usize::from(r.u16()?);
        if count > MAX_KEYS {
            return Err(DecodeError::TooLarge {
                what: "delivered keys",
                len: count,
                max: MAX_KEYS,
            }
            .into());
        }
        let mut keys = Vec::with_capacity(count);
        for _ in 0..count {
            let epoch = r.u32()?;
            let len = usize::from(r.u16()?);
            keys.push((epoch, r.bytes(len)?.to_vec()));
        }
        let tag = r.array()?;
        r.finish()?;
        Ok(Self {
            account,
            fingerprint,
            sender,
            sender_sign,
            sender_dh,
            keys,
            tag,
        })
    }

    /// The MAC check alone: whoever made this knew `secret`.
    pub fn check_tag(&self, secret: &[u8; 32]) -> Result<(), PairingError> {
        if verify_pairing_tag(secret, &[b"delivery", &self.body()], &self.tag) {
            Ok(())
        } else {
            Err(PairingError::BadTag)
        }
    }

    /// Everything the new device checks before it says "paired", and the
    /// `UK` epochs it was handed: the MAC, then the verified `chain` against
    /// the delivery (account, fingerprint, sender), then itself in the chain,
    /// then each sealed key.
    pub fn open(&self, secret: &[u8; 32], chain: &ChainState, me: &str, me_sign: &[u8; 32], me_dh: &DhKeypair) -> Result<UserKeys, PairingError> {
        self.check_tag(secret)?;
        if chain.account() != self.account {
            return Err(PairingError::WrongAccount);
        }
        if fingerprint(&chain.account_key()) != self.fingerprint {
            return Err(PairingError::Fingerprint);
        }
        let sender = chain.admin(&self.sender).ok_or(PairingError::Sender)?;
        if sender.info.sign_key != self.sender_sign || sender.info.dh_key != self.sender_dh {
            return Err(PairingError::Sender);
        }
        let mine = chain.device(me).ok_or(PairingError::NotListed)?;
        if mine.info.sign_key != *me_sign || mine.info.dh_key != me_dh.public() {
            return Err(PairingError::NotListed);
        }
        self.keys
            .iter()
            .map(|(epoch, sealed)| {
                let ctx = SealContext {
                    purpose: Purpose::UserKey,
                    account: self.account.clone(),
                    sender: self.sender.clone(),
                    recipient: me.into(),
                    epoch: *epoch,
                };
                let key = wrap::open_from(me_dh, &self.sender_dh, &ctx, &SealedKey::decode(sealed)?)?;
                // Exactly the key the chain committed for the epoch.
                chain.check_user_key(*epoch, &key).map_err(wrap::WrapError::Uncommitted)?;
                Ok((*epoch, key))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::tests::{device_info, fixture, h};
    use crate::chain::{Action, ChainState, DeviceInfo, Signer};
    use crate::pake::{Cpace, Role};
    use crate::testing::{bytes32, rng};

    struct NewDevice {
        sign: ed25519_dalek::SigningKey,
        dh: DhKeypair,
    }

    fn new_device() -> NewDevice {
        NewDevice {
            sign: ed25519_dalek::SigningKey::from_bytes(&bytes32(400)),
            dh: DhKeypair::from_secret(bytes32(401)),
        }
    }

    /// dev-0 adds `n` (as "dev-n") to the sample chain.
    fn add(chain: &ChainState, f: &crate::chain::tests::Fixture, n: &NewDevice) -> ChainState {
        let mut chain = chain.clone();
        let info = DeviceInfo {
            id: "dev-n".into(),
            sign_key: n.sign.verifying_key().to_bytes(),
            dh_key: n.dh.public(),
            full: true,
        };
        let entry = chain
            .draft(Signer::Device("dev-0".into()), Action::AddDevice(info), 9_000)
            .sign(&f.devices[0].0);
        chain.apply(&entry, &mut h()).unwrap();
        chain
    }

    #[test]
    fn offers_round_trip_as_text_and_junk_is_refused() {
        let n = new_device();
        let offer = Offer {
            account: "acct-1".into(),
            device: "dev-n".into(),
            sign_key: n.sign.verifying_key().to_bytes(),
            dh_key: n.dh.public(),
            secret: bytes32(402),
        };
        let text = offer.to_text();
        assert!(text.starts_with(OFFER_PREFIX) && text.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-'));
        assert_eq!(Offer::from_text(&text).unwrap(), offer);
        assert_eq!(
            Offer::from_text(&format!(" {} ", text.to_lowercase())).unwrap(),
            offer,
            "pasted, any case"
        );
        // `DRP1-` reads as the same offer, any case; `RHP1-` is still what is written.
        let body = &text[OFFER_PREFIX.len()..];
        let dr = format!("DRP1-{body}");
        assert_eq!(Offer::from_text(&dr).unwrap(), offer);
        assert_eq!(Offer::from_text(&format!(" {} \n", dr.to_lowercase())).unwrap(), offer);
        assert!(Offer::from_text(&format!("dRp1-{}", &body[..20])).is_err(), "cut short");
        assert_eq!(Offer::from_text(&format!("Drp1-{body}")).unwrap().to_text(), text);
        assert!(Offer::from_text("RHP1-AAAA").is_err());
        assert!(Offer::from_text("DRP1-AAAA").is_err());
        assert!(Offer::from_text(&text[..text.len() - 8]).is_err(), "cut short");
        assert!(Offer::from_text(&dr[..dr.len() - 8]).is_err(), "cut short");
        assert_eq!(Offer::from_text("RH1-ABC"), Err(PairingError::BadOffer));
        for other in ["RH1-", "DR1-", "RHR1-", "DRR1-", "DRP2-", "RHP2-", "DRP1", "DRP1-RHP1-", ""] {
            assert_eq!(Offer::from_text(&format!("{other}{body}")), Err(PairingError::BadOffer), "{other}");
        }
    }

    #[test]
    fn a_qr_delivery_opens_only_with_its_secret_and_a_chain_that_lists_both_devices() {
        let f = fixture();
        let chain = crate::wrap::tests::committed(&f, &[[0x10; 32], [0x11; 32]], &[]);
        let n = new_device();
        let s = bytes32(402);
        let with_n = add(&chain, &f, &n);
        let uk0 = [0x10u8; 32];
        let uk1 = [0x11u8; 32];
        let delivery = Delivery::create(
            &with_n,
            "dev-0",
            f.devices[0].0.verifying_key().to_bytes(),
            &f.devices[0].1,
            "dev-n",
            &n.dh.public(),
            &[(0, &uk0), (1, &uk1)],
            &s,
            &mut rng(1),
        )
        .unwrap();
        let wire = Delivery::decode(&delivery.encode()).unwrap();
        assert_eq!(wire, delivery);
        let me_sign = n.sign.verifying_key().to_bytes();
        let keys = wire.open(&s, &with_n, "dev-n", &me_sign, &n.dh).unwrap();
        assert_eq!(
            keys.iter().map(|(e, k)| (*e, k.to_vec())).collect::<Vec<_>>(),
            vec![(0, uk0.to_vec()), (1, uk1.to_vec())]
        );
        // Another secret (the hub, which never saw the screen).
        assert_eq!(wire.open(&[9; 32], &with_n, "dev-n", &me_sign, &n.dh), Err(PairingError::BadTag));
        // A chain that does not (yet) list the new device.
        assert_eq!(wire.open(&s, &chain, "dev-n", &me_sign, &n.dh), Err(PairingError::NotListed));
        // A chain whose sender keys differ from the delivery's (a hub-forged sender).
        let mut forged = wire.clone();
        forged.sender_dh = f.devices[1].1.public();
        forged.tag = pairing_tag(&s, &[b"delivery", &forged.body()]);
        assert_eq!(forged.open(&s, &with_n, "dev-n", &me_sign, &n.dh), Err(PairingError::Sender));
        // A delivery naming another account key.
        let mut other = wire.clone();
        other.fingerprint = [7; 16];
        other.tag = pairing_tag(&s, &[b"delivery", &other.body()]);
        assert_eq!(other.open(&s, &with_n, "dev-n", &me_sign, &n.dh), Err(PairingError::Fingerprint));
        // A sender that is not a full device of the chain.
        let mut limited = wire.clone();
        limited.sender = "dev-2".into();
        limited.sender_sign = device_info(&f, 2, false).sign_key;
        limited.sender_dh = device_info(&f, 2, false).dh_key;
        limited.tag = pairing_tag(&s, &[b"delivery", &limited.body()]);
        assert_eq!(limited.open(&s, &with_n, "dev-n", &me_sign, &n.dh), Err(PairingError::Sender));
        // A user key the chain did not commit (a sender that went rogue, or a planted one).
        let rogue = Delivery::create(
            &with_n,
            "dev-0",
            f.devices[0].0.verifying_key().to_bytes(),
            &f.devices[0].1,
            "dev-n",
            &n.dh.public(),
            &[(1, &[0xAA; 32])],
            &s,
            &mut rng(5),
        )
        .unwrap();
        assert!(matches!(
            rogue.open(&s, &with_n, "dev-n", &me_sign, &n.dh),
            Err(PairingError::Wrap(wrap::WrapError::Uncommitted(_)))
        ));
    }

    #[test]
    fn a_numeric_run_carries_the_new_devices_keys_and_one_wrong_code_ends_it() {
        let f = fixture();
        let chain = crate::wrap::tests::committed(&f, &[[0x10; 32]], &[]);
        let n = new_device();
        let ci = channel_id("acct-1", "dev-n");
        let sid = [3u8; 16];
        // The existing device (initiator) types the code the new device shows.
        let (old_run, ya) = Cpace::start(Role::Initiator, b"48151623", &ci, &sid, &mut rng(1)).unwrap();
        let hello = Hello::decode(&Hello { sid, share: ya }.encode()).unwrap();
        let (new_run, yb) = Cpace::start(Role::Responder, b"48151623", &ci, &hello.sid, &mut rng(2)).unwrap();
        let new_keys = new_run.finish(&hello.share).unwrap();
        let answer = Answer::new(&hello, yb, "dev-n", n.sign.verifying_key().to_bytes(), n.dh.public(), &new_keys);
        let answer = Answer::decode(&answer.encode()).unwrap();
        let old_keys = old_run.finish(&answer.share).unwrap();
        answer.verify(&hello, &old_keys).unwrap();
        // The delivery, MAC'd with the run's secret, opens on the new device.
        let with_n = add(&chain, &f, &n);
        let delivery = Delivery::create(
            &with_n,
            "dev-0",
            f.devices[0].0.verifying_key().to_bytes(),
            &f.devices[0].1,
            "dev-n",
            &answer.dh_key,
            &[(0, &[0x10u8; 32][..])],
            &old_keys.pair_secret,
            &mut rng(3),
        )
        .unwrap();
        assert!(delivery
            .open(&new_keys.pair_secret, &with_n, "dev-n", &n.sign.verifying_key().to_bytes(), &n.dh)
            .is_ok());

        // A wrong code: the answer does not verify, and the attacker learned one guess's worth.
        let (bad_run, ya) = Cpace::start(Role::Initiator, b"00000000", &ci, &sid, &mut rng(4)).unwrap();
        let hello = Hello { sid, share: ya };
        let (new_run, yb) = Cpace::start(Role::Responder, b"48151623", &ci, &sid, &mut rng(5)).unwrap();
        let new_keys = new_run.finish(&hello.share).unwrap();
        let answer = Answer::new(&hello, yb, "dev-n", n.sign.verifying_key().to_bytes(), n.dh.public(), &new_keys);
        let bad_keys = bad_run.finish(&answer.share).unwrap();
        assert_eq!(answer.verify(&hello, &bad_keys), Err(PairingError::BadAnswer));
        // Keys swapped in transit (the hub substituting its own) do not verify either.
        let (old_run, ya) = Cpace::start(Role::Initiator, b"48151623", &ci, &sid, &mut rng(6)).unwrap();
        let hello = Hello { sid, share: ya };
        let (new_run, yb) = Cpace::start(Role::Responder, b"48151623", &ci, &sid, &mut rng(7)).unwrap();
        let new_keys = new_run.finish(&hello.share).unwrap();
        let mut answer = Answer::new(&hello, yb, "dev-n", n.sign.verifying_key().to_bytes(), n.dh.public(), &new_keys);
        answer.dh_key = f.devices[1].1.public();
        let old_keys = old_run.finish(&answer.share).unwrap();
        assert_eq!(answer.verify(&hello, &old_keys), Err(PairingError::BadAnswer));
    }
}
