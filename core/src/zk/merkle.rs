//! Poseidon2-based sparse Merkle tree for the native shielded runtime.
//!
//! The live runtime uses domain-separated Poseidon2/Goldilocks hashing over
//! canonical 32-byte byte strings for commitments, nullifiers, and Merkle
//! nodes.

use ark_std::rand::{rngs::OsRng, RngCore};
use p3_field::PrimeField64;
use p3_goldilocks::{default_goldilocks_poseidon2_8, Goldilocks, Poseidon2Goldilocks};
use p3_symmetric::{CryptographicHasher, PaddingFreeSponge};
use serde::{Deserialize, Serialize};

type NativePermutation = Poseidon2Goldilocks<8>;
type NativeHasher = PaddingFreeSponge<NativePermutation, 8, 4, 4>;

const DOMAIN_MERKLE_NODE: u64 = 0x4c49434e4d524b31;
const DOMAIN_COMMITMENT: u64 = 0x4c49434e434d5431;
const DOMAIN_NULLIFIER: u64 = 0x4c49434e4e554c31;
const DOMAIN_RECIPIENT: u64 = 0x4c49434e52435031;
const DOMAIN_RECIPIENT_PREIMAGE: u64 = 0x4c49434e52504331;
const DOMAIN_RANDOM_SCALAR: u64 = 0x4c49434e524e4431;

const CANONICAL_SCALAR_MODULUS_LE: [u8; 32] = [
    1, 0, 0, 240, 147, 245, 225, 67, 145, 112, 185, 121, 72, 232, 51, 40, 93, 88, 129, 129, 182,
    69, 80, 184, 41, 160, 49, 225, 114, 78, 100, 48,
];

const BYTES32_WORDS: usize = 8;

/// Tree depth: supports 2^20 = ~1 million commitments.
pub const TREE_DEPTH: usize = 20;

fn poseidon2_hasher() -> NativeHasher {
    NativeHasher::new(default_goldilocks_poseidon2_8())
}

fn u64_to_words(value: u64) -> [u64; 2] {
    [value & 0xFFFF_FFFF, (value >> 32) & 0xFFFF_FFFF]
}

fn bytes32_to_words(bytes: &[u8; 32]) -> [u64; BYTES32_WORDS] {
    let mut words = [0u64; BYTES32_WORDS];
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        words[index] = u32::from_le_bytes(chunk.try_into().expect("4-byte limb")) as u64;
    }
    words
}

fn digest_to_bytes(digest: [Goldilocks; 4]) -> [u8; 32] {
    let mut output = [0u8; 32];
    for (index, word) in digest.into_iter().enumerate() {
        output[index * 8..(index + 1) * 8].copy_from_slice(&word.as_canonical_u64().to_le_bytes());
    }
    output
}

pub(crate) fn is_canonical_scalar_bytes(bytes: &[u8; 32]) -> bool {
    for (candidate, modulus) in bytes.iter().zip(CANONICAL_SCALAR_MODULUS_LE.iter()).rev() {
        if candidate < modulus {
            return true;
        }
        if candidate > modulus {
            return false;
        }
    }

    false
}

fn canonical_poseidon2_hash(domain: u64, input_words: &[u64]) -> [u8; 32] {
    let hasher = poseidon2_hasher();

    for counter in 0u64.. {
        let digest = hasher.hash_iter(
            std::iter::once(Goldilocks::new(domain))
                .chain(std::iter::once(Goldilocks::new(counter)))
                .chain(input_words.iter().copied().map(Goldilocks::new)),
        );

        let mut output = digest_to_bytes(digest);
        output[31] &= 0x3F;
        if is_canonical_scalar_bytes(&output) {
            return output;
        }
    }

    unreachable!("u64 counter exhausted while canonicalizing Poseidon2 digest")
}

pub(crate) fn scalar_bytes_from_seed(domain: u64, seed: [u8; 32]) -> [u8; 32] {
    canonical_poseidon2_hash(domain, &bytes32_to_words(&seed))
}

/// Generate a random canonical 32-byte scalar-compatible value.
pub fn random_scalar_bytes() -> [u8; 32] {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    scalar_bytes_from_seed(DOMAIN_RANDOM_SCALAR, seed)
}

/// Derive the canonical recipient preimage bytes used by the current witness model.
pub fn recipient_preimage_from_bytes(bytes: [u8; 32]) -> [u8; 32] {
    scalar_bytes_from_seed(DOMAIN_RECIPIENT_PREIMAGE, bytes)
}

/// Native Poseidon2 commitment hash for a value and blinding secret.
pub fn commitment_hash(value: u64, blinding: &[u8; 32]) -> [u8; 32] {
    let mut words = Vec::with_capacity(2 + BYTES32_WORDS);
    words.extend(u64_to_words(value));
    words.extend(bytes32_to_words(blinding));
    canonical_poseidon2_hash(DOMAIN_COMMITMENT, &words)
}

/// Native Poseidon2 nullifier hash for a note serial and spending key.
pub fn nullifier_hash(serial: &[u8; 32], spending_key: &[u8; 32]) -> [u8; 32] {
    let mut words = Vec::with_capacity(BYTES32_WORDS * 2);
    words.extend(bytes32_to_words(serial));
    words.extend(bytes32_to_words(spending_key));
    canonical_poseidon2_hash(DOMAIN_NULLIFIER, &words)
}

/// Native Poseidon2 recipient binding hash.
pub fn recipient_hash(recipient_preimage: &[u8; 32]) -> [u8; 32] {
    canonical_poseidon2_hash(DOMAIN_RECIPIENT, &bytes32_to_words(recipient_preimage))
}

/// A Merkle path (authentication path) for proving leaf membership.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MerklePath {
    /// Sibling hashes from leaf to root (TREE_DEPTH elements)
    pub siblings: Vec<[u8; 32]>,
    /// Path bits (0 = left child, 1 = right child)
    pub path_bits: Vec<bool>,
    /// The leaf index
    pub index: u64,
}

/// Sparse Merkle tree with native Poseidon2 node hashing.
#[derive(Clone, Debug)]
pub struct MerkleTree {
    leaves: Vec<[u8; 32]>,
    empty_hashes: Vec<[u8; 32]>,
}

impl MerkleTree {
    pub fn new() -> Self {
        let empty_hashes = Self::compute_empty_hashes();
        Self {
            leaves: Vec::new(),
            empty_hashes,
        }
    }

    pub fn empty_root() -> [u8; 32] {
        let empty_hashes = Self::compute_empty_hashes();
        empty_hashes[TREE_DEPTH]
    }

    fn compute_empty_hashes() -> Vec<[u8; 32]> {
        let mut hashes = vec![[0u8; 32]; TREE_DEPTH + 1];
        hashes[0] = [0u8; 32];
        for i in 1..=TREE_DEPTH {
            hashes[i] = poseidon_hash_pair(&hashes[i - 1], &hashes[i - 1]);
        }
        hashes
    }

    pub fn insert(&mut self, leaf: [u8; 32]) -> u64 {
        let index = self.leaves.len() as u64;
        self.leaves.push(leaf);
        self.rebuild_path(index as usize);
        index
    }

    pub fn root(&self) -> [u8; 32] {
        if self.leaves.is_empty() {
            return self.empty_hashes[TREE_DEPTH];
        }
        self.compute_root()
    }

    fn compute_root(&self) -> [u8; 32] {
        if self.leaves.is_empty() {
            return self.empty_hashes[TREE_DEPTH];
        }

        let mut current_level: Vec<[u8; 32]> = self.leaves.clone();

        for depth in 0..TREE_DEPTH {
            let mut next_level = Vec::new();
            let pairs = current_level.len().div_ceil(2);

            for i in 0..pairs {
                let left = current_level[i * 2];
                let right = if i * 2 + 1 < current_level.len() {
                    current_level[i * 2 + 1]
                } else {
                    self.empty_hashes[depth]
                };
                next_level.push(poseidon_hash_pair(&left, &right));
            }

            current_level = next_level;
        }

        current_level[0]
    }

    fn rebuild_path(&mut self, _leaf_index: usize) {}

    pub fn proof(&self, index: u64) -> Option<MerklePath> {
        let index_usize = index as usize;
        if index_usize >= self.leaves.len() {
            return None;
        }

        let mut siblings = Vec::with_capacity(TREE_DEPTH);
        let mut path_bits = Vec::with_capacity(TREE_DEPTH);

        let mut current_level: Vec<[u8; 32]> = self.leaves.clone();
        let mut current_index = index_usize;

        for depth in 0..TREE_DEPTH {
            let is_right = current_index % 2 == 1;
            path_bits.push(is_right);

            let sibling_index = if is_right {
                current_index - 1
            } else {
                current_index + 1
            };

            let sibling = if sibling_index < current_level.len() {
                current_level[sibling_index]
            } else {
                self.empty_hashes[depth]
            };
            siblings.push(sibling);

            let mut next_level = Vec::new();
            let pairs = current_level.len().div_ceil(2);
            for i in 0..pairs {
                let left = current_level[i * 2];
                let right = if i * 2 + 1 < current_level.len() {
                    current_level[i * 2 + 1]
                } else {
                    self.empty_hashes[depth]
                };
                next_level.push(poseidon_hash_pair(&left, &right));
            }

            current_level = next_level;
            current_index /= 2;
        }

        Some(MerklePath {
            siblings,
            path_bits,
            index,
        })
    }

    pub fn verify_proof(root: &[u8; 32], leaf: &[u8; 32], proof: &MerklePath) -> bool {
        if proof.siblings.len() != TREE_DEPTH || proof.path_bits.len() != TREE_DEPTH {
            return false;
        }

        let mut current = *leaf;

        for i in 0..TREE_DEPTH {
            let (left, right) = if proof.path_bits[i] {
                (proof.siblings[i], current)
            } else {
                (current, proof.siblings[i])
            };
            current = poseidon_hash_pair(&left, &right);
        }

        current == *root
    }

    pub fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }

    pub fn get_leaf(&self, index: u64) -> Option<[u8; 32]> {
        self.leaves.get(index as usize).copied()
    }
}

impl Default for MerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Poseidon2 hash of two canonical 32-byte nodes.
pub fn poseidon_hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut words = Vec::with_capacity(BYTES32_WORDS * 2);
    words.extend(bytes32_to_words(left));
    words.extend(bytes32_to_words(right));
    canonical_poseidon2_hash(DOMAIN_MERKLE_NODE, &words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_tree() {
        let tree = MerkleTree::new();
        assert_eq!(tree.leaf_count(), 0);
        assert_eq!(tree.root(), MerkleTree::empty_root());
    }

    #[test]
    fn test_random_scalar_bytes_are_canonical() {
        let scalar = random_scalar_bytes();
        assert!(is_canonical_scalar_bytes(&scalar));
        assert_ne!(scalar, [0u8; 32]);
    }

    #[test]
    fn test_recipient_preimage_is_deterministic() {
        let seed = [7u8; 32];
        assert_eq!(
            recipient_preimage_from_bytes(seed),
            recipient_preimage_from_bytes(seed)
        );
    }

    #[test]
    fn test_commitment_hash_depends_on_blinding() {
        let commitment_a = commitment_hash(42, &[1u8; 32]);
        let commitment_b = commitment_hash(42, &[2u8; 32]);
        assert_ne!(commitment_a, commitment_b);
    }

    #[test]
    fn test_insert_and_root_changes() {
        let mut tree = MerkleTree::new();
        let root0 = tree.root();

        tree.insert([1u8; 32]);
        let root1 = tree.root();
        assert_ne!(root0, root1);

        tree.insert([2u8; 32]);
        let root2 = tree.root();
        assert_ne!(root1, root2);
    }

    #[test]
    fn test_merkle_proof_valid() {
        let mut tree = MerkleTree::new();
        let leaf = [42u8; 32];
        let index = tree.insert(leaf);

        let proof = tree.proof(index).unwrap();
        let root = tree.root();
        assert!(MerkleTree::verify_proof(&root, &leaf, &proof));
    }

    #[test]
    fn test_merkle_proof_invalid_leaf() {
        let mut tree = MerkleTree::new();
        tree.insert([42u8; 32]);

        let proof = tree.proof(0).unwrap();
        let root = tree.root();
        let fake_leaf = [99u8; 32];
        assert!(!MerkleTree::verify_proof(&root, &fake_leaf, &proof));
    }

    #[test]
    fn test_multiple_leaves() {
        let mut tree = MerkleTree::new();
        for i in 0..10 {
            let mut leaf = [0u8; 32];
            leaf[0] = i;
            tree.insert(leaf);
        }

        let root = tree.root();
        for i in 0..10 {
            let mut leaf = [0u8; 32];
            leaf[0] = i;
            let proof = tree.proof(i as u64).unwrap();
            assert!(MerkleTree::verify_proof(&root, &leaf, &proof));
        }
    }

    #[test]
    fn test_poseidon2_hash_deterministic() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let h1 = poseidon_hash_pair(&a, &b);
        let h2 = poseidon_hash_pair(&a, &b);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_poseidon2_hash_different_inputs() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let c = [3u8; 32];
        assert_ne!(poseidon_hash_pair(&a, &b), poseidon_hash_pair(&a, &c));
    }
}
