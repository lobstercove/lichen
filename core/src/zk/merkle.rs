//! Poseidon-based Sparse Merkle Tree (32-level)
//!
//! Uses Poseidon hash which is algebraic and SNARK-friendly,
//! approximately 8x cheaper in-circuit than SHA-256.
//!
//! The tree stores note commitments as leaves. The root changes
//! with each insertion. Wallets maintain a local copy for
//! generating Merkle proofs needed by ZK circuits.

use ark_bn254::Fr;
use ark_crypto_primitives::sponge::poseidon::{PoseidonConfig, PoseidonSponge};
use ark_crypto_primitives::sponge::CryptographicSponge;
use ark_ff::{BigInteger, Field, PrimeField};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Tree depth: supports 2^20 = ~1 million commitments.
///
/// Depth 20 is the sweet spot for Groth16/BN254: the trusted setup completes
/// in seconds (vs. minutes for depth 32) and stays within memory limits on
/// standard developer hardware (16 GB).  Production can increase this if
/// needed — the on-chain transaction format (proof, nullifier, root) is
/// independent of tree depth.
pub const TREE_DEPTH: usize = 20;

/// A Merkle path (authentication path) for proving leaf membership
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MerklePath {
    /// Sibling hashes from leaf to root (TREE_DEPTH elements)
    pub siblings: Vec<[u8; 32]>,
    /// Path bits (0 = left child, 1 = right child)
    pub path_bits: Vec<bool>,
    /// The leaf index
    pub index: u64,
}

/// Sparse Merkle Tree with Poseidon hash
#[derive(Clone, Debug)]
pub struct MerkleTree {
    /// All leaf nodes (note commitment hashes)
    leaves: Vec<[u8; 32]>,
    /// Precomputed empty subtree hashes for each level
    empty_hashes: Vec<[u8; 32]>,
}

impl MerkleTree {
    /// Create a new empty Merkle tree
    pub fn new() -> Self {
        let empty_hashes = Self::compute_empty_hashes();
        Self {
            leaves: Vec::new(),
            empty_hashes,
        }
    }

    /// Compute the empty tree root (deterministic constant)
    pub fn empty_root() -> [u8; 32] {
        let empty_hashes = Self::compute_empty_hashes();
        empty_hashes[TREE_DEPTH]
    }

    /// Precompute empty subtree hashes: H(empty, empty) at each level
    fn compute_empty_hashes() -> Vec<[u8; 32]> {
        let mut hashes = vec![[0u8; 32]; TREE_DEPTH + 1];
        // Level 0: empty leaf = zeros
        hashes[0] = [0u8; 32];
        // Each level: hash of two empty children
        for i in 1..=TREE_DEPTH {
            hashes[i] = poseidon_hash_pair(&hashes[i - 1], &hashes[i - 1]);
        }
        hashes
    }

    /// Insert a new leaf (note commitment hash) and return its index
    pub fn insert(&mut self, leaf: [u8; 32]) -> u64 {
        let index = self.leaves.len() as u64;
        self.leaves.push(leaf);

        // Rebuild affected path from leaf to root
        self.rebuild_path(index as usize);

        index
    }

    /// Get the current root hash
    pub fn root(&self) -> [u8; 32] {
        if self.leaves.is_empty() {
            return self.empty_hashes[TREE_DEPTH];
        }
        self.compute_root()
    }

    /// Compute root from current state
    fn compute_root(&self) -> [u8; 32] {
        let n = self.leaves.len();
        if n == 0 {
            return self.empty_hashes[TREE_DEPTH];
        }

        // Build tree bottom-up
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

    /// Rebuild the path from a leaf index to the root.
    ///
    /// AUDIT-FIX CORE-03: This is intentionally a no-op. The tree uses full
    /// O(n) recomputation via `root()` on demand, which is acceptable for
    /// privacy-set trees where leaf count is bounded by shielded pool depth.
    /// Incremental path updates add complexity without meaningful gain for
    /// trees of depth TREE_DEPTH (20).
    #[allow(dead_code)]
    fn rebuild_path(&mut self, _leaf_index: usize) {
        // Full recomputation in root() — O(leaves) per call.
        // See root() for the bottom-up rebuild.
    }

    /// Generate a Merkle proof for the leaf at the given index
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
            // Determine if we're a left or right child
            let is_right = current_index % 2 == 1;
            path_bits.push(is_right);

            // Get sibling
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

            // Move up: compute next level
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

    /// Verify a Merkle proof against a given root
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

    /// Get the number of leaves in the tree
    pub fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }

    /// Get a leaf by index
    pub fn get_leaf(&self, index: u64) -> Option<[u8; 32]> {
        self.leaves.get(index as usize).copied()
    }
}

impl Default for MerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Poseidon hash of two 32-byte inputs (SNARK-friendly)
///
/// Uses Poseidon sponge construction over BN254 scalar field.
/// In-circuit, this is ~8x cheaper than SHA-256.
pub fn poseidon_hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let left_fr = Fr::from_le_bytes_mod_order(left);
    let right_fr = Fr::from_le_bytes_mod_order(right);
    let result = poseidon_hash_fr(left_fr, right_fr);
    fr_to_bytes(&result)
}

/// Poseidon hash of a single 32-byte input
pub fn poseidon_hash_single(input: &[u8; 32]) -> [u8; 32] {
    let fr = Fr::from_le_bytes_mod_order(input);
    let config = poseidon_config();
    let mut sponge = PoseidonSponge::<Fr>::new(&config);
    sponge.absorb(&fr);
    let result: Vec<Fr> = sponge.squeeze_field_elements(1);
    fr_to_bytes(&result[0])
}

/// Poseidon hash of two Fr elements, returning Fr (for use in circuits)
///
/// This is the canonical Poseidon computation shared by both native
/// code and in-circuit gadgets. Circuits must use the same `poseidon_config()`
/// to produce matching hashes.
pub fn poseidon_hash_fr(left: Fr, right: Fr) -> Fr {
    let config = poseidon_config();
    let mut sponge = PoseidonSponge::<Fr>::new(&config);
    sponge.absorb(&left);
    sponge.absorb(&right);
    let result: Vec<Fr> = sponge.squeeze_field_elements(1);
    result[0]
}

/// Convert Fr to its canonical 32-byte little-endian representation
pub fn fr_to_bytes(fr: &Fr) -> [u8; 32] {
    let mut output = [0u8; 32];
    let bytes = fr.into_bigint().to_bytes_le();
    let len = std::cmp::min(bytes.len(), 32);
    output[..len].copy_from_slice(&bytes[..len]);
    output
}

/// Standard Poseidon config for BN254 (rate=2, capacity=1, width=3)
///
/// This config MUST be used by all code that computes or verifies Poseidon
/// hashes: the native Merkle tree, the circuit gadgets, commitment hashes,
/// and nullifier derivation. Using a different config will produce different
/// hashes and break proof verification.
pub fn poseidon_config() -> PoseidonConfig<Fr> {
    // Use standard Poseidon parameters for BN254
    // Full rounds = 8, partial rounds = 57 (for 128-bit security)
    let full_rounds = 8;
    let partial_rounds = 57;
    let alpha = 5; // x^5 S-box

    // Generate round constants and MDS matrix deterministically
    // In production, use the standard Poseidon constants from the paper
    let width = 3; // rate 2 + capacity 1
    let total_rounds = full_rounds + partial_rounds;

    // Round constants (deterministically generated)
    // ark field is Vec<Vec<F>> indexed by [round_num][state_element_index]
    let mut round_constants: Vec<Vec<Fr>> = Vec::new();
    for r in 0..total_rounds {
        let mut round_rc = Vec::new();
        for w in 0..width {
            let i: u64 = (r * width + w) as u64;
            let mut hasher = Sha256::new();
            hasher.update(b"MoltChain-Poseidon-RC-");
            hasher.update(i.to_le_bytes());
            let hash = hasher.finalize();
            round_rc.push(Fr::from_le_bytes_mod_order(&hash));
        }
        round_constants.push(round_rc);
    }

    // MDS matrix (Cauchy matrix construction)
    let mut mds = Vec::new();
    for i in 0..width {
        let mut row = Vec::new();
        for j in 0..width {
            // Use 1/(x_i + y_j) for Cauchy matrix
            let x = Fr::from((i + 1) as u64);
            let y = Fr::from((width + j + 1) as u64);
            let entry = (x + y).inverse().unwrap_or(Fr::from(1u64));
            row.push(entry);
        }
        mds.push(row);
    }

    PoseidonConfig {
        full_rounds: full_rounds as usize,
        partial_rounds: partial_rounds as usize,
        alpha: alpha as u64,
        ark: round_constants,
        mds,
        rate: 2,
        capacity: 1,
    }
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

        // Verify proof for each leaf
        let root = tree.root();
        for i in 0..10 {
            let mut leaf = [0u8; 32];
            leaf[0] = i;
            let proof = tree.proof(i as u64).unwrap();
            assert!(
                MerkleTree::verify_proof(&root, &leaf, &proof),
                "Proof failed for leaf {}",
                i
            );
        }
    }

    #[test]
    fn test_poseidon_hash_deterministic() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let h1 = poseidon_hash_pair(&a, &b);
        let h2 = poseidon_hash_pair(&a, &b);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_poseidon_hash_different_inputs() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let c = [3u8; 32];
        assert_ne!(poseidon_hash_pair(&a, &b), poseidon_hash_pair(&a, &c));
    }
}
