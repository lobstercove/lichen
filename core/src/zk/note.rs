//! Shielded note structure.
//!
//! A note represents a hidden value in the shielded pool. Notes are committed
//! to the Merkle tree and encrypted for the recipient.
//!
//! note = { owner, value, blinding, serial }
//! commitment = Poseidon2(value, blinding)
//! nullifier = Poseidon2(serial, spending_key)
//! encrypted_note = ChaCha20-Poly1305(note, shared_secret)

use super::keys::SpendingKey;
use super::merkle::nullifier_hash;
use super::pedersen::ValueCommitment;
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng as AeadOsRng},
    ChaCha20Poly1305, Nonce,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A shielded note (plaintext, only known to owner).
#[derive(Clone, Debug)]
pub struct Note {
    pub owner: [u8; 32],
    pub value: u64,
    pub blinding: [u8; 32],
    pub serial: [u8; 32],
}

/// Nullifier: unique tag that marks a note as spent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nullifier(pub [u8; 32]);

/// Encrypted note (stored on-chain, only recipient can decrypt).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncryptedNote {
    pub ciphertext: Vec<u8>,
    pub ephemeral_pk: [u8; 32],
    pub commitment: [u8; 32],
}

impl Note {
    pub fn new(owner: [u8; 32], value: u64, blinding: [u8; 32], serial: [u8; 32]) -> Self {
        Self {
            owner,
            value,
            blinding,
            serial,
        }
    }

    pub fn commitment(&self) -> ValueCommitment {
        ValueCommitment::commit(self.value, self.blinding)
    }

    pub fn commitment_hash(&self) -> [u8; 32] {
        self.commitment().to_hash()
    }

    pub fn commitment_leaf(&self) -> [u8; 32] {
        self.commitment_hash()
    }

    pub fn nullifier(&self, spending_key: &SpendingKey) -> Nullifier {
        Nullifier(nullifier_hash(&self.serial, &spending_key.to_bytes()))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(32 + 8 + 32 + 32);
        bytes.extend_from_slice(&self.owner);
        bytes.extend_from_slice(&self.value.to_le_bytes());
        bytes.extend_from_slice(&self.blinding);
        bytes.extend_from_slice(&self.serial);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 104 {
            return Err("note bytes too short");
        }

        let mut owner = [0u8; 32];
        owner.copy_from_slice(&bytes[0..32]);

        let mut value_bytes = [0u8; 8];
        value_bytes.copy_from_slice(&bytes[32..40]);
        let value = u64::from_le_bytes(value_bytes);

        let mut blinding = [0u8; 32];
        blinding.copy_from_slice(&bytes[40..72]);

        let mut serial = [0u8; 32];
        serial.copy_from_slice(&bytes[72..104]);

        Ok(Self {
            owner,
            value,
            blinding,
            serial,
        })
    }

    pub fn encrypt(&self, recipient_viewing_key: &[u8; 32]) -> EncryptedNote {
        let note_bytes = self.to_bytes();
        let commitment_hash = self.commitment_hash();

        use chacha20poly1305::aead::rand_core::RngCore;
        let mut ephemeral_pk = [0u8; 32];
        AeadOsRng.fill_bytes(&mut ephemeral_pk);

        let mut key_hasher = Sha256::new();
        key_hasher.update(ephemeral_pk);
        key_hasher.update(recipient_viewing_key);
        let encryption_key: [u8; 32] = key_hasher.finalize().into();

        let mut nonce_hasher = Sha256::new();
        nonce_hasher.update(encryption_key);
        nonce_hasher.update(b"nonce");
        let nonce_material: [u8; 32] = nonce_hasher.finalize().into();
        let nonce = Nonce::from_slice(&nonce_material[..12]);

        let cipher = ChaCha20Poly1305::new_from_slice(&encryption_key)
            .unwrap_or_else(|e| panic!("FATAL: ChaCha20 key init with 32-byte key failed: {}", e));
        let ciphertext = cipher
            .encrypt(nonce, note_bytes.as_ref())
            .map_err(|_| "note encryption failed")
            .expect("ChaCha20-Poly1305 encrypt of well-formed plaintext");

        EncryptedNote {
            ciphertext,
            ephemeral_pk,
            commitment: commitment_hash,
        }
    }

    pub fn decrypt(
        encrypted: &EncryptedNote,
        viewing_key: &[u8; 32],
    ) -> Result<Self, &'static str> {
        let mut key_hasher = Sha256::new();
        key_hasher.update(encrypted.ephemeral_pk);
        key_hasher.update(viewing_key);
        let decryption_key: [u8; 32] = key_hasher.finalize().into();

        let mut nonce_hasher = Sha256::new();
        nonce_hasher.update(decryption_key);
        nonce_hasher.update(b"nonce");
        let nonce_material: [u8; 32] = nonce_hasher.finalize().into();
        let nonce = Nonce::from_slice(&nonce_material[..12]);

        let cipher =
            ChaCha20Poly1305::new_from_slice(&decryption_key).map_err(|_| "invalid key length")?;
        let plaintext = cipher
            .decrypt(nonce, encrypted.ciphertext.as_ref())
            .map_err(|_| "decryption failed — wrong key or corrupted ciphertext")?;

        let note = Self::from_bytes(&plaintext)?;

        let expected_commitment = note.commitment_hash();
        if expected_commitment != encrypted.commitment {
            return Err("commitment mismatch — wrong key or corrupted note");
        }

        Ok(note)
    }
}

impl Nullifier {
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, String> {
        let bytes = hex::decode(s).map_err(|e| format!("invalid hex: {}", e))?;
        if bytes.len() != 32 {
            return Err("nullifier must be 32 bytes".to_string());
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zk::merkle::random_scalar_bytes;

    fn test_note() -> Note {
        Note::new(
            [1u8; 32],
            1_000_000_000,
            random_scalar_bytes(),
            random_scalar_bytes(),
        )
    }

    #[test]
    fn test_note_commitment() {
        let note = test_note();
        let c1 = note.commitment_hash();
        let c2 = note.commitment_hash();
        assert_eq!(c1, c2);
        assert_ne!(c1, [0u8; 32]);
    }

    #[test]
    fn test_nullifier_deterministic() {
        let note = test_note();
        let sk = SpendingKey(random_scalar_bytes());
        let n1 = note.nullifier(&sk);
        let n2 = note.nullifier(&sk);
        assert_eq!(n1, n2);
    }

    #[test]
    fn test_nullifier_different_keys() {
        let note = test_note();
        let sk1 = SpendingKey(random_scalar_bytes());
        let sk2 = SpendingKey(random_scalar_bytes());
        assert_ne!(note.nullifier(&sk1), note.nullifier(&sk2));
    }

    #[test]
    fn test_note_serialization() {
        let note = test_note();
        let bytes = note.to_bytes();
        let restored = Note::from_bytes(&bytes).unwrap();
        assert_eq!(note.owner, restored.owner);
        assert_eq!(note.value, restored.value);
        assert_eq!(note.blinding, restored.blinding);
        assert_eq!(note.serial, restored.serial);
    }

    #[test]
    fn test_note_encrypt_decrypt() {
        let viewing_key = [42u8; 32];
        let note = test_note();
        let encrypted = note.encrypt(&viewing_key);
        let decrypted = Note::decrypt(&encrypted, &viewing_key).unwrap();
        assert_eq!(note.owner, decrypted.owner);
        assert_eq!(note.value, decrypted.value);
        assert_eq!(note.blinding, decrypted.blinding);
        assert_eq!(note.serial, decrypted.serial);
    }

    #[test]
    fn test_note_decrypt_wrong_key() {
        let viewing_key = [42u8; 32];
        let wrong_key = [99u8; 32];
        let note = test_note();
        let encrypted = note.encrypt(&viewing_key);
        let result = Note::decrypt(&encrypted, &wrong_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_nullifier_hex_roundtrip() {
        let nullifier = Nullifier([0xAB; 32]);
        let hex = nullifier.to_hex();
        let restored = Nullifier::from_hex(&hex).unwrap();
        assert_eq!(nullifier, restored);
    }

    #[test]
    fn test_commitment_different_values() {
        let note1 = Note::new([1u8; 32], 100, [42u8; 32], [99u8; 32]);
        let note2 = Note::new([1u8; 32], 200, [42u8; 32], [99u8; 32]);
        assert_ne!(note1.commitment_leaf(), note2.commitment_leaf());
    }

    #[test]
    fn test_commitment_hash_matches_native_commitment() {
        let note = test_note();
        assert_eq!(note.commitment_hash(), note.commitment().to_bytes());
    }
}
