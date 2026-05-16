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
pub const RESERVE_LIABILITY_STARK_PUBLIC_INPUT_WORDS: usize =
    (BYTES32_GOLDILOCKS_WORDS * 3) + (U64_GOLDILOCKS_WORDS * 4);
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

pub fn goldilocks_words_to_u64(
    words: [u64; U64_GOLDILOCKS_WORDS],
    label: &str,
) -> Result<u64, String> {
    if words.iter().any(|word| *word > u32::MAX as u64) {
        return Err(format!(
            "{} public input contains a non-canonical 32-bit limb",
            label
        ));
    }

    Ok(words[0] | (words[1] << 32))
}

pub fn bytes32_to_goldilocks_words(bytes: [u8; 32]) -> [u64; BYTES32_GOLDILOCKS_WORDS] {
    let mut words = [0u64; BYTES32_GOLDILOCKS_WORDS];
    for (index, chunk) in bytes.chunks_exact(GOLDILOCKS_WORD_BYTES).enumerate() {
        let limb = u32::from_le_bytes(chunk.try_into().expect("4-byte commitment limb"));
        words[index] = u64::from(limb);
    }
    words
}

pub fn goldilocks_words_to_bytes32(
    words: [u64; BYTES32_GOLDILOCKS_WORDS],
    label: &str,
) -> Result<[u8; 32], String> {
    let mut bytes = [0u8; 32];
    for (index, word) in words.into_iter().enumerate() {
        if word > u32::MAX as u64 {
            return Err(format!(
                "{} public input contains a non-canonical 32-bit limb",
                label
            ));
        }
        let start = index * GOLDILOCKS_WORD_BYTES;
        bytes[start..start + GOLDILOCKS_WORD_BYTES].copy_from_slice(&(word as u32).to_le_bytes());
    }
    Ok(bytes)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReserveLiabilityAirPublicValues {
    pub domain_hash_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub statement_hash_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub witness_commitment_words: [u64; BYTES32_GOLDILOCKS_WORDS],
    pub reserve_amount_words: [u64; U64_GOLDILOCKS_WORDS],
    pub liability_amount_words: [u64; U64_GOLDILOCKS_WORDS],
    pub epoch_words: [u64; U64_GOLDILOCKS_WORDS],
    pub verifier_version_words: [u64; U64_GOLDILOCKS_WORDS],
}

impl ReserveLiabilityAirPublicValues {
    pub fn new(
        domain_hash: [u8; 32],
        statement_hash: [u8; 32],
        witness_commitment: [u8; 32],
        reserve_amount: u64,
        liability_amount: u64,
        epoch: u64,
        verifier_version: u64,
    ) -> Self {
        Self {
            domain_hash_words: bytes32_to_goldilocks_words(domain_hash),
            statement_hash_words: bytes32_to_goldilocks_words(statement_hash),
            witness_commitment_words: bytes32_to_goldilocks_words(witness_commitment),
            reserve_amount_words: u64_to_goldilocks_words(reserve_amount),
            liability_amount_words: u64_to_goldilocks_words(liability_amount),
            epoch_words: u64_to_goldilocks_words(epoch),
            verifier_version_words: u64_to_goldilocks_words(verifier_version),
        }
    }

    pub fn to_stark_public_inputs(self) -> [u64; RESERVE_LIABILITY_STARK_PUBLIC_INPUT_WORDS] {
        let mut public_inputs = [0u64; RESERVE_LIABILITY_STARK_PUBLIC_INPUT_WORDS];
        let mut cursor = 0;

        public_inputs[cursor..cursor + BYTES32_GOLDILOCKS_WORDS]
            .copy_from_slice(&self.domain_hash_words);
        cursor += BYTES32_GOLDILOCKS_WORDS;
        public_inputs[cursor..cursor + BYTES32_GOLDILOCKS_WORDS]
            .copy_from_slice(&self.statement_hash_words);
        cursor += BYTES32_GOLDILOCKS_WORDS;
        public_inputs[cursor..cursor + BYTES32_GOLDILOCKS_WORDS]
            .copy_from_slice(&self.witness_commitment_words);
        cursor += BYTES32_GOLDILOCKS_WORDS;
        public_inputs[cursor..cursor + U64_GOLDILOCKS_WORDS]
            .copy_from_slice(&self.reserve_amount_words);
        cursor += U64_GOLDILOCKS_WORDS;
        public_inputs[cursor..cursor + U64_GOLDILOCKS_WORDS]
            .copy_from_slice(&self.liability_amount_words);
        cursor += U64_GOLDILOCKS_WORDS;
        public_inputs[cursor..cursor + U64_GOLDILOCKS_WORDS].copy_from_slice(&self.epoch_words);
        cursor += U64_GOLDILOCKS_WORDS;
        public_inputs[cursor..cursor + U64_GOLDILOCKS_WORDS]
            .copy_from_slice(&self.verifier_version_words);

        public_inputs
    }

    pub fn as_fields(self) -> [StarkField; RESERVE_LIABILITY_STARK_PUBLIC_INPUT_WORDS] {
        self.to_stark_public_inputs().map(StarkField::new)
    }

    pub fn from_stark_public_inputs(inputs: &[u64]) -> Result<Self, String> {
        expect_public_input_len(
            inputs,
            RESERVE_LIABILITY_STARK_PUBLIC_INPUT_WORDS,
            "reserve/liability",
        )?;

        let mut cursor = 0;
        let domain_hash_words =
            copy_public_input_words(inputs, cursor, "reserve/liability domain hash")?;
        cursor += BYTES32_GOLDILOCKS_WORDS;
        let statement_hash_words =
            copy_public_input_words(inputs, cursor, "reserve/liability statement hash")?;
        cursor += BYTES32_GOLDILOCKS_WORDS;
        let witness_commitment_words =
            copy_public_input_words(inputs, cursor, "reserve/liability witness commitment")?;
        cursor += BYTES32_GOLDILOCKS_WORDS;
        let reserve_amount_words =
            copy_public_input_words(inputs, cursor, "reserve/liability reserve amount")?;
        cursor += U64_GOLDILOCKS_WORDS;
        let liability_amount_words =
            copy_public_input_words(inputs, cursor, "reserve/liability liability amount")?;
        cursor += U64_GOLDILOCKS_WORDS;
        let epoch_words = copy_public_input_words(inputs, cursor, "reserve/liability epoch")?;
        cursor += U64_GOLDILOCKS_WORDS;
        let verifier_version_words =
            copy_public_input_words(inputs, cursor, "reserve/liability verifier version")?;

        let public_values = Self {
            domain_hash_words,
            statement_hash_words,
            witness_commitment_words,
            reserve_amount_words,
            liability_amount_words,
            epoch_words,
            verifier_version_words,
        };
        public_values.validate()?;
        Ok(public_values)
    }

    pub fn domain_hash(self) -> Result<[u8; 32], String> {
        goldilocks_words_to_bytes32(self.domain_hash_words, "reserve/liability domain hash")
    }

    pub fn statement_hash(self) -> Result<[u8; 32], String> {
        goldilocks_words_to_bytes32(
            self.statement_hash_words,
            "reserve/liability statement hash",
        )
    }

    pub fn witness_commitment(self) -> Result<[u8; 32], String> {
        goldilocks_words_to_bytes32(
            self.witness_commitment_words,
            "reserve/liability witness commitment",
        )
    }

    pub fn reserve_amount(self) -> Result<u64, String> {
        goldilocks_words_to_u64(
            self.reserve_amount_words,
            "reserve/liability reserve amount",
        )
    }

    pub fn liability_amount(self) -> Result<u64, String> {
        goldilocks_words_to_u64(
            self.liability_amount_words,
            "reserve/liability liability amount",
        )
    }

    pub fn epoch(self) -> Result<u64, String> {
        goldilocks_words_to_u64(self.epoch_words, "reserve/liability epoch")
    }

    pub fn verifier_version(self) -> Result<u64, String> {
        goldilocks_words_to_u64(
            self.verifier_version_words,
            "reserve/liability verifier version",
        )
    }

    pub fn solvency_margin(self) -> Result<u64, String> {
        let reserve = self.reserve_amount()?;
        let liability = self.liability_amount()?;
        reserve
            .checked_sub(liability)
            .ok_or_else(|| "reserve/liability statement is undercollateralized".to_string())
    }

    pub fn validate(self) -> Result<(), String> {
        let domain_hash = self.domain_hash()?;
        let statement_hash = self.statement_hash()?;
        let witness_commitment = self.witness_commitment()?;
        if domain_hash == [0u8; 32] {
            return Err("reserve/liability domain hash must be non-zero".to_string());
        }
        if statement_hash == [0u8; 32] {
            return Err("reserve/liability statement hash must be non-zero".to_string());
        }
        if witness_commitment == [0u8; 32] {
            return Err("reserve/liability witness commitment must be non-zero".to_string());
        }
        if self.verifier_version()? == 0 {
            return Err("reserve/liability verifier version must be positive".to_string());
        }
        self.solvency_margin()?;
        Ok(())
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

        let reserve_liability = ReserveLiabilityAirPublicValues::new(
            [10u8; 32], [11u8; 32], [12u8; 32], 9_000, 8_999, 123, 1,
        );
        assert_eq!(
            ReserveLiabilityAirPublicValues::from_stark_public_inputs(
                &reserve_liability.to_stark_public_inputs()
            )
            .expect("reserve/liability roundtrip"),
            reserve_liability
        );
        assert_eq!(
            reserve_liability
                .solvency_margin()
                .expect("reserve/liability margin"),
            1
        );
    }

    #[test]
    fn test_reserve_liability_public_inputs_reject_undercollateralized_statement() {
        let public_values = ReserveLiabilityAirPublicValues::new(
            [10u8; 32], [11u8; 32], [12u8; 32], 100, 101, 123, 1,
        );

        let error = ReserveLiabilityAirPublicValues::from_stark_public_inputs(
            &public_values.to_stark_public_inputs(),
        )
        .expect_err("undercollateralized reserve/liability statement should fail");

        assert!(error.contains("undercollateralized"), "{error}");
    }
}
