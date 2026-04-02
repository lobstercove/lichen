//! STARK AIR foundation for shielded flows.
//!
//! This module defines the shared Plonky3 proof configuration, canonical
//! Goldilocks public-input layouts, and AIR types consumed by the native
//! shielded prover and verifier path.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_challenger::DuplexChallenger;
use p3_commit::ExtensionMmcs;
use p3_dft::Radix2DitParallel;
use p3_field::{extension::BinomialExtensionField, Field};
use p3_fri::{FriParameters, TwoAdicFriPcs};
use p3_goldilocks::{default_goldilocks_poseidon2_8, Goldilocks, Poseidon2Goldilocks};
use p3_matrix::dense::RowMajorMatrix;
use p3_merkle_tree::MerkleTreeMmcs;
use p3_symmetric::{PaddingFreeSponge, TruncatedPermutation};
use p3_uni_stark::{Proof as StarkProof, StarkConfig};
use serde::{Deserialize, Serialize};

pub type StarkField = Goldilocks;

pub const STARK_TRACE_ROWS: usize = 4;

const STARK_LOG_BLOWUP: usize = 1;
const STARK_LOG_FINAL_POLY_LEN: usize = 0;
const STARK_MAX_LOG_ARITY: usize = 1;
const STARK_NUM_QUERIES: usize = 16;

pub const GOLDILOCKS_WORD_BYTES: usize = 4;
pub const U64_GOLDILOCKS_WORDS: usize = 2;
pub const BYTES32_GOLDILOCKS_WORDS: usize = 8;
pub const SHIELD_STARK_PUBLIC_INPUT_WORDS: usize = U64_GOLDILOCKS_WORDS + BYTES32_GOLDILOCKS_WORDS;
pub const UNSHIELD_STARK_PUBLIC_INPUT_WORDS: usize =
    (BYTES32_GOLDILOCKS_WORDS * 3) + U64_GOLDILOCKS_WORDS;
pub const TRANSFER_STARK_PUBLIC_INPUT_WORDS: usize = BYTES32_GOLDILOCKS_WORDS * 5;
pub const SHIELD_AIR_TRACE_WIDTH: usize = (U64_GOLDILOCKS_WORDS * 2) + BYTES32_GOLDILOCKS_WORDS;

const COL_AMOUNT_START: usize = 0;
const COL_VALUE_START: usize = COL_AMOUNT_START + U64_GOLDILOCKS_WORDS;
const COL_COMMITMENT_START: usize = COL_VALUE_START + U64_GOLDILOCKS_WORDS;

type LichenChallenge = BinomialExtensionField<StarkField, 2>;
type LichenPermutation = Poseidon2Goldilocks<8>;
type LichenHash = PaddingFreeSponge<LichenPermutation, 8, 4, 4>;
type LichenCompress = TruncatedPermutation<LichenPermutation, 2, 4, 8>;
type LichenValMmcs = MerkleTreeMmcs<
    <StarkField as Field>::Packing,
    <StarkField as Field>::Packing,
    LichenHash,
    LichenCompress,
    2,
    4,
>;
type LichenChallengeMmcs = ExtensionMmcs<StarkField, LichenChallenge, LichenValMmcs>;
type LichenDft = Radix2DitParallel<StarkField>;
type LichenPcs = TwoAdicFriPcs<StarkField, LichenDft, LichenValMmcs, LichenChallengeMmcs>;
type LichenChallenger = DuplexChallenger<StarkField, LichenPermutation, 8, 4>;

pub type LichenStarkConfig = StarkConfig<LichenPcs, LichenChallenge, LichenChallenger>;
pub type LichenStarkProof = StarkProof<LichenStarkConfig>;

fn copy_public_input_words<const N: usize>(
    inputs: &[u64],
    start: usize,
    proof_label: &str,
) -> Result<[u64; N], String> {
    let end = start + N;
    let slice = inputs.get(start..end).ok_or_else(|| {
        format!(
            "{} public input layout ended early at word {} (expected {})",
            proof_label, start, end
        )
    })?;
    let mut words = [0u64; N];
    words.copy_from_slice(slice);
    Ok(words)
}

fn expect_public_input_len(
    inputs: &[u64],
    expected: usize,
    proof_label: &str,
) -> Result<(), String> {
    if inputs.len() != expected {
        return Err(format!(
            "{} proof exposed {} Goldilocks public-input words (expected {})",
            proof_label,
            inputs.len(),
            expected
        ));
    }

    Ok(())
}

pub fn build_stark_config() -> LichenStarkConfig {
    let permutation = default_goldilocks_poseidon2_8();
    let hash = LichenHash::new(permutation.clone());
    let compress = LichenCompress::new(permutation.clone());
    let val_mmcs = LichenValMmcs::new(hash, compress, 0);
    let challenge_mmcs = LichenChallengeMmcs::new(val_mmcs.clone());
    let fri_params = FriParameters {
        log_blowup: STARK_LOG_BLOWUP,
        log_final_poly_len: STARK_LOG_FINAL_POLY_LEN,
        max_log_arity: STARK_MAX_LOG_ARITY,
        num_queries: STARK_NUM_QUERIES,
        commit_proof_of_work_bits: 0,
        query_proof_of_work_bits: 0,
        mmcs: challenge_mmcs,
    };
    let pcs = LichenPcs::new(LichenDft::default(), val_mmcs, fri_params);
    let challenger = LichenChallenger::new(permutation);
    LichenStarkConfig::new(pcs, challenger)
}

pub fn deserialize_stark_proof(bytes: &[u8]) -> Result<LichenStarkProof, String> {
    bincode::deserialize(bytes).map_err(|e| format!("invalid STARK proof bytes: {}", e))
}

pub fn u64_to_goldilocks_words(value: u64) -> [u64; U64_GOLDILOCKS_WORDS] {
    [value & 0xFFFF_FFFF, (value >> 32) & 0xFFFF_FFFF]
}

pub fn bytes32_to_goldilocks_words(bytes: [u8; 32]) -> [u64; BYTES32_GOLDILOCKS_WORDS] {
    let mut words = [0u64; BYTES32_GOLDILOCKS_WORDS];
    for (index, chunk) in bytes.chunks_exact(GOLDILOCKS_WORD_BYTES).enumerate() {
        let limb = u32::from_le_bytes(chunk.try_into().expect("4-byte commitment limb"));
        words[index] = u64::from(limb);
    }
    words
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldAirPublicValues {
    pub amount_words: [u64; U64_GOLDILOCKS_WORDS],
    pub commitment_words: [u64; BYTES32_GOLDILOCKS_WORDS],
}

impl ShieldAirPublicValues {
    pub fn new(amount: u64, commitment: [u8; 32]) -> Self {
        Self {
            amount_words: u64_to_goldilocks_words(amount),
            commitment_words: bytes32_to_goldilocks_words(commitment),
        }
    }

    pub fn to_stark_public_inputs(self) -> [u64; SHIELD_STARK_PUBLIC_INPUT_WORDS] {
        let mut public_inputs = [0u64; SHIELD_STARK_PUBLIC_INPUT_WORDS];
        public_inputs[..U64_GOLDILOCKS_WORDS].copy_from_slice(&self.amount_words);
        public_inputs[U64_GOLDILOCKS_WORDS..].copy_from_slice(&self.commitment_words);
        public_inputs
    }

    pub fn as_fields(self) -> [StarkField; SHIELD_STARK_PUBLIC_INPUT_WORDS] {
        self.to_stark_public_inputs().map(StarkField::new)
    }

    pub fn from_stark_public_inputs(inputs: &[u64]) -> Result<Self, String> {
        expect_public_input_len(inputs, SHIELD_STARK_PUBLIC_INPUT_WORDS, "shield")?;

        Ok(Self {
            amount_words: copy_public_input_words(inputs, 0, "shield")?,
            commitment_words: copy_public_input_words(inputs, U64_GOLDILOCKS_WORDS, "shield")?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ShieldAir {
    public_values: [StarkField; SHIELD_STARK_PUBLIC_INPUT_WORDS],
}

impl ShieldAir {
    pub fn new(public_values: ShieldAirPublicValues) -> Self {
        Self {
            public_values: public_values.as_fields(),
        }
    }

    pub fn public_values(&self) -> &[StarkField; SHIELD_STARK_PUBLIC_INPUT_WORDS] {
        &self.public_values
    }
}

impl BaseAir<StarkField> for ShieldAir {
    fn width(&self) -> usize {
        SHIELD_AIR_TRACE_WIDTH
    }
}

impl<AB> Air<AB> for ShieldAir
where
    AB: AirBuilder<F = StarkField>,
{
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();

        for limb in 0..U64_GOLDILOCKS_WORDS {
            builder.assert_eq(
                main.current(COL_VALUE_START + limb).unwrap(),
                main.current(COL_AMOUNT_START + limb).unwrap(),
            );
        }

        let mut first_row = builder.when_first_row();
        for limb in 0..U64_GOLDILOCKS_WORDS {
            first_row.assert_eq(
                main.current(COL_AMOUNT_START + limb).unwrap(),
                self.public_values[limb],
            );
        }
        for limb in 0..BYTES32_GOLDILOCKS_WORDS {
            first_row.assert_eq(
                main.current(COL_COMMITMENT_START + limb).unwrap(),
                self.public_values[U64_GOLDILOCKS_WORDS + limb],
            );
        }

        let mut transition = builder.when_transition();
        for col in 0..SHIELD_AIR_TRACE_WIDTH {
            transition.assert_eq(main.next(col).unwrap(), main.current(col).unwrap());
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConstantTraceAir<const WIDTH: usize> {
    public_values: [StarkField; WIDTH],
}

impl<const WIDTH: usize> ConstantTraceAir<WIDTH> {
    pub fn new(public_values: [StarkField; WIDTH]) -> Self {
        Self { public_values }
    }

    pub fn public_values(&self) -> &[StarkField; WIDTH] {
        &self.public_values
    }
}

impl<const WIDTH: usize> BaseAir<StarkField> for ConstantTraceAir<WIDTH> {
    fn width(&self) -> usize {
        WIDTH
    }
}

impl<AB, const WIDTH: usize> Air<AB> for ConstantTraceAir<WIDTH>
where
    AB: AirBuilder<F = StarkField>,
{
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();

        let mut first_row = builder.when_first_row();
        for column in 0..WIDTH {
            first_row.assert_eq(main.current(column).unwrap(), self.public_values[column]);
        }

        let mut transition = builder.when_transition();
        for column in 0..WIDTH {
            transition.assert_eq(main.next(column).unwrap(), main.current(column).unwrap());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnshieldAirPublicValues {
    pub merkle_root_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub nullifier_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub amount_words: [u64; U64_GOLDILOCKS_WORDS],
    pub recipient_words: [u64; BYTES32_GOLDILOCKS_WORDS],
}

impl UnshieldAirPublicValues {
    pub fn new(
        merkle_root: [u8; 32],
        nullifier: [u8; 32],
        amount: u64,
        recipient: [u8; 32],
    ) -> Self {
        Self {
            merkle_root_words: bytes32_to_goldilocks_words(merkle_root),
            nullifier_words: bytes32_to_goldilocks_words(nullifier),
            amount_words: u64_to_goldilocks_words(amount),
            recipient_words: bytes32_to_goldilocks_words(recipient),
        }
    }

    pub fn to_stark_public_inputs(self) -> [u64; UNSHIELD_STARK_PUBLIC_INPUT_WORDS] {
        let mut public_inputs = [0u64; UNSHIELD_STARK_PUBLIC_INPUT_WORDS];
        public_inputs[..BYTES32_GOLDILOCKS_WORDS].copy_from_slice(&self.merkle_root_words);
        public_inputs[BYTES32_GOLDILOCKS_WORDS..BYTES32_GOLDILOCKS_WORDS * 2]
            .copy_from_slice(&self.nullifier_words);
        public_inputs
            [BYTES32_GOLDILOCKS_WORDS * 2..(BYTES32_GOLDILOCKS_WORDS * 2) + U64_GOLDILOCKS_WORDS]
            .copy_from_slice(&self.amount_words);
        public_inputs[(BYTES32_GOLDILOCKS_WORDS * 2) + U64_GOLDILOCKS_WORDS..]
            .copy_from_slice(&self.recipient_words);
        public_inputs
    }

    pub fn as_fields(self) -> [StarkField; UNSHIELD_STARK_PUBLIC_INPUT_WORDS] {
        self.to_stark_public_inputs().map(StarkField::new)
    }

    pub fn from_stark_public_inputs(inputs: &[u64]) -> Result<Self, String> {
        expect_public_input_len(inputs, UNSHIELD_STARK_PUBLIC_INPUT_WORDS, "unshield")?;

        Ok(Self {
            merkle_root_words: copy_public_input_words(inputs, 0, "unshield")?,
            nullifier_words: copy_public_input_words(inputs, BYTES32_GOLDILOCKS_WORDS, "unshield")?,
            amount_words: copy_public_input_words(
                inputs,
                BYTES32_GOLDILOCKS_WORDS * 2,
                "unshield",
            )?,
            recipient_words: copy_public_input_words(
                inputs,
                (BYTES32_GOLDILOCKS_WORDS * 2) + U64_GOLDILOCKS_WORDS,
                "unshield",
            )?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferAirPublicValues {
    pub merkle_root_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub nullifier_a_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub nullifier_b_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub commitment_c_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub commitment_d_words: [u64; BYTES32_GOLDILOCKS_WORDS],
}

impl TransferAirPublicValues {
    pub fn new(
        merkle_root: [u8; 32],
        nullifier_a: [u8; 32],
        nullifier_b: [u8; 32],
        commitment_c: [u8; 32],
        commitment_d: [u8; 32],
    ) -> Self {
        Self {
            merkle_root_words: bytes32_to_goldilocks_words(merkle_root),
            nullifier_a_words: bytes32_to_goldilocks_words(nullifier_a),
            nullifier_b_words: bytes32_to_goldilocks_words(nullifier_b),
            commitment_c_words: bytes32_to_goldilocks_words(commitment_c),
            commitment_d_words: bytes32_to_goldilocks_words(commitment_d),
        }
    }

    pub fn to_stark_public_inputs(self) -> [u64; TRANSFER_STARK_PUBLIC_INPUT_WORDS] {
        let mut public_inputs = [0u64; TRANSFER_STARK_PUBLIC_INPUT_WORDS];
        public_inputs[..BYTES32_GOLDILOCKS_WORDS].copy_from_slice(&self.merkle_root_words);
        public_inputs[BYTES32_GOLDILOCKS_WORDS..BYTES32_GOLDILOCKS_WORDS * 2]
            .copy_from_slice(&self.nullifier_a_words);
        public_inputs[BYTES32_GOLDILOCKS_WORDS * 2..BYTES32_GOLDILOCKS_WORDS * 3]
            .copy_from_slice(&self.nullifier_b_words);
        public_inputs[BYTES32_GOLDILOCKS_WORDS * 3..BYTES32_GOLDILOCKS_WORDS * 4]
            .copy_from_slice(&self.commitment_c_words);
        public_inputs[BYTES32_GOLDILOCKS_WORDS * 4..].copy_from_slice(&self.commitment_d_words);
        public_inputs
    }

    pub fn as_fields(self) -> [StarkField; TRANSFER_STARK_PUBLIC_INPUT_WORDS] {
        self.to_stark_public_inputs().map(StarkField::new)
    }

    pub fn from_stark_public_inputs(inputs: &[u64]) -> Result<Self, String> {
        expect_public_input_len(inputs, TRANSFER_STARK_PUBLIC_INPUT_WORDS, "transfer")?;

        Ok(Self {
            merkle_root_words: copy_public_input_words(inputs, 0, "transfer")?,
            nullifier_a_words: copy_public_input_words(
                inputs,
                BYTES32_GOLDILOCKS_WORDS,
                "transfer",
            )?,
            nullifier_b_words: copy_public_input_words(
                inputs,
                BYTES32_GOLDILOCKS_WORDS * 2,
                "transfer",
            )?,
            commitment_c_words: copy_public_input_words(
                inputs,
                BYTES32_GOLDILOCKS_WORDS * 3,
                "transfer",
            )?,
            commitment_d_words: copy_public_input_words(
                inputs,
                BYTES32_GOLDILOCKS_WORDS * 4,
                "transfer",
            )?,
        })
    }
}

pub fn build_shield_trace(public_values: ShieldAirPublicValues) -> RowMajorMatrix<StarkField> {
    let mut row = Vec::with_capacity(SHIELD_AIR_TRACE_WIDTH);
    row.extend(public_values.amount_words.map(StarkField::new));
    row.extend(public_values.amount_words.map(StarkField::new));
    row.extend(public_values.commitment_words.map(StarkField::new));

    let mut values = Vec::with_capacity(STARK_TRACE_ROWS * SHIELD_AIR_TRACE_WIDTH);
    for _ in 0..STARK_TRACE_ROWS {
        values.extend_from_slice(&row);
    }

    RowMajorMatrix::new(values, SHIELD_AIR_TRACE_WIDTH)
}

pub fn build_constant_trace<const WIDTH: usize>(
    public_values: &[StarkField; WIDTH],
) -> RowMajorMatrix<StarkField> {
    let mut values = Vec::with_capacity(STARK_TRACE_ROWS * WIDTH);
    for _ in 0..STARK_TRACE_ROWS {
        values.extend_from_slice(public_values);
    }

    RowMajorMatrix::new(values, WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p3_air::check_constraints;

    #[test]
    fn test_u64_words_are_32_bit_limbs() {
        let words = u64_to_goldilocks_words(0x1122_3344_5566_7788);
        assert_eq!(words, [0x5566_7788, 0x1122_3344]);
    }

    #[test]
    fn test_commitment_bytes_use_canonical_32_bit_words() {
        let mut commitment = [0u8; 32];
        commitment[..4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
        commitment[4..8].copy_from_slice(&0x5566_7788u32.to_le_bytes());
        let words = bytes32_to_goldilocks_words(commitment);
        assert_eq!(words[0], 0x1122_3344);
        assert_eq!(words[1], 0x5566_7788);
        assert!(words.iter().all(|word| *word <= u32::MAX as u64));
    }

    #[test]
    fn test_shield_air_accepts_constant_trace() {
        let public_values = ShieldAirPublicValues::new(1_000_000, [7u8; 32]);
        let air = ShieldAir::new(public_values);
        let trace = build_shield_trace(public_values);
        check_constraints(&air, &trace, &[]);
    }

    #[test]
    #[should_panic]
    fn test_shield_air_rejects_value_mismatch() {
        let public_values = ShieldAirPublicValues::new(9, [3u8; 32]);
        let air = ShieldAir::new(public_values);
        let mut trace = build_shield_trace(public_values);
        trace.values[COL_VALUE_START] = StarkField::new(10);
        check_constraints(&air, &trace, &[]);
    }

    #[test]
    fn test_public_input_roundtrips() {
        let shield = ShieldAirPublicValues::new(321, [1u8; 32]);
        assert_eq!(
            ShieldAirPublicValues::from_stark_public_inputs(&shield.to_stark_public_inputs())
                .expect("shield roundtrip"),
            shield
        );

        let unshield = UnshieldAirPublicValues::new([2u8; 32], [3u8; 32], 654, [4u8; 32]);
        assert_eq!(
            UnshieldAirPublicValues::from_stark_public_inputs(&unshield.to_stark_public_inputs())
                .expect("unshield roundtrip"),
            unshield
        );

        let transfer =
            TransferAirPublicValues::new([5u8; 32], [6u8; 32], [7u8; 32], [8u8; 32], [9u8; 32]);
        assert_eq!(
            TransferAirPublicValues::from_stark_public_inputs(&transfer.to_stark_public_inputs())
                .expect("transfer roundtrip"),
            transfer
        );
    }
}
