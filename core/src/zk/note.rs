//! Shielded Note Structure
//!
//! A note represents a hidden value in the shielded pool.
//! Notes are committed to the Merkle tree and encrypted for the recipient.
//!
//! note = { owner, value, blinding, serial }
//! commitment = Pedersen(value, blinding)
//! nullifier = Poseidon(serial, spending_key)
//! encrypted_note = ChaCha20-Poly1305(note, shared_secret)

use super::keys::SpendingKey;
use super::merkle::poseidon_hash_pair;
use super::pedersen::PedersenCommitment;
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A shielded note (plaintext, only known to owner)
#[derive(Clone, Debug)]
pub struct Note {
    /// Recipient's shielded public key (viewing key)
    pub owner: [u8; 32],
    /// Amount in shells
    pub value: u64,
    /// Blinding factor for Pedersen commitment
    pub blinding: Fr,
    /// Unique serial number for nullifier derivation
    pub serial: Fr,
}

/// Nullifier: unique tag that marks a note as spent
/// nullifier = Poseidon(serial, spending_key)
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nullifier(pub [u8; 32]);

/// Encrypted note (stored on-chain, only recipient can decrypt)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncryptedNote {
    /// Encrypted note data (ChaCha20-Poly1305)
    pub ciphertext: Vec<u8>,
    /// Ephemeral public key for ECDH key agreement
    pub ephemeral_pk: [u8; 32],
    /// Commitment to the note value
    pub commitment: [u8; 32],
}

impl Note {
    /// Create a new note
    pub fn new(owner: [u8; 32], value: u64, blinding: Fr, serial: Fr) -> Self {
        Self {
            owner,
            value,
            blinding,
            serial,
        }
    }

    /// Compute the Pedersen commitment for this note
    pub fn commitment(&self) -> PedersenCommitment {
        PedersenCommitment::commit(self.value, self.blinding)
    }

    /// Compute the commitment hash (32 bytes, for Merkle tree leaf)
    pub fn commitment_hash(&self) -> [u8; 32] {
        self.commitment().to_hash()
    }

    /// Compute the nullifier for this note using the spending key
    /// nullifier = Poseidon(serial, spending_key)
    pub fn nullifier(&self, spending_key: &SpendingKey) -> Nullifier {
        let serial_bytes = fr_to_bytes(&self.serial);
        let sk_bytes = fr_to_bytes(&spending_key.0);
        Nullifier(poseidon_hash_pair(&serial_bytes, &sk_bytes))
    }

    /// Serialize note to bytes for encryption
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(32 + 8 + 32 + 32);
        bytes.extend_from_slice(&self.owner);
        bytes.extend_from_slice(&self.value.to_le_bytes());
        bytes.extend_from_slice(&fr_to_bytes(&self.blinding));
        bytes.extend_from_slice(&fr_to_bytes(&self.serial));
        bytes
    }

    /// Deserialize note from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 104 {
            // 32 + 8 + 32 + 32
            return Err("note bytes too short");
        }

        let mut owner = [0u8; 32];
        owner.copy_from_slice(&bytes[0..32]);

        let mut value_bytes = [0u8; 8];
        value_bytes.copy_from_slice(&bytes[32..40]);
        let value = u64::from_le_bytes(value_bytes);

        let blinding = Fr::from_le_bytes_mod_order(&bytes[40..72]);
        let serial = Fr::from_le_bytes_mod_order(&bytes[72..104]);

        Ok(Self {
            owner,
            value,
            blinding,
            serial,
        })
    }

    /// Encrypt this note for the recipient using ECDH + ChaCha20-Poly1305
    ///
    /// The sender generates an ephemeral keypair, computes a shared secret
    /// with the recipient's viewing key, and encrypts the note.
    pub fn encrypt(&self, _recipient_viewing_key: &[u8; 32]) -> EncryptedNote {
        // In production: ECDH(ephemeral_sk, recipient_vk) -> shared_secret
        // Then ChaCha20-Poly1305(note_bytes, shared_secret)
        //
        // For now, use a deterministic encryption scheme:
        let note_bytes = self.to_bytes();
        let commitment_hash = self.commitment_hash();

        // Derive ephemeral key (in production: random)
        let mut hasher = Sha256::new();
        hasher.update(b"MoltChain-ephemeral-");
        hasher.update(&note_bytes);
        let ephemeral_pk: [u8; 32] = hasher.finalize().into();

        // Derive encryption key via ECDH simulation
        let mut key_hasher = Sha256::new();
        key_hasher.update(&ephemeral_pk);
        key_hasher.update(_recipient_viewing_key);
        let encryption_key: [u8; 32] = key_hasher.finalize().into();

        // XOR encryption (placeholder — production uses ChaCha20-Poly1305)
        let mut ciphertext = note_bytes.clone();
        for (i, byte) in ciphertext.iter_mut().enumerate() {
            *byte ^= encryption_key[i % 32];
        }

        EncryptedNote {
            ciphertext,
            ephemeral_pk,
            commitment: commitment_hash,
        }
    }

    /// Attempt to decrypt a note using the recipient's spending key
    pub fn decrypt(
        encrypted: &EncryptedNote,
        viewing_key: &[u8; 32],
    ) -> Result<Self, &'static str> {
        // Derive decryption key via ECDH simulation
        let mut key_hasher = Sha256::new();
        key_hasher.update(&encrypted.ephemeral_pk);
        key_hasher.update(viewing_key);
        let decryption_key: [u8; 32] = key_hasher.finalize().into();

        // XOR decryption (placeholder — production uses ChaCha20-Poly1305)
        let mut plaintext = encrypted.ciphertext.clone();
        for (i, byte) in plaintext.iter_mut().enumerate() {
            *byte ^= decryption_key[i % 32];
        }

        let note = Self::from_bytes(&plaintext)?;

        // Verify commitment matches
        let expected_commitment = note.commitment_hash();
        if expected_commitment != encrypted.commitment {
            return Err("commitment mismatch — wrong key or corrupted note");
        }

        Ok(note)
    }
}

impl Nullifier {
    /// Check if this nullifier is the zero nullifier (invalid)
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string
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

/// Convert a field element to 32 bytes (little-endian)
pub fn fr_to_bytes(fr: &Fr) -> [u8; 32] {
    let mut output = [0u8; 32];
    let bigint = fr.into_bigint();
    let bytes = bigint.to_bytes_le();
    let len = std::cmp::min(bytes.len(), 32);
    output[..len].copy_from_slice(&bytes[..len]);
    output
}

/// Convert 32 bytes to a field element
pub fn bytes_to_fr(bytes: &[u8; 32]) -> Fr {
    Fr::from_le_bytes_mod_order(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::UniformRand;
    use ark_std::rand::rngs::OsRng;

    fn test_note() -> Note {
        Note::new(
            [1u8; 32],
            1_000_000_000, // 1 MOLT
            Fr::rand(&mut OsRng),
            Fr::rand(&mut OsRng),
        )
    }

    #[test]
    fn test_note_commitment() {
        let note = test_note();
        let c1 = note.commitment_hash();
        let c2 = note.commitment_hash();
        assert_eq!(c1, c2); // deterministic
        assert_ne!(c1, [0u8; 32]); // non-trivial
    }

    #[test]
    fn test_nullifier_deterministic() {
        let note = test_note();
        let sk = SpendingKey(Fr::rand(&mut OsRng));
        let n1 = note.nullifier(&sk);
        let n2 = note.nullifier(&sk);
        assert_eq!(n1, n2);
    }

    #[test]
    fn test_nullifier_different_keys() {
        let note = test_note();
        let sk1 = SpendingKey(Fr::rand(&mut OsRng));
        let sk2 = SpendingKey(Fr::rand(&mut OsRng));
        assert_ne!(note.nullifier(&sk1), note.nullifier(&sk2));
    }

    #[test]
    fn test_note_serialization() {
        let note = test_note();
        let bytes = note.to_bytes();
        let restored = Note::from_bytes(&bytes).unwrap();
        assert_eq!(note.owner, restored.owner);
        assert_eq!(note.value, restored.value);
    }

    #[test]
    fn test_note_encrypt_decrypt() {
        let viewing_key = [42u8; 32];
        let note = test_note();
        let encrypted = note.encrypt(&viewing_key);
        let decrypted = Note::decrypt(&encrypted, &viewing_key).unwrap();
        assert_eq!(note.owner, decrypted.owner);
        assert_eq!(note.value, decrypted.value);
    }

    #[test]
    fn test_note_decrypt_wrong_key() {
        let viewing_key = [42u8; 32];
        let wrong_key = [99u8; 32];
        let note = test_note();
        let encrypted = note.encrypt(&viewing_key);
        // Wrong key should fail (commitment mismatch or garbage)
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
}
