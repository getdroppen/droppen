//! A software passkey for tests (`docs/passkeys.md` §8): what a browser's
//! `navigator.credentials.create()` and `.get()` hand a relying party —
//! `RegistrationResponseJSON` and `AuthenticationResponseJSON` — made by an
//! authenticator that lives in memory, with a knob for every check the hub
//! makes so each can be failed on purpose ([`Tweak`]).
//!
//! Ed25519 (COSE -8), ES256 (-7) and RS256 (-257), like the hub takes. The
//! RS256 key is one fixed, throwaway 2048-bit key (ring makes no RSA keys):
//! a test fixture, not a secret. Nothing here talks to a hub.

use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, RsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING, RSA_PKCS1_SHA256};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// A throwaway RSA-2048 key (PKCS#8, base64) for RS256 credentials in tests.
const TEST_RSA_PKCS8: [&str; LINES] = [
    "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCf/cldw2gRNvwlvAKDih9BIRMwjYrKByCWZQFH7otlDZytMIGa",
    "Gw1tygkKBktRS51QJfBVmZwPS4q+vhlqnEAT+tPxuJ2+agX9+tOAehddmSlZfvedMZlVBNkKyw3pkNUv4ESvkEDA3XIv1yutlfpW",
    "jlNfAlzz2VxauE/6rmMv0pR2FSBXgu0i6eKm4uTohLdJkpwW8vsWGVOe0OzKMjpaH+mfNRF04PRohblfnmFR00osaixpNzBPXy/k",
    "COnUH1m1lFnruPCXjNY3Wd3uuwBBLKffiAmCr0B5Mib7kLtvXRLj/8QLxPGcwmD3EMjN7EXSIqjNDrAxvXGsn7iYtOODAgMBAAEC",
    "ggEBAI5bGXYlbY6LtzW4ISB7oK2pyVJVTt3OE6/d9ZLUbRGgV/0+AMNnOL6smvhhe1i9iB8NctgnktWFKNHpgeH7X4SnlTgqRals",
    "2yIOsWxKtkmhA0rPj3E99anRY3GYFSvyo84ypcr09rjlEToCIaoVnaawgvwxtRWAHQ3Xm8p4qmacdtNxVFDQsATzdR2aFnNwO5ee",
    "RYQRAI/KlKVV7ZR+Apsi7Rq3D+ifQCHiNbFmX+5zD7/PD1pVZt0hNXu49TIdrlutjQHmpSakS6wxkxj/++f5DKC4E1L1t+I27e8f",
    "Lk5jqMc5DLTTcMIcIJ5Gs/95NShWLh7Zd5BoJjl1zZerV8kCgYEA1BKqNbiK8D0yqFG84aJl+Pz7AWN7YpsG+tL58bHrBOtrC/2/",
    "kE+chlgQB5qDmAhHBTqU0zJgIFQuUHn5XPQC/PKP9hogDMKEY+BVMvWtHR8pfLMy7zM7PrU6NuRdScPD516JsI4iTHauoRh0z9fb",
    "FcfwFCF2r4xSTuDWlEDzir0CgYEAwSF0p6CcT2OQjz7mB/2vTyxWtq+7uiz5u+l1D1uSSa3P14RRGDk5WJQbzfcC0brwnO1g1sB4",
    "iczAiT3Ffnifm1PzzjTb5DecW0fsieC01Z2tZqa60jznoBPG6JZ00c1gO+ocBbTV+LveL+K4rH0/ChyB2K5+N7dV9I6LLgnYKz8C",
    "gYAvlzyxL4emX+VlVmpvrBT8KjFd32hJ+YY4dW33qjiWcwojt77scKEsmMiIHEmZrYnah4nsVyqxNpLqJmUqUev1va1GyhgVJPW2",
    "wBn+hJljpqGVxVURzsplyR+WP+sbaZ4l5tY5U7wosp1/s1BLcCJPIf4jVlsfC/tQji45ONYudQKBgB8p1dgCm5ZX24ixvo6Fidc9",
    "sVRTMPiOr1F6D1CGFynaaRI+oJNHEJDRfVcTx5muD4tS5nBgqFKb/mlm1UpeDmsS9eFNODowGwPDrFBnOObLRaKtvrGT8FAtQMhh",
    "FsKP4DpqwVDOaER1lBmn/OKgZx6rkOw049r7s1v98Elif0YjAoGAMjsQniiA5wMLaCNszIPV8K9vPdvFQJVEOc0jSfTRu0lNzxX8",
    "09lXX1VQ/30RHyHYfZNAZdAvS1/9H318VC3rG4gM6Z+FEsKr0NUfGmKXohXZ4YLQ4mpr53D6N0SQGd9QJFIPY2r8iEPPugk6JQly",
    "Lz/p0G8j3/VTWdRd/mbrGlQ=",
];
const LINES: usize = 17;

pub fn b64u(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

pub fn unb64u(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .expect("base64url")
}

/// The algorithm a software passkey signs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    Ed25519,
    Es256,
    Rs256,
}

enum Key {
    Ed(ed25519_dalek::SigningKey),
    Ec(EcdsaKeyPair),
    Rsa(RsaKeyPair),
}

/// A CBOR item, as much as attestation objects and COSE keys need.
#[derive(Debug, Clone)]
pub enum Cbor {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Map(Vec<(Cbor, Cbor)>),
}

impl Cbor {
    fn head(out: &mut Vec<u8>, major: u8, n: u64) {
        let m = major << 5;
        match n {
            0..=23 => out.push(m | n as u8),
            24..=0xff => out.extend([m | 24, n as u8]),
            0x100..=0xffff => {
                out.push(m | 25);
                out.extend((n as u16).to_be_bytes());
            }
            0x1_0000..=0xffff_ffff => {
                out.push(m | 26);
                out.extend((n as u32).to_be_bytes());
            }
            _ => {
                out.push(m | 27);
                out.extend(n.to_be_bytes());
            }
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Cbor::Int(i) if *i >= 0 => Self::head(out, 0, *i as u64),
            Cbor::Int(i) => Self::head(out, 1, (-1 - *i) as u64),
            Cbor::Bytes(b) => {
                Self::head(out, 2, b.len() as u64);
                out.extend(b);
            }
            Cbor::Text(t) => {
                Self::head(out, 3, t.len() as u64);
                out.extend(t.as_bytes());
            }
            Cbor::Map(entries) => {
                Self::head(out, 5, entries.len() as u64);
                for (k, v) in entries {
                    k.write(out);
                    v.write(out);
                }
            }
        }
    }
}

/// What to get wrong on purpose (each field one check of the hub's).
#[derive(Debug, Clone, Default)]
pub struct Tweak {
    /// The credential's own `type` (`"public-key"`).
    pub credential_type: Option<String>,
    /// The client data's `type` (`"webauthn.create"` / `"webauthn.get"`).
    pub client_type: Option<String>,
    /// Another challenge in the client data than the options gave.
    pub challenge: Option<Vec<u8>>,
    /// Another origin than the ceremony's.
    pub origin: Option<String>,
    /// `crossOrigin` in the client data.
    pub cross_origin: Option<bool>,
    /// The RP id whose hash goes into the authenticator data.
    pub rp_id: Option<String>,
    /// Flags taken off (`0x01` UP, `0x04` UV, `0x40` AT, `0x08` BE) and put on.
    pub flags_off: u8,
    pub flags_on: u8,
    /// The assertion's user handle (`Some(None)`: none at all).
    pub user_handle: Option<Option<Vec<u8>>>,
    /// Flip a bit of the signature.
    pub bad_signature: bool,
    /// The counter to report (else the passkey's own, which `get` advances
    /// when it is counting).
    pub sign_count: Option<u32>,
    /// Another credential id in the JSON than the attested one.
    pub raw_id: Option<Vec<u8>>,
    /// An attestation format other than `none` (with an empty statement).
    pub fmt: Option<String>,
    /// The JSON's `id` alone (not `rawId`) something else.
    pub id: Option<String>,
    /// Attested credential data in an assertion's authenticator data too.
    pub attested_in_assertion: bool,
    /// The client data as a JSON array (type, challenge, origin) where it
    /// should be an object.
    pub client_data_array: bool,
    /// Raw members written into the client data's object before its own
    /// (`"type":"webauthn.create"`, say): a key twice, the first one this.
    pub client_data_first: Option<String>,
}

/// A passkey in memory.
pub struct SoftPasskey {
    pub alg: Alg,
    key: Key,
    pub credential_id: Vec<u8>,
    /// The account's user handle, as `create` was given it in `user.id`.
    pub user_handle: Vec<u8>,
    pub rp_id: String,
    /// The signature counter; 0 and never advanced unless `counting`.
    pub sign_count: u32,
    pub counting: bool,
    /// BE and BS (a synced passkey has both).
    pub backup_eligible: bool,
    pub backup_state: bool,
    pub aaguid: [u8; 16],
}

impl SoftPasskey {
    pub fn new(alg: Alg, rp_id: &str) -> Self {
        let rng = SystemRandom::new();
        let key = match alg {
            Alg::Ed25519 => Key::Ed(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
            Alg::Es256 => {
                let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).expect("P-256 key");
                Key::Ec(EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).expect("P-256 key"))
            }
            Alg::Rs256 => {
                let der = base64::engine::general_purpose::STANDARD
                    .decode(TEST_RSA_PKCS8.concat())
                    .expect("test key");
                Key::Rsa(RsaKeyPair::from_pkcs8(&der).expect("test RSA key"))
            }
        };
        let mut credential_id = vec![0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut credential_id);
        Self {
            alg,
            key,
            credential_id,
            user_handle: Vec::new(),
            rp_id: rp_id.to_string(),
            sign_count: 0,
            counting: false,
            backup_eligible: true,
            backup_state: true,
            aaguid: [0; 16],
        }
    }

    /// The COSE_Key of the public key.
    pub fn cose_key(&self) -> Vec<u8> {
        let entries = match &self.key {
            Key::Ed(k) => vec![
                (Cbor::Int(1), Cbor::Int(1)),
                (Cbor::Int(3), Cbor::Int(-8)),
                (Cbor::Int(-1), Cbor::Int(6)),
                (Cbor::Int(-2), Cbor::Bytes(k.verifying_key().to_bytes().to_vec())),
            ],
            Key::Ec(k) => {
                let point = k.public_key().as_ref();
                vec![
                    (Cbor::Int(1), Cbor::Int(2)),
                    (Cbor::Int(3), Cbor::Int(-7)),
                    (Cbor::Int(-1), Cbor::Int(1)),
                    (Cbor::Int(-2), Cbor::Bytes(point[1..33].to_vec())),
                    (Cbor::Int(-3), Cbor::Bytes(point[33..].to_vec())),
                ]
            }
            Key::Rsa(k) => {
                let public: ring::rsa::PublicKeyComponents<Vec<u8>> = k.public().into();
                vec![
                    (Cbor::Int(1), Cbor::Int(3)),
                    (Cbor::Int(3), Cbor::Int(-257)),
                    (Cbor::Int(-1), Cbor::Bytes(public.n)),
                    (Cbor::Int(-2), Cbor::Bytes(public.e)),
                ]
            }
        };
        Cbor::Map(entries).encode()
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        let rng = SystemRandom::new();
        match &self.key {
            Key::Ed(k) => ed25519_dalek::Signer::sign(k, message).to_bytes().to_vec(),
            Key::Ec(k) => k.sign(&rng, message).expect("ECDSA").as_ref().to_vec(),
            Key::Rsa(k) => {
                let mut sig = vec![0u8; k.public().modulus_len()];
                k.sign(&RSA_PKCS1_SHA256, &rng, message, &mut sig).expect("RSA");
                sig
            }
        }
    }

    fn client_data(kind: &str, challenge: &[u8], origin: &str, tweak: &Tweak) -> Vec<u8> {
        let mut c = json!({
            "type": tweak.client_type.as_deref().unwrap_or(kind),
            "challenge": b64u(tweak.challenge.as_deref().unwrap_or(challenge)),
            "origin": tweak.origin.as_deref().unwrap_or(origin),
        });
        if let Some(cross) = tweak.cross_origin {
            c["crossOrigin"] = json!(cross);
        }
        if tweak.client_data_array {
            c = json!([c["type"], c["challenge"], c["origin"]]);
        }
        let text = serde_json::to_string(&c).expect("JSON");
        match &tweak.client_data_first {
            Some(first) => format!("{{{first},{}", &text[1..]).into_bytes(),
            None => text.into_bytes(),
        }
    }

    fn auth_data(&self, flags: u8, sign_count: u32, attested: bool, tweak: &Tweak) -> Vec<u8> {
        let rp_id = tweak.rp_id.as_deref().unwrap_or(&self.rp_id);
        let mut out = Sha256::digest(rp_id.as_bytes()).to_vec();
        out.push((flags & !tweak.flags_off) | tweak.flags_on);
        out.extend(sign_count.to_be_bytes());
        if attested {
            out.extend(self.aaguid);
            out.extend((self.credential_id.len() as u16).to_be_bytes());
            out.extend(&self.credential_id);
            out.extend(self.cose_key());
        }
        out
    }

    fn flags(&self, attested: bool) -> u8 {
        let mut flags = 0x01 | 0x04;
        if self.backup_eligible {
            flags |= 0x08;
        }
        if self.backup_state {
            flags |= 0x10;
        }
        if attested {
            flags |= 0x40;
        }
        flags
    }

    /// `navigator.credentials.create()` for `options` (a
    /// `PublicKeyCredentialCreationOptionsJSON`) on `origin`: the
    /// `RegistrationResponseJSON`. Keeps `user.id` as its user handle.
    pub fn create(&mut self, options: &Value, origin: &str) -> Value {
        self.create_with(options, origin, &Tweak::default())
    }

    pub fn create_with(&mut self, options: &Value, origin: &str, tweak: &Tweak) -> Value {
        let challenge = unb64u(options["challenge"].as_str().expect("options.challenge"));
        if let Some(user) = options["user"]["id"].as_str() {
            self.user_handle = unb64u(user);
        }
        let client = Self::client_data("webauthn.create", &challenge, origin, tweak);
        let auth = self.auth_data(self.flags(true), tweak.sign_count.unwrap_or(self.sign_count), true, tweak);
        let object = Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text(tweak.fmt.clone().unwrap_or_else(|| "none".into()))),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), Cbor::Bytes(auth)),
        ]);
        let raw_id = tweak.raw_id.clone().unwrap_or_else(|| self.credential_id.clone());
        json!({
            "id": tweak.id.clone().unwrap_or_else(|| b64u(&raw_id)),
            "rawId": b64u(&raw_id),
            "type": tweak.credential_type.as_deref().unwrap_or("public-key"),
            "response": {
                "clientDataJSON": b64u(&client),
                "attestationObject": b64u(&object.encode()),
                "transports": ["internal", "hybrid"],
            },
            "authenticatorAttachment": "platform",
            "clientExtensionResults": {"credProps": {"rk": true}, "prf": {"enabled": true}},
        })
    }

    /// `navigator.credentials.get()` for `options` (a
    /// `PublicKeyCredentialRequestOptionsJSON`) on `origin`: the
    /// `AuthenticationResponseJSON`. A counting passkey's counter goes up.
    pub fn get(&mut self, options: &Value, origin: &str) -> Value {
        self.get_with(options, origin, &Tweak::default())
    }

    pub fn get_with(&mut self, options: &Value, origin: &str, tweak: &Tweak) -> Value {
        let challenge = unb64u(options["challenge"].as_str().expect("options.challenge"));
        if self.counting {
            self.sign_count += 1;
        }
        let client = Self::client_data("webauthn.get", &challenge, origin, tweak);
        let auth = self.auth_data(
            self.flags(tweak.attested_in_assertion),
            tweak.sign_count.unwrap_or(self.sign_count),
            tweak.attested_in_assertion,
            tweak,
        );
        let mut signed = auth.clone();
        signed.extend(Sha256::digest(&client));
        let mut signature = self.sign(&signed);
        if tweak.bad_signature {
            let last = signature.len() - 1;
            signature[last / 2] ^= 0x01;
        }
        let user_handle = match &tweak.user_handle {
            Some(Some(h)) => json!(b64u(h)),
            Some(None) => Value::Null,
            None => json!(b64u(&self.user_handle)),
        };
        let raw_id = tweak.raw_id.clone().unwrap_or_else(|| self.credential_id.clone());
        json!({
            "id": tweak.id.clone().unwrap_or_else(|| b64u(&raw_id)),
            "rawId": b64u(&raw_id),
            "type": tweak.credential_type.as_deref().unwrap_or("public-key"),
            "response": {
                "clientDataJSON": b64u(&client),
                "authenticatorData": b64u(&auth),
                "signature": b64u(&signature),
                "userHandle": user_handle,
            },
            "authenticatorAttachment": "platform",
            "clientExtensionResults": {},
        })
    }
}
