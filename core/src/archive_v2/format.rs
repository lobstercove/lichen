use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Hash;

pub const ARCHIVE_V2_FORMAT_VERSION: u16 = 2;
pub(crate) const ARCHIVE_V2_SEGMENT_MAGIC: &[u8; 16] = b"LICHEN-ARCHIVE2\0";
pub(crate) const ARCHIVE_V2_MANIFEST_MAGIC: &[u8; 16] = b"LICHEN-AV2-MAN\0\0";
pub(crate) const ARCHIVE_V2_CATALOG_MAGIC: &[u8; 16] = b"LICHEN-AV2-CAT\0\0";
pub(crate) const ARCHIVE_V2_FRAME_HASH_DOMAIN: &[u8] = b"lichen:archive-v2:frame";
pub(crate) const ARCHIVE_V2_ROOT_DOMAIN: &[u8] = b"lichen:archive-v2:root";
pub(crate) const MAX_NETWORK_ID_BYTES: usize = 256;
pub(crate) const MAX_DICTIONARY_BYTES: usize = 128 * 1024;
pub(crate) const MAX_FRAME_COUNT: usize = 1_000_000;
pub(crate) const MAX_FRAME_COMPRESSED_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_FRAME_UNCOMPRESSED_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_SEGMENT_RECORDS: usize = 10_000_000;
pub(crate) const MAX_TRANSACTION_FILTER_BYTES: usize = 4 * 1024 * 1024;
const TRANSACTION_FILTER_TARGET_BITS_PER_ENTRY: u64 = 32;
const MAX_TRANSACTION_FILTER_HASH_FUNCTIONS: u8 = 16;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ArchiveV2Error {
    #[error("archive v2 input is truncated: {0}")]
    Truncated(&'static str),
    #[error("archive v2 input is malformed: {0}")]
    Malformed(String),
    #[error("archive v2 allocation bound exceeded: {0}")]
    Bounds(String),
    #[error("archive v2 object belongs to network {actual}, expected {expected}")]
    WrongNetwork { expected: String, actual: String },
    #[error("archive v2 object has the wrong genesis commitment")]
    WrongGenesis,
    #[error("archive v2 object hash mismatch")]
    WrongObjectHash,
    #[error("archive v2 content root mismatch")]
    WrongRoot,
    #[error("archive v2 canonical continuity mismatch: {0}")]
    Continuity(String),
    #[error("archive v2 duplicate or out-of-order record: {0}")]
    Ordering(String),
    #[error("archive v2 object is unavailable: {0}")]
    Unavailable(String),
    #[error("archive v2 object is being fetched: {0}")]
    Fetching(String),
    #[error("archive v2 I/O error: {0}")]
    Io(String),
    #[error("archive v2 codec error: {0}")]
    Codec(String),
    #[error("archive v2 role admission failed: {0}")]
    Role(String),
}

impl From<std::io::Error> for ArchiveV2Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2Identity {
    pub network_id: String,
    pub genesis_hash: Hash,
}

impl ArchiveV2Identity {
    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.network_id.is_empty() || self.network_id.len() > MAX_NETWORK_ID_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "network id length {} is outside 1..={MAX_NETWORK_ID_BYTES}",
                self.network_id.len()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CodecConfig {
    pub zstd_level: i32,
    pub target_frame_bytes: u32,
    pub max_frame_bytes: u32,
    #[serde(default)]
    pub dictionary: Vec<u8>,
}

impl Default for ArchiveV2CodecConfig {
    fn default() -> Self {
        Self {
            zstd_level: 6,
            target_frame_bytes: 4 * 1024 * 1024,
            max_frame_bytes: 64 * 1024 * 1024,
            dictionary: Vec::new(),
        }
    }
}

impl ArchiveV2CodecConfig {
    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if !(1..=19).contains(&self.zstd_level) {
            return Err(ArchiveV2Error::Bounds(
                "zstd level must be in 1..=19".to_string(),
            ));
        }
        if !(1024 * 1024..=16 * 1024 * 1024).contains(&self.target_frame_bytes) {
            return Err(ArchiveV2Error::Bounds(
                "target frame bytes must be in 1 MiB..=16 MiB".to_string(),
            ));
        }
        if self.max_frame_bytes < self.target_frame_bytes
            || self.max_frame_bytes as usize > MAX_FRAME_COMPRESSED_BYTES
        {
            return Err(ArchiveV2Error::Bounds(
                "max frame bytes must cover the target and be at most 64 MiB".to_string(),
            ));
        }
        if self.dictionary.len() > MAX_DICTIONARY_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "dictionary is {} bytes, maximum is {MAX_DICTIONARY_BYTES}",
                self.dictionary.len()
            )));
        }
        Ok(())
    }

    pub fn dictionary_hash(&self) -> Hash {
        Hash::hash(&self.dictionary)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ArchiveV2FrameKind {
    Transactions = 1,
    Blocks = 2,
    PublicIndexes = 3,
}

impl TryFrom<u8> for ArchiveV2FrameKind {
    type Error = ArchiveV2Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Transactions),
            2 => Ok(Self::Blocks),
            3 => Ok(Self::PublicIndexes),
            _ => Err(ArchiveV2Error::Malformed(format!(
                "unknown frame kind {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2FrameDescriptor {
    pub kind: ArchiveV2FrameKind,
    pub first_ordinal: u64,
    pub record_count: u32,
    pub file_offset: u64,
    pub compressed_bytes: u32,
    pub uncompressed_bytes: u32,
    pub content_hash: Hash,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2PublicIndexes {
    /// Canonical slot to block-frame and record ordinal.
    pub blocks_by_slot: BTreeMap<u64, (u32, u32)>,
    /// Transaction signature to transaction-frame, ordinal, block slot, and
    /// ordinal within the block.
    pub transactions_by_signature: BTreeMap<Hash, (u32, u32, u64, u32)>,
    /// Public-history category rows. Keys and values are the exact canonical
    /// bytes exposed by the legacy RocksDB categories.
    pub categories: BTreeMap<String, Vec<ArchiveV2PublicRow>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2PublicRow {
    pub slot: u64,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CategoryCommitment {
    pub category: String,
    pub row_count: u64,
    pub digest: Hash,
}

/// Compact deterministic negative-membership filter for transaction signatures.
///
/// A negative result is exact (no false negatives); a positive result is only a
/// hint and must still be resolved through the segment's canonical public
/// index. Keeping this filter in the catalog-authenticated manifest prevents a
/// missing transaction lookup from reading every large segment object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2TransactionFilter {
    pub hash_functions: u8,
    pub bit_len: u64,
    pub bits: Vec<u8>,
}

impl ArchiveV2TransactionFilter {
    pub(crate) fn build<'a>(
        signatures: impl IntoIterator<Item = &'a Hash>,
        transaction_count: u64,
    ) -> Result<Self, ArchiveV2Error> {
        if transaction_count == 0 {
            return Ok(Self::default());
        }
        let (hash_functions, bit_len, byte_len) = Self::parameters(transaction_count);
        let mut filter = Self {
            hash_functions,
            bit_len,
            bits: vec![0; byte_len],
        };
        let mut actual_count = 0u64;
        for signature in signatures {
            filter.insert(signature);
            actual_count = actual_count
                .checked_add(1)
                .ok_or_else(|| ArchiveV2Error::Bounds("transaction count overflow".to_string()))?;
        }
        if actual_count != transaction_count {
            return Err(ArchiveV2Error::Ordering(format!(
                "transaction filter received {actual_count} signatures for {transaction_count} transactions"
            )));
        }
        Ok(filter)
    }

    pub fn might_contain(&self, signature: &Hash) -> bool {
        if self.bit_len == 0 {
            return false;
        }
        self.positions(signature)
            .all(|position| self.bits[position / 8] & (1 << (position % 8)) != 0)
    }

    pub(crate) fn validate(&self, transaction_count: u64) -> Result<(), ArchiveV2Error> {
        if transaction_count == 0 {
            if self != &Self::default() {
                return Err(ArchiveV2Error::Ordering(
                    "empty transaction set has a non-empty membership filter".to_string(),
                ));
            }
            return Ok(());
        }
        let (hash_functions, bit_len, byte_len) = Self::parameters(transaction_count);
        if self.hash_functions != hash_functions
            || self.bit_len != bit_len
            || self.bits.len() != byte_len
        {
            return Err(ArchiveV2Error::Ordering(
                "transaction membership filter parameters are non-canonical".to_string(),
            ));
        }
        let used_bits = (self.bits.len() * 8) as u64;
        if used_bits > self.bit_len {
            let padding_mask = !((1u16 << (8 - (used_bits - self.bit_len) as u16)) - 1) as u8;
            if self
                .bits
                .last()
                .is_some_and(|byte| byte & padding_mask != 0)
            {
                return Err(ArchiveV2Error::Ordering(
                    "transaction membership filter has non-zero padding".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn parameters(transaction_count: u64) -> (u8, u64, usize) {
        let bit_len = transaction_count
            .saturating_mul(TRANSACTION_FILTER_TARGET_BITS_PER_ENTRY)
            .min((MAX_TRANSACTION_FILTER_BYTES as u64) * 8)
            .max(64);
        let byte_len = bit_len.div_ceil(8) as usize;
        let bits_per_entry = (bit_len / transaction_count).max(1);
        let hash_functions = ((bits_per_entry.saturating_mul(7)).div_ceil(10))
            .clamp(1, MAX_TRANSACTION_FILTER_HASH_FUNCTIONS as u64)
            as u8;
        (hash_functions, bit_len, byte_len)
    }

    fn insert(&mut self, signature: &Hash) {
        let positions = self.positions(signature).collect::<Vec<_>>();
        for position in positions {
            self.bits[position / 8] |= 1 << (position % 8);
        }
    }

    fn positions(&self, signature: &Hash) -> impl Iterator<Item = usize> + '_ {
        let first = u64::from_le_bytes(signature.0[0..8].try_into().unwrap());
        let step = u64::from_le_bytes(signature.0[8..16].try_into().unwrap()) | 1;
        (0..self.hash_functions).map(move |index| {
            first.wrapping_add((index as u64).wrapping_mul(step)) as usize % self.bit_len as usize
        })
    }
}

impl ArchiveV2PublicIndexes {
    pub fn validate(&self, start_slot: u64, end_slot: u64) -> Result<(), ArchiveV2Error> {
        let expected_slots = end_slot
            .checked_sub(start_slot)
            .and_then(|distance| distance.checked_add(1))
            .ok_or_else(|| ArchiveV2Error::Bounds("invalid segment slot range".to_string()))?;
        if self.blocks_by_slot.len() as u64 != expected_slots {
            return Err(ArchiveV2Error::Ordering(format!(
                "slot index has {} entries for {expected_slots} slots",
                self.blocks_by_slot.len()
            )));
        }
        for slot in start_slot..=end_slot {
            let Some(_) = self.blocks_by_slot.get(&slot) else {
                return Err(ArchiveV2Error::Ordering(format!(
                    "slot index is missing {slot}"
                )));
            };
        }
        for (category, rows) in &self.categories {
            if category.is_empty() || category.len() > 128 {
                return Err(ArchiveV2Error::Bounds(
                    "public index category name is invalid".to_string(),
                ));
            }
            if rows.len() > MAX_SEGMENT_RECORDS {
                return Err(ArchiveV2Error::Bounds(format!(
                    "public index {category} has too many rows"
                )));
            }
            if rows
                .iter()
                .any(|row| row.slot < start_slot || row.slot > end_slot)
            {
                return Err(ArchiveV2Error::Ordering(format!(
                    "public index {category} contains a row outside the segment range"
                )));
            }
            if rows
                .windows(2)
                .any(|pair| pair[0].key.as_slice() >= pair[1].key.as_slice())
            {
                return Err(ArchiveV2Error::Ordering(format!(
                    "public index {category} is duplicated or out of order"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2Manifest {
    pub format_version: u16,
    pub identity: ArchiveV2Identity,
    pub start_slot: u64,
    pub end_slot: u64,
    pub previous_segment_hash: Option<Hash>,
    pub previous_block_hash: Hash,
    pub first_block_hash: Hash,
    pub last_block_hash: Hash,
    pub segment_object_hash: Hash,
    pub segment_content_root: Hash,
    pub index_schema_version: u16,
    pub zstd_level: i32,
    pub target_frame_bytes: u32,
    pub max_frame_bytes: u32,
    pub dictionary_bytes: u32,
    pub dictionary_hash: Hash,
    pub block_count: u64,
    pub transaction_count: u64,
    pub transaction_filter: ArchiveV2TransactionFilter,
    pub public_index_rows: u64,
    pub category_commitments: Vec<ArchiveV2CategoryCommitment>,
    pub frames: Vec<ArchiveV2FrameDescriptor>,
}

impl ArchiveV2Manifest {
    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.format_version != ARCHIVE_V2_FORMAT_VERSION {
            return Err(ArchiveV2Error::Malformed(format!(
                "unsupported format version {}",
                self.format_version
            )));
        }
        self.identity.validate()?;
        if self.end_slot < self.start_slot {
            return Err(ArchiveV2Error::Bounds(
                "segment end precedes start".to_string(),
            ));
        }
        let expected_blocks = self
            .end_slot
            .checked_sub(self.start_slot)
            .and_then(|distance| distance.checked_add(1))
            .ok_or_else(|| ArchiveV2Error::Bounds("segment block count overflow".to_string()))?;
        if self.block_count != expected_blocks {
            return Err(ArchiveV2Error::Ordering(format!(
                "manifest block count {} does not cover its slot range {expected_blocks}",
                self.block_count
            )));
        }
        self.transaction_filter.validate(self.transaction_count)?;
        if self.index_schema_version != 1 {
            return Err(ArchiveV2Error::Malformed(format!(
                "unsupported public index schema version {}",
                self.index_schema_version
            )));
        }
        if self.dictionary_bytes as usize > MAX_DICTIONARY_BYTES {
            return Err(ArchiveV2Error::Bounds(
                "manifest dictionary byte count exceeds the format limit".to_string(),
            ));
        }
        ArchiveV2CodecConfig {
            zstd_level: self.zstd_level,
            target_frame_bytes: self.target_frame_bytes,
            max_frame_bytes: self.max_frame_bytes,
            dictionary: vec![0; self.dictionary_bytes as usize],
        }
        .validate()?;
        if self
            .category_commitments
            .windows(2)
            .any(|pair| pair[0].category >= pair[1].category)
            || self.category_commitments.iter().any(|commitment| {
                commitment.category.is_empty()
                    || commitment.category.len() > 128
                    || commitment.row_count as usize > MAX_SEGMENT_RECORDS
            })
        {
            return Err(ArchiveV2Error::Ordering(
                "manifest category commitments are invalid, duplicated, or out of order"
                    .to_string(),
            ));
        }
        let required_categories = crate::state::PUBLIC_HISTORY_SNAPSHOT_CATEGORIES
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let actual_categories = self
            .category_commitments
            .iter()
            .map(|commitment| commitment.category.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if actual_categories != required_categories
            || actual_categories.len() != self.category_commitments.len()
        {
            return Err(ArchiveV2Error::Ordering(
                "manifest does not commit to the exact public-history category set".to_string(),
            ));
        }
        let committed_rows =
            self.category_commitments
                .iter()
                .try_fold(0u64, |total, commitment| {
                    total.checked_add(commitment.row_count).ok_or_else(|| {
                        ArchiveV2Error::Bounds("category row count overflow".to_string())
                    })
                })?;
        if committed_rows != self.public_index_rows {
            return Err(ArchiveV2Error::Ordering(
                "manifest category commitments do not match public index row count".to_string(),
            ));
        }
        if self.frames.is_empty() || self.frames.len() > MAX_FRAME_COUNT {
            return Err(ArchiveV2Error::Bounds(
                "manifest frame count is invalid".to_string(),
            ));
        }
        let mut prior_end = 0u64;
        for (index, frame) in self.frames.iter().enumerate() {
            if frame.record_count == 0
                || frame.compressed_bytes as usize > MAX_FRAME_COMPRESSED_BYTES
                || frame.uncompressed_bytes as usize > MAX_FRAME_UNCOMPRESSED_BYTES
            {
                return Err(ArchiveV2Error::Bounds(format!(
                    "frame {index} has invalid bounds"
                )));
            }
            if index > 0 && frame.file_offset < prior_end {
                return Err(ArchiveV2Error::Ordering(
                    "frame offsets overlap or move backwards".to_string(),
                ));
            }
            prior_end = frame
                .file_offset
                .saturating_add(frame.compressed_bytes as u64);
        }
        Ok(())
    }
}
