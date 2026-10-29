//! The fingerprint of this machine's keys, which the local settings, the
//! droppen.app consent page and a renewal's notes show.

use rh_proto::identity::MachineIdentity;

/// The label [`machine_fingerprint`] hashes under (the web's
/// `machineFingerprint` in `web/src/features/link/handoff.ts` uses the same).
pub const MACHINE_FINGERPRINT_LABEL: &[u8] = b"rh/v1 machine fingerprint\0";

/// The fingerprint of this machine's keys as the local settings and the
/// droppen.app consent page both show it: the first 16 bytes of
/// SHA-256(label ‖ signing key ‖ channel key), in 8 groups of 4 hex digits.
pub fn machine_fingerprint(identity: &MachineIdentity) -> String {
    machine_fingerprint_of(&identity.sign_public(), &identity.dh_public())
}

/// [`machine_fingerprint`] of two public keys.
pub fn machine_fingerprint_of(sign: &[u8; 32], dh: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(MACHINE_FINGERPRINT_LABEL);
    h.update(sign);
    h.update(dh);
    let digest = h.finalize();
    digest[..16]
        .chunks(2)
        .map(droppen_base::text::hex)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_fingerprint_is_eight_groups_of_four_hex_digits() {
        let identity = MachineIdentity::from_seed(&[3; 32]);
        let fp = machine_fingerprint(&identity);
        assert_eq!(fp.split(' ').count(), 8);
        assert!(fp.split(' ').all(|g| g.len() == 4 && g.bytes().all(|b| b.is_ascii_hexdigit())));
        assert_eq!(fp, machine_fingerprint(&MachineIdentity::from_seed(&[3; 32])), "stable");
        assert_ne!(fp, machine_fingerprint(&MachineIdentity::from_seed(&[4; 32])));
        // The vector the web's `machineFingerprint` is tested against.
        assert_eq!(machine_fingerprint_of(&[1; 32], &[2; 32]), MACHINE_FINGERPRINT_VECTOR);
    }

    /// `machine_fingerprint_of(&[1; 32], &[2; 32])`, also in `web/test/link-handoff.test.ts`.
    const MACHINE_FINGERPRINT_VECTOR: &str = "d4c7 841d 009c f90b 2395 fcd4 4a5a 52fd";
}
