//! Internal field-bridge helpers kept only for the private witness-adapter path.
//!
//! The native shielded runtime no longer uses these functions directly. They
//! remain solely because the dormant arkworks circuits still encode witness
//! values as field elements.

use ark_bn254::Fr;
use ark_crypto_primitives::sponge::poseidon::{PoseidonConfig, PoseidonSponge};
use ark_crypto_primitives::sponge::CryptographicSponge;
use ark_ff::{BigInteger, Field, PrimeField};
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::merkle::TREE_DEPTH;

#[allow(dead_code)]
pub(crate) fn poseidon_hash_fr(left: Fr, right: Fr) -> Fr {
    let config = poseidon_config();
    let mut sponge = PoseidonSponge::<Fr>::new(&config);
    sponge.absorb(&left);
    sponge.absorb(&right);
    let result: Vec<Fr> = sponge.squeeze_field_elements(1);
    result[0]
}

pub(crate) fn fr_to_bytes(fr: &Fr) -> [u8; 32] {
    let mut output = [0u8; 32];
    let bytes = fr.into_bigint().to_bytes_le();
    let len = std::cmp::min(bytes.len(), 32);
    output[..len].copy_from_slice(&bytes[..len]);
    output
}

#[allow(dead_code)]
pub(crate) fn bytes_to_fr(bytes: &[u8; 32]) -> Fr {
    Fr::from_le_bytes_mod_order(bytes)
}

pub(crate) fn poseidon_config() -> PoseidonConfig<Fr> {
    let full_rounds = 8;
    let partial_rounds = 57;
    let alpha = 5;

    let width = 3;
    let total_rounds = full_rounds + partial_rounds;

    let mut round_constants: Vec<Vec<Fr>> = Vec::new();
    for r in 0..total_rounds {
        let mut round_rc = Vec::new();
        for w in 0..width {
            let i: u64 = (r * width + w) as u64;
            let mut hasher = Sha256::new();
            hasher.update(b"Lichen-Poseidon-RC-");
            hasher.update(i.to_le_bytes());
            let hash = hasher.finalize();
            round_rc.push(Fr::from_le_bytes_mod_order(&hash));
        }
        round_constants.push(round_rc);
    }

    let mut mds = Vec::new();
    for i in 0..width {
        let mut row = Vec::new();
        for j in 0..width {
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
#[derive(Clone, Debug)]
pub(crate) struct Bn254MerklePath {
    pub siblings: Vec<Fr>,
    pub path_bits: Vec<bool>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct Bn254MerkleTree {
    leaves: Vec<Fr>,
    empty_hashes: Vec<Fr>,
}

#[cfg(test)]
impl Bn254MerkleTree {
    pub(crate) fn new() -> Self {
        Self {
            leaves: Vec::new(),
            empty_hashes: Self::compute_empty_hashes(),
        }
    }

    fn compute_empty_hashes() -> Vec<Fr> {
        let mut hashes = vec![Fr::from(0u64); TREE_DEPTH + 1];
        for depth in 1..=TREE_DEPTH {
            hashes[depth] = poseidon_hash_fr(hashes[depth - 1], hashes[depth - 1]);
        }
        hashes
    }

    pub(crate) fn insert(&mut self, leaf: Fr) -> u64 {
        let index = self.leaves.len() as u64;
        self.leaves.push(leaf);
        index
    }

    pub(crate) fn root(&self) -> Fr {
        if self.leaves.is_empty() {
            return self.empty_hashes[TREE_DEPTH];
        }

        let mut current_level = self.leaves.clone();
        for depth in 0..TREE_DEPTH {
            let mut next_level = Vec::with_capacity(current_level.len().div_ceil(2));
            let pairs = current_level.len().div_ceil(2);
            for pair in 0..pairs {
                let left = current_level[pair * 2];
                let right = if pair * 2 + 1 < current_level.len() {
                    current_level[pair * 2 + 1]
                } else {
                    self.empty_hashes[depth]
                };
                next_level.push(poseidon_hash_fr(left, right));
            }
            current_level = next_level;
        }

        current_level[0]
    }

    pub(crate) fn proof(&self, index: u64) -> Option<Bn254MerklePath> {
        let index_usize = index as usize;
        if index_usize >= self.leaves.len() {
            return None;
        }

        let mut siblings = Vec::with_capacity(TREE_DEPTH);
        let mut path_bits = Vec::with_capacity(TREE_DEPTH);
        let mut current_level = self.leaves.clone();
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

            let mut next_level = Vec::with_capacity(current_level.len().div_ceil(2));
            let pairs = current_level.len().div_ceil(2);
            for pair in 0..pairs {
                let left = current_level[pair * 2];
                let right = if pair * 2 + 1 < current_level.len() {
                    current_level[pair * 2 + 1]
                } else {
                    self.empty_hashes[depth]
                };
                next_level.push(poseidon_hash_fr(left, right));
            }

            current_level = next_level;
            current_index /= 2;
        }

        Some(Bn254MerklePath {
            siblings,
            path_bits,
        })
    }
}
