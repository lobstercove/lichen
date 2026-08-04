use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::format::{
    ArchiveV2CategoryCommitment, ArchiveV2CodecConfig, ArchiveV2Error, ArchiveV2FrameDescriptor,
    ArchiveV2FrameKind, ArchiveV2Identity, ArchiveV2Manifest, ArchiveV2PublicIndexes,
    ArchiveV2PublicRow, ArchiveV2TransactionFilter, ARCHIVE_V2_FORMAT_VERSION,
    ARCHIVE_V2_FRAME_HASH_DOMAIN, ARCHIVE_V2_MANIFEST_MAGIC, ARCHIVE_V2_ROOT_DOMAIN,
    ARCHIVE_V2_SEGMENT_MAGIC, MAX_DICTIONARY_BYTES, MAX_FRAME_COMPRESSED_BYTES, MAX_FRAME_COUNT,
    MAX_FRAME_UNCOMPRESSED_BYTES, MAX_NETWORK_ID_BYTES, MAX_SEGMENT_RECORDS,
};
use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};
use crate::{Block, Hash, Transaction};

const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 128 * 1024 * 1024;
const SEGMENT_TRAILER_BYTES: usize = 32;
const COMPACT_INDEX_MAGIC: &[u8; 8] = b"AV2IDX1\0";
const COMPACT_INDEX_VERSION: u16 = 1;
const MAX_INDEX_KEY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivedTransactionRecord {
    signature: Hash,
    first_block_slot: u64,
    encoded_transaction: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivedBlockRecord {
    slot: u64,
    block_hash: Hash,
    encoded_skeleton: Vec<u8>,
    transaction_ordinals: Vec<u32>,
}

#[derive(Debug, Clone)]
struct FrameBuild {
    kind: ArchiveV2FrameKind,
    first_ordinal: u64,
    record_count: u32,
    raw: Vec<u8>,
    compressed: Vec<u8>,
    content_hash: Hash,
}

#[derive(Debug, Clone)]
pub struct ArchiveV2SegmentContents {
    pub blocks: Vec<Block>,
    pub public_categories: BTreeMap<String, Vec<ArchiveV2PublicRow>>,
}

impl ArchiveV2SegmentContents {
    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        Self {
            blocks,
            public_categories: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveV2TransactionLocation {
    pub frame_index: u32,
    pub frame_ordinal: u32,
    pub block_slot: u64,
    pub block_ordinal: u32,
}

#[derive(Debug, Clone)]
pub struct ArchiveV2DecodedSegment {
    pub manifest: ArchiveV2Manifest,
    pub blocks: Vec<Block>,
    pub transactions: Vec<Transaction>,
    pub indexes: ArchiveV2PublicIndexes,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ArchiveV2SegmentCodec;

impl ArchiveV2Manifest {
    /// Deterministic manifest encoding: fixed field order through the versioned
    /// Rust struct, fixed-width little-endian legacy bincode, no maps with
    /// nondeterministic iteration, and strict trailing-byte rejection.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, ArchiveV2Error> {
        self.validate()?;
        let payload =
            serialize_legacy_bincode(self, "archive v2 manifest").map_err(ArchiveV2Error::Codec)?;
        if payload.len() > MAX_MANIFEST_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "manifest is {} bytes",
                payload.len()
            )));
        }
        let mut encoded =
            Vec::with_capacity(ARCHIVE_V2_MANIFEST_MAGIC.len() + 4 + payload.len() + 32);
        encoded.extend_from_slice(ARCHIVE_V2_MANIFEST_MAGIC);
        encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&payload);
        encoded.extend_from_slice(&Hash::hash(&payload).0);
        Ok(encoded)
    }

    pub fn decode_canonical(encoded: &[u8]) -> Result<Self, ArchiveV2Error> {
        let minimum = ARCHIVE_V2_MANIFEST_MAGIC.len() + 4 + 32;
        if encoded.len() < minimum {
            return Err(ArchiveV2Error::Truncated("manifest"));
        }
        if !encoded.starts_with(ARCHIVE_V2_MANIFEST_MAGIC) {
            return Err(ArchiveV2Error::Malformed(
                "manifest magic mismatch".to_string(),
            ));
        }
        let length_offset = ARCHIVE_V2_MANIFEST_MAGIC.len();
        let payload_len = u32::from_le_bytes(
            encoded[length_offset..length_offset + 4]
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated("manifest length"))?,
        ) as usize;
        if payload_len > MAX_MANIFEST_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "manifest payload is {payload_len} bytes"
            )));
        }
        let payload_start = length_offset + 4;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or_else(|| ArchiveV2Error::Bounds("manifest length overflow".to_string()))?;
        if payload_end.checked_add(32) != Some(encoded.len()) {
            return Err(ArchiveV2Error::Truncated("manifest payload"));
        }
        let payload = &encoded[payload_start..payload_end];
        if Hash::hash(payload).0 != encoded[payload_end..] {
            return Err(ArchiveV2Error::WrongRoot);
        }
        let manifest = deserialize_legacy_bincode_strict(
            payload,
            MAX_MANIFEST_BYTES as u64,
            "archive v2 manifest",
        )
        .map_err(ArchiveV2Error::Codec)?;
        ArchiveV2Manifest::validate(&manifest)?;
        Ok(manifest)
    }
}

impl ArchiveV2SegmentCodec {
    pub fn encode(
        identity: ArchiveV2Identity,
        previous_segment_hash: Option<Hash>,
        previous_block_hash: Hash,
        contents: &ArchiveV2SegmentContents,
        config: &ArchiveV2CodecConfig,
    ) -> Result<(Vec<u8>, ArchiveV2Manifest), ArchiveV2Error> {
        identity.validate()?;
        config.validate()?;
        let blocks = contents
            .blocks
            .iter()
            .cloned()
            .map(canonical_archive_block)
            .collect::<Vec<_>>();
        let first = blocks
            .first()
            .ok_or_else(|| ArchiveV2Error::Bounds("segment has no blocks".to_string()))?;
        let last = blocks
            .last()
            .ok_or_else(|| ArchiveV2Error::Bounds("segment has no blocks".to_string()))?;
        if blocks.len() > MAX_SEGMENT_RECORDS {
            return Err(ArchiveV2Error::Bounds(
                "segment has too many blocks".to_string(),
            ));
        }
        for pair in blocks.windows(2) {
            if pair[1].header.slot != pair[0].header.slot.saturating_add(1)
                || pair[1].header.parent_hash != pair[0].hash()
            {
                return Err(ArchiveV2Error::Continuity(format!(
                    "slot {} does not follow slot {}",
                    pair[1].header.slot, pair[0].header.slot
                )));
            }
        }
        if first.header.slot > 0 && first.header.parent_hash != previous_block_hash {
            return Err(ArchiveV2Error::Continuity(format!(
                "segment start slot {} does not commit to its previous block",
                first.header.slot
            )));
        }

        let mut transactions = Vec::<ArchivedTransactionRecord>::new();
        let mut transaction_ordinals = BTreeMap::<Hash, u32>::new();
        let mut archived_blocks = Vec::with_capacity(blocks.len());
        for block in &blocks {
            let mut ordinals = Vec::with_capacity(block.transactions.len());
            for transaction in &block.transactions {
                let signature = transaction.signature();
                let encoded = serialize_legacy_bincode(transaction, "archive v2 transaction")
                    .map_err(ArchiveV2Error::Codec)?;
                let ordinal = if let Some(ordinal) = transaction_ordinals.get(&signature) {
                    let existing = &transactions[*ordinal as usize];
                    if existing.encoded_transaction != encoded {
                        return Err(ArchiveV2Error::Ordering(format!(
                            "transaction {} has conflicting bodies",
                            signature.to_hex()
                        )));
                    }
                    return Err(ArchiveV2Error::Ordering(format!(
                        "transaction {} appears more than once in the canonical segment",
                        signature.to_hex()
                    )));
                } else {
                    let ordinal = u32::try_from(transactions.len()).map_err(|_| {
                        ArchiveV2Error::Bounds("transaction ordinal overflow".to_string())
                    })?;
                    transaction_ordinals.insert(signature, ordinal);
                    transactions.push(ArchivedTransactionRecord {
                        signature,
                        first_block_slot: block.header.slot,
                        encoded_transaction: encoded,
                    });
                    ordinal
                };
                ordinals.push(ordinal);
            }
            let mut skeleton = block.clone();
            skeleton.transactions.clear();
            let encoded_skeleton = serialize_legacy_bincode(&skeleton, "archive v2 block skeleton")
                .map_err(ArchiveV2Error::Codec)?;
            archived_blocks.push(ArchivedBlockRecord {
                slot: block.header.slot,
                block_hash: block.hash(),
                encoded_skeleton,
                transaction_ordinals: ordinals,
            });
        }

        let mut frames = Vec::new();
        let transaction_records = transactions
            .iter()
            .map(|record| serialize_record(record, "archive v2 transaction record"))
            .collect::<Result<Vec<_>, _>>()?;
        frames.extend(build_frames(
            ArchiveV2FrameKind::Transactions,
            &transaction_records,
            config,
        )?);
        let block_records = archived_blocks
            .iter()
            .map(|record| serialize_record(record, "archive v2 block record"))
            .collect::<Result<Vec<_>, _>>()?;
        let block_frame_start = frames.len();
        frames.extend(build_frames(
            ArchiveV2FrameKind::Blocks,
            &block_records,
            config,
        )?);

        let mut indexes = ArchiveV2PublicIndexes {
            categories: contents.public_categories.clone(),
            ..ArchiveV2PublicIndexes::default()
        };
        let mut global_ordinal = 0u32;
        for (relative_frame, frame) in frames[..block_frame_start].iter().enumerate() {
            for local in 0..frame.record_count {
                let transaction = &transactions[global_ordinal as usize];
                indexes.transactions_by_signature.insert(
                    transaction.signature,
                    (
                        relative_frame as u32,
                        local,
                        transaction.first_block_slot,
                        0,
                    ),
                );
                global_ordinal = global_ordinal.saturating_add(1);
            }
        }
        let mut block_ordinal = 0u32;
        for (relative_frame, frame) in frames[block_frame_start..].iter().enumerate() {
            for local in 0..frame.record_count {
                let block = &archived_blocks[block_ordinal as usize];
                indexes.blocks_by_slot.insert(
                    block.slot,
                    ((block_frame_start + relative_frame) as u32, local),
                );
                for (ordinal_in_block, transaction_ordinal) in
                    block.transaction_ordinals.iter().enumerate()
                {
                    let signature = transactions[*transaction_ordinal as usize].signature;
                    if let Some(location) = indexes.transactions_by_signature.get_mut(&signature) {
                        location.2 = block.slot;
                        location.3 = ordinal_in_block as u32;
                    }
                }
                let _ = local;
                block_ordinal = block_ordinal.saturating_add(1);
            }
        }
        indexes.validate(first.header.slot, last.header.slot)?;
        let index_record = serialize_raw_record(
            &encode_compact_indexes(&indexes)?,
            "archive v2 compact public indexes",
        )?;
        frames.extend(build_frames(
            ArchiveV2FrameKind::PublicIndexes,
            &[index_record],
            config,
        )?);

        let (segment_bytes, descriptors, content_root) = encode_segment_file(
            &identity,
            first.header.slot,
            last.header.slot,
            previous_segment_hash,
            previous_block_hash,
            config,
            &frames,
        )?;
        let segment_object_hash = Hash::hash(&segment_bytes);
        let logical_categories = logical_public_categories(&blocks, &indexes.categories)?;
        let public_index_rows = logical_categories
            .values()
            .map(|rows| rows.len() as u64)
            .sum();
        let category_commitments = category_commitments(&logical_categories);
        let manifest = ArchiveV2Manifest {
            format_version: ARCHIVE_V2_FORMAT_VERSION,
            identity,
            start_slot: first.header.slot,
            end_slot: last.header.slot,
            previous_segment_hash,
            previous_block_hash,
            first_block_hash: first.hash(),
            last_block_hash: last.hash(),
            segment_object_hash,
            segment_content_root: content_root,
            index_schema_version: 1,
            zstd_level: config.zstd_level,
            target_frame_bytes: config.target_frame_bytes,
            max_frame_bytes: config.max_frame_bytes,
            dictionary_bytes: config.dictionary.len() as u32,
            dictionary_hash: config.dictionary_hash(),
            block_count: blocks.len() as u64,
            transaction_count: transactions.len() as u64,
            transaction_filter: ArchiveV2TransactionFilter::build(
                indexes.transactions_by_signature.keys(),
                transactions.len() as u64,
            )?,
            public_index_rows,
            category_commitments,
            frames: descriptors,
        };
        manifest.validate()?;
        Ok((segment_bytes, manifest))
    }

    pub fn decode(
        segment_bytes: &[u8],
        manifest: &ArchiveV2Manifest,
        expected_identity: &ArchiveV2Identity,
    ) -> Result<ArchiveV2DecodedSegment, ArchiveV2Error> {
        manifest.validate()?;
        expected_identity.validate()?;
        if manifest.identity.network_id != expected_identity.network_id {
            return Err(ArchiveV2Error::WrongNetwork {
                expected: expected_identity.network_id.clone(),
                actual: manifest.identity.network_id.clone(),
            });
        }
        if manifest.identity.genesis_hash != expected_identity.genesis_hash {
            return Err(ArchiveV2Error::WrongGenesis);
        }
        if Hash::hash(segment_bytes) != manifest.segment_object_hash {
            return Err(ArchiveV2Error::WrongObjectHash);
        }
        let parsed = decode_segment_file(segment_bytes, manifest)?;
        let transaction_frames = parsed
            .iter()
            .filter(|(descriptor, _)| descriptor.kind == ArchiveV2FrameKind::Transactions);
        let mut transactions = Vec::with_capacity(manifest.transaction_count as usize);
        for (_, raw) in transaction_frames {
            for record in decode_records(raw)? {
                let archived: ArchivedTransactionRecord = deserialize_legacy_bincode_strict(
                    record,
                    MAX_RECORD_BYTES as u64,
                    "archive v2 transaction record",
                )
                .map_err(ArchiveV2Error::Codec)?;
                let transaction: Transaction = deserialize_legacy_bincode_strict(
                    &archived.encoded_transaction,
                    MAX_RECORD_BYTES as u64,
                    "archive v2 transaction",
                )
                .map_err(ArchiveV2Error::Codec)?;
                if transaction.signature() != archived.signature {
                    return Err(ArchiveV2Error::WrongRoot);
                }
                transactions.push(transaction);
            }
        }
        if transactions.len() as u64 != manifest.transaction_count {
            return Err(ArchiveV2Error::Ordering(
                "decoded transaction count does not match manifest".to_string(),
            ));
        }

        let mut blocks = Vec::with_capacity(manifest.block_count as usize);
        for (_, raw) in parsed
            .iter()
            .filter(|(descriptor, _)| descriptor.kind == ArchiveV2FrameKind::Blocks)
        {
            for record in decode_records(raw)? {
                let archived: ArchivedBlockRecord = deserialize_legacy_bincode_strict(
                    record,
                    MAX_RECORD_BYTES as u64,
                    "archive v2 block record",
                )
                .map_err(ArchiveV2Error::Codec)?;
                let mut block: Block = deserialize_legacy_bincode_strict(
                    &archived.encoded_skeleton,
                    MAX_RECORD_BYTES as u64,
                    "archive v2 block skeleton",
                )
                .map_err(ArchiveV2Error::Codec)?;
                block.transactions = archived
                    .transaction_ordinals
                    .iter()
                    .map(|ordinal| {
                        transactions.get(*ordinal as usize).cloned().ok_or_else(|| {
                            ArchiveV2Error::Ordering(format!(
                                "block slot {} references missing transaction ordinal {ordinal}",
                                archived.slot
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if block.header.slot != archived.slot || block.hash() != archived.block_hash {
                    return Err(ArchiveV2Error::WrongRoot);
                }
                blocks.push(block);
            }
        }
        validate_decoded_blocks(&blocks, manifest)?;

        let index_frames = parsed
            .iter()
            .filter(|(descriptor, _)| descriptor.kind == ArchiveV2FrameKind::PublicIndexes)
            .collect::<Vec<_>>();
        if index_frames.len() != 1 {
            return Err(ArchiveV2Error::Ordering(
                "segment must contain exactly one public index frame".to_string(),
            ));
        }
        let index_records = decode_records(&index_frames[0].1)?;
        if index_records.len() != 1 {
            return Err(ArchiveV2Error::Ordering(
                "public index frame must contain exactly one record".to_string(),
            ));
        }
        let indexes = decode_compact_indexes(index_records[0])?;
        indexes.validate(manifest.start_slot, manifest.end_slot)?;
        if ArchiveV2TransactionFilter::build(
            indexes.transactions_by_signature.keys(),
            manifest.transaction_count,
        )? != manifest.transaction_filter
        {
            return Err(ArchiveV2Error::WrongRoot);
        }
        let logical_categories = logical_public_categories(&blocks, &indexes.categories)?;
        if category_commitments(&logical_categories) != manifest.category_commitments {
            return Err(ArchiveV2Error::WrongRoot);
        }
        validate_index_frame_references(&indexes, &manifest.frames)?;
        Ok(ArchiveV2DecodedSegment {
            manifest: manifest.clone(),
            blocks,
            transactions,
            indexes,
        })
    }

    pub fn decode_block_at(
        segment_bytes: &[u8],
        manifest: &ArchiveV2Manifest,
        expected_identity: &ArchiveV2Identity,
        slot: u64,
    ) -> Result<Option<Block>, ArchiveV2Error> {
        let config = verify_seekable_object(segment_bytes, manifest, expected_identity)?;
        if slot < manifest.start_slot || slot > manifest.end_slot {
            return Ok(None);
        }
        let indexes = decode_seekable_indexes(segment_bytes, manifest, &config)?;
        validate_index_frame_references(&indexes, &manifest.frames)?;
        let Some((block_frame_index, block_frame_ordinal)) =
            indexes.blocks_by_slot.get(&slot).copied()
        else {
            return Err(ArchiveV2Error::Ordering(format!(
                "segment slot index is missing {slot}"
            )));
        };
        let block_descriptor = manifest
            .frames
            .get(block_frame_index as usize)
            .ok_or_else(|| ArchiveV2Error::Ordering("block frame index is invalid".to_string()))?;
        let block_raw = decode_seekable_frame(segment_bytes, block_descriptor, &config.dictionary)?;
        let block_record = record_at(
            &block_raw,
            block_frame_ordinal,
            block_descriptor.record_count,
        )?;
        let archived: ArchivedBlockRecord = deserialize_legacy_bincode_strict(
            block_record,
            MAX_RECORD_BYTES as u64,
            "archive v2 seekable block record",
        )
        .map_err(ArchiveV2Error::Codec)?;
        let mut block: Block = deserialize_legacy_bincode_strict(
            &archived.encoded_skeleton,
            MAX_RECORD_BYTES as u64,
            "archive v2 seekable block skeleton",
        )
        .map_err(ArchiveV2Error::Codec)?;

        let mut transaction_frames = BTreeMap::<u32, Vec<u8>>::new();
        let mut transactions = Vec::with_capacity(archived.transaction_ordinals.len());
        for ordinal in &archived.transaction_ordinals {
            let (frame_index, descriptor) =
                transaction_descriptor_for_ordinal(&manifest.frames, *ordinal as u64)?;
            if let std::collections::btree_map::Entry::Vacant(entry) =
                transaction_frames.entry(frame_index)
            {
                entry.insert(decode_seekable_frame(
                    segment_bytes,
                    descriptor,
                    &config.dictionary,
                )?);
            }
            let raw = transaction_frames
                .get(&frame_index)
                .expect("seekable transaction frame was inserted");
            let records = decode_records(raw)?;
            let local = (*ordinal as u64)
                .checked_sub(descriptor.first_ordinal)
                .ok_or_else(|| {
                    ArchiveV2Error::Ordering("transaction ordinal underflow".to_string())
                })?;
            let record = records.get(local as usize).ok_or_else(|| {
                ArchiveV2Error::Ordering("transaction frame ordinal is invalid".to_string())
            })?;
            let archived_transaction: ArchivedTransactionRecord =
                deserialize_legacy_bincode_strict(
                    record,
                    MAX_RECORD_BYTES as u64,
                    "archive v2 seekable transaction record",
                )
                .map_err(ArchiveV2Error::Codec)?;
            let transaction: Transaction = deserialize_legacy_bincode_strict(
                &archived_transaction.encoded_transaction,
                MAX_RECORD_BYTES as u64,
                "archive v2 seekable transaction",
            )
            .map_err(ArchiveV2Error::Codec)?;
            if transaction.signature() != archived_transaction.signature {
                return Err(ArchiveV2Error::WrongRoot);
            }
            transactions.push(transaction);
        }
        block.transactions = transactions;
        if block.header.slot != slot || archived.slot != slot || block.hash() != archived.block_hash
        {
            return Err(ArchiveV2Error::WrongRoot);
        }
        Ok(Some(block))
    }

    pub fn decode_transaction_at(
        segment_bytes: &[u8],
        manifest: &ArchiveV2Manifest,
        expected_identity: &ArchiveV2Identity,
        signature: &Hash,
    ) -> Result<Option<(Transaction, u64)>, ArchiveV2Error> {
        let config = verify_seekable_object(segment_bytes, manifest, expected_identity)?;
        let indexes = decode_seekable_indexes(segment_bytes, manifest, &config)?;
        validate_index_frame_references(&indexes, &manifest.frames)?;
        let Some((frame_index, frame_ordinal, slot, _)) =
            indexes.transactions_by_signature.get(signature).copied()
        else {
            return Ok(None);
        };
        let descriptor = manifest.frames.get(frame_index as usize).ok_or_else(|| {
            ArchiveV2Error::Ordering("transaction frame index is invalid".to_string())
        })?;
        let raw = decode_seekable_frame(segment_bytes, descriptor, &config.dictionary)?;
        let record = record_at(&raw, frame_ordinal, descriptor.record_count)?;
        let archived: ArchivedTransactionRecord = deserialize_legacy_bincode_strict(
            record,
            MAX_RECORD_BYTES as u64,
            "archive v2 seekable transaction record",
        )
        .map_err(ArchiveV2Error::Codec)?;
        let transaction: Transaction = deserialize_legacy_bincode_strict(
            &archived.encoded_transaction,
            MAX_RECORD_BYTES as u64,
            "archive v2 seekable transaction",
        )
        .map_err(ArchiveV2Error::Codec)?;
        if archived.signature != *signature || transaction.signature() != *signature {
            return Err(ArchiveV2Error::WrongRoot);
        }
        Ok(Some((transaction, slot)))
    }
}

pub(super) fn canonical_archive_block(mut block: Block) -> Block {
    // These fields are locally collected finality proofs, not block-hash or
    // public-history identity. Nodes can retain different valid quorum
    // subsets and rounds for the same canonical block. Historical RPC derives
    // the deterministic proof from the canonical child commit transaction, so
    // Archive V2 stores the same normalized representation used by legacy
    // public-history parity.
    block.commit_round = 0;
    block.commit_signatures.clear();
    block
}

fn logical_public_categories(
    blocks: &[Block],
    raw_categories: &BTreeMap<String, Vec<ArchiveV2PublicRow>>,
) -> Result<BTreeMap<String, Vec<ArchiveV2PublicRow>>, ArchiveV2Error> {
    let required = crate::state::PUBLIC_HISTORY_SNAPSHOT_CATEGORIES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if raw_categories
        .keys()
        .any(|category| !required.contains(category.as_str()))
    {
        return Err(ArchiveV2Error::Ordering(
            "Archive V2 source contains an unknown public-history category".to_string(),
        ));
    }
    let mut logical = crate::state::PUBLIC_HISTORY_SNAPSHOT_CATEGORIES
        .iter()
        .map(|category| {
            (
                (*category).to_string(),
                BTreeMap::<Vec<u8>, ArchiveV2PublicRow>::new(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (category, rows) in raw_categories {
        let destination = logical.get_mut(category).ok_or_else(|| {
            ArchiveV2Error::Ordering(format!("unknown public-history category {category}"))
        })?;
        for row in rows {
            if destination.insert(row.key.clone(), row.clone()).is_some() {
                return Err(ArchiveV2Error::Ordering(format!(
                    "public-history category {category} contains a duplicate key"
                )));
            }
        }
    }
    for block in blocks {
        insert_logical_category_row(
            &mut logical,
            "slots",
            ArchiveV2PublicRow {
                slot: block.header.slot,
                key: block.header.slot.to_be_bytes().to_vec(),
                value: block.hash().0.to_vec(),
            },
        )?;
        let mut block_value = vec![0xBC];
        block_value.extend_from_slice(
            &serialize_legacy_bincode(block, "Archive V2 logical block commitment")
                .map_err(ArchiveV2Error::Codec)?,
        );
        insert_logical_category_row(
            &mut logical,
            "blocks",
            ArchiveV2PublicRow {
                slot: block.header.slot,
                key: block.hash().0.to_vec(),
                value: block_value,
            },
        )?;
        for (ordinal, transaction) in block.transactions.iter().enumerate() {
            let signature = transaction.signature();
            let mut transaction_value = vec![0xBC];
            transaction_value.extend_from_slice(
                &serialize_legacy_bincode(transaction, "Archive V2 logical transaction commitment")
                    .map_err(ArchiveV2Error::Codec)?,
            );
            insert_logical_category_row(
                &mut logical,
                "transactions",
                ArchiveV2PublicRow {
                    slot: block.header.slot,
                    key: signature.0.to_vec(),
                    value: transaction_value,
                },
            )?;
            let mut slot_key = Vec::with_capacity(16);
            slot_key.extend_from_slice(&block.header.slot.to_be_bytes());
            slot_key.extend_from_slice(&(ordinal as u64).to_be_bytes());
            insert_logical_category_row(
                &mut logical,
                "tx_by_slot",
                ArchiveV2PublicRow {
                    slot: block.header.slot,
                    key: slot_key,
                    value: signature.0.to_vec(),
                },
            )?;
            insert_logical_category_row(
                &mut logical,
                "tx_to_slot",
                ArchiveV2PublicRow {
                    slot: block.header.slot,
                    key: signature.0.to_vec(),
                    value: block.header.slot.to_be_bytes().to_vec(),
                },
            )?;
        }
    }
    Ok(logical
        .into_iter()
        .map(|(category, rows)| (category, rows.into_values().collect()))
        .collect())
}

fn insert_logical_category_row(
    categories: &mut BTreeMap<String, BTreeMap<Vec<u8>, ArchiveV2PublicRow>>,
    category: &str,
    row: ArchiveV2PublicRow,
) -> Result<(), ArchiveV2Error> {
    let rows = categories.get_mut(category).ok_or_else(|| {
        ArchiveV2Error::Ordering(format!(
            "missing logical public-history category {category}"
        ))
    })?;
    if rows.insert(row.key.clone(), row).is_some() {
        return Err(ArchiveV2Error::Ordering(format!(
            "logical public-history category {category} contains a duplicate key"
        )));
    }
    Ok(())
}

fn category_commitments(
    categories: &BTreeMap<String, Vec<ArchiveV2PublicRow>>,
) -> Vec<ArchiveV2CategoryCommitment> {
    categories
        .iter()
        .map(|(category, rows)| {
            let mut hasher = Sha256::new();
            hasher.update(ARCHIVE_V2_ROOT_DOMAIN);
            hasher.update(b"public-category");
            hasher.update((category.len() as u64).to_le_bytes());
            hasher.update(category.as_bytes());
            hasher.update((rows.len() as u64).to_le_bytes());
            for row in rows {
                hasher.update(row.slot.to_le_bytes());
                hasher.update((row.key.len() as u64).to_le_bytes());
                hasher.update(&row.key);
                hasher.update((row.value.len() as u64).to_le_bytes());
                hasher.update(&row.value);
            }
            ArchiveV2CategoryCommitment {
                category: category.clone(),
                row_count: rows.len() as u64,
                digest: Hash(hasher.finalize().into()),
            }
        })
        .collect()
}

fn validate_index_frame_references(
    indexes: &ArchiveV2PublicIndexes,
    frames: &[ArchiveV2FrameDescriptor],
) -> Result<(), ArchiveV2Error> {
    for (slot, (frame_index, frame_ordinal)) in &indexes.blocks_by_slot {
        let descriptor = frames.get(*frame_index as usize).ok_or_else(|| {
            ArchiveV2Error::Ordering(format!("slot {slot} references a missing frame"))
        })?;
        if descriptor.kind != ArchiveV2FrameKind::Blocks
            || *frame_ordinal >= descriptor.record_count
        {
            return Err(ArchiveV2Error::Ordering(format!(
                "slot {slot} references an invalid block-frame ordinal"
            )));
        }
    }
    for (signature, (frame_index, frame_ordinal, _, _)) in &indexes.transactions_by_signature {
        let descriptor = frames.get(*frame_index as usize).ok_or_else(|| {
            ArchiveV2Error::Ordering(format!(
                "transaction {signature} references a missing frame"
            ))
        })?;
        if descriptor.kind != ArchiveV2FrameKind::Transactions
            || *frame_ordinal >= descriptor.record_count
        {
            return Err(ArchiveV2Error::Ordering(format!(
                "transaction {signature} references an invalid frame ordinal"
            )));
        }
    }
    Ok(())
}

fn record_at(frame: &[u8], ordinal: u32, expected_count: u32) -> Result<&[u8], ArchiveV2Error> {
    let records = decode_records(frame)?;
    if records.len() != expected_count as usize {
        return Err(ArchiveV2Error::Ordering(
            "frame record count differs from its descriptor".to_string(),
        ));
    }
    records
        .get(ordinal as usize)
        .copied()
        .ok_or_else(|| ArchiveV2Error::Ordering("frame record ordinal is invalid".to_string()))
}

fn transaction_descriptor_for_ordinal(
    frames: &[ArchiveV2FrameDescriptor],
    ordinal: u64,
) -> Result<(u32, &ArchiveV2FrameDescriptor), ArchiveV2Error> {
    frames
        .iter()
        .enumerate()
        .find(|(_, descriptor)| {
            descriptor.kind == ArchiveV2FrameKind::Transactions
                && ordinal >= descriptor.first_ordinal
                && ordinal
                    < descriptor
                        .first_ordinal
                        .saturating_add(descriptor.record_count as u64)
        })
        .map(|(index, descriptor)| (index as u32, descriptor))
        .ok_or_else(|| {
            ArchiveV2Error::Ordering(format!(
                "transaction ordinal {ordinal} has no containing frame"
            ))
        })
}

fn encode_compact_indexes(indexes: &ArchiveV2PublicIndexes) -> Result<Vec<u8>, ArchiveV2Error> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(COMPACT_INDEX_MAGIC);
    encoded.extend_from_slice(&COMPACT_INDEX_VERSION.to_le_bytes());

    push_index_varint(&mut encoded, indexes.blocks_by_slot.len() as u64);
    let mut previous_slot = 0u64;
    for (index, (slot, (frame, ordinal))) in indexes.blocks_by_slot.iter().enumerate() {
        let delta = if index == 0 {
            *slot
        } else {
            slot.checked_sub(previous_slot).ok_or_else(|| {
                ArchiveV2Error::Ordering("block slot index moves backwards".to_string())
            })?
        };
        push_index_varint(&mut encoded, delta);
        push_index_varint(&mut encoded, *frame as u64);
        push_index_varint(&mut encoded, *ordinal as u64);
        previous_slot = *slot;
    }

    push_index_varint(&mut encoded, indexes.transactions_by_signature.len() as u64);
    let mut previous_signature = Vec::new();
    for (signature, (frame, ordinal, slot, block_ordinal)) in &indexes.transactions_by_signature {
        push_index_prefixed(&mut encoded, &previous_signature, &signature.0);
        push_index_varint(&mut encoded, *frame as u64);
        push_index_varint(&mut encoded, *ordinal as u64);
        push_index_varint(&mut encoded, *slot);
        push_index_varint(&mut encoded, *block_ordinal as u64);
        previous_signature = signature.0.to_vec();
    }

    push_index_varint(&mut encoded, indexes.categories.len() as u64);
    let mut previous_category = Vec::new();
    for (category, rows) in &indexes.categories {
        push_index_prefixed(&mut encoded, &previous_category, category.as_bytes());
        push_index_varint(&mut encoded, rows.len() as u64);
        let mut previous_key = Vec::new();
        for row in rows {
            push_index_varint(&mut encoded, row.slot);
            push_index_prefixed(&mut encoded, &previous_key, &row.key);
            push_index_varint(&mut encoded, row.value.len() as u64);
            encoded.extend_from_slice(&row.value);
            previous_key.clone_from(&row.key);
        }
        previous_category = category.as_bytes().to_vec();
    }
    if encoded.len() > MAX_RECORD_BYTES {
        return Err(ArchiveV2Error::Bounds(format!(
            "compact public indexes are {} bytes",
            encoded.len()
        )));
    }
    Ok(encoded)
}

fn decode_compact_indexes(encoded: &[u8]) -> Result<ArchiveV2PublicIndexes, ArchiveV2Error> {
    let mut cursor = CompactIndexCursor {
        bytes: encoded,
        offset: 0,
    };
    if cursor.take(COMPACT_INDEX_MAGIC.len(), "compact index magic")? != COMPACT_INDEX_MAGIC {
        return Err(ArchiveV2Error::Malformed(
            "compact public index magic mismatch".to_string(),
        ));
    }
    let version = u16::from_le_bytes(
        cursor
            .take(2, "compact index version")?
            .try_into()
            .map_err(|_| ArchiveV2Error::Truncated("compact index version"))?,
    );
    if version != COMPACT_INDEX_VERSION {
        return Err(ArchiveV2Error::Malformed(format!(
            "unsupported compact public index version {version}"
        )));
    }
    let block_count = cursor.count("block index count", MAX_SEGMENT_RECORDS)?;
    let mut blocks_by_slot = BTreeMap::new();
    let mut previous_slot = 0u64;
    for index in 0..block_count {
        let delta = cursor.varint("block slot delta")?;
        let slot = if index == 0 {
            delta
        } else {
            if delta == 0 {
                return Err(ArchiveV2Error::Ordering(
                    "compact block slot delta is zero".to_string(),
                ));
            }
            previous_slot
                .checked_add(delta)
                .ok_or_else(|| ArchiveV2Error::Bounds("compact block slot overflow".to_string()))?
        };
        let frame = cursor.u32_varint("block frame index")?;
        let ordinal = cursor.u32_varint("block frame ordinal")?;
        if blocks_by_slot.insert(slot, (frame, ordinal)).is_some() {
            return Err(ArchiveV2Error::Ordering(
                "compact block slot index is duplicated".to_string(),
            ));
        }
        previous_slot = slot;
    }

    let transaction_count = cursor.count("transaction index count", MAX_SEGMENT_RECORDS)?;
    let mut transactions_by_signature = BTreeMap::new();
    let mut previous_signature = Vec::new();
    for _ in 0..transaction_count {
        let signature = cursor.prefixed(&previous_signature, 32, "transaction signature prefix")?;
        if signature.len() != 32 {
            return Err(ArchiveV2Error::Bounds(
                "compact transaction signature is not 32 bytes".to_string(),
            ));
        }
        let signature = Hash(
            signature
                .as_slice()
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated("transaction signature"))?,
        );
        if !previous_signature.is_empty() && signature.0.as_slice() <= previous_signature.as_slice()
        {
            return Err(ArchiveV2Error::Ordering(
                "compact transaction signatures are out of order".to_string(),
            ));
        }
        let location = (
            cursor.u32_varint("transaction frame index")?,
            cursor.u32_varint("transaction frame ordinal")?,
            cursor.varint("transaction block slot")?,
            cursor.u32_varint("transaction block ordinal")?,
        );
        if transactions_by_signature
            .insert(signature, location)
            .is_some()
        {
            return Err(ArchiveV2Error::Ordering(
                "compact transaction index is duplicated".to_string(),
            ));
        }
        previous_signature = signature.0.to_vec();
    }

    let category_count = cursor.count("public category count", 1024)?;
    let mut categories = BTreeMap::new();
    let mut previous_category = Vec::new();
    for _ in 0..category_count {
        let category_bytes = cursor.prefixed(&previous_category, 128, "public category prefix")?;
        let category = String::from_utf8(category_bytes.clone()).map_err(|_| {
            ArchiveV2Error::Malformed("compact public category is not UTF-8".to_string())
        })?;
        if category.is_empty() {
            return Err(ArchiveV2Error::Bounds(
                "compact public category is empty".to_string(),
            ));
        }
        if !previous_category.is_empty()
            && category_bytes.as_slice() <= previous_category.as_slice()
        {
            return Err(ArchiveV2Error::Ordering(
                "compact public categories are out of order".to_string(),
            ));
        }
        let row_count = cursor.count("public category row count", MAX_SEGMENT_RECORDS)?;
        let mut rows = Vec::with_capacity(row_count);
        let mut previous_key = Vec::new();
        for _ in 0..row_count {
            let slot = cursor.varint("public row slot")?;
            let key =
                cursor.prefixed(&previous_key, MAX_INDEX_KEY_BYTES, "public row key prefix")?;
            if !previous_key.is_empty() && key.as_slice() <= previous_key.as_slice() {
                return Err(ArchiveV2Error::Ordering(format!(
                    "compact public category {category} keys are out of order"
                )));
            }
            let value_len = cursor.count("public row value length", MAX_RECORD_BYTES)?;
            let value = cursor.take(value_len, "public row value")?.to_vec();
            rows.push(ArchiveV2PublicRow {
                slot,
                key: key.clone(),
                value,
            });
            previous_key = key;
        }
        if categories.insert(category, rows).is_some() {
            return Err(ArchiveV2Error::Ordering(
                "compact public category is duplicated".to_string(),
            ));
        }
        previous_category = category_bytes;
    }
    cursor.finish()?;
    Ok(ArchiveV2PublicIndexes {
        blocks_by_slot,
        transactions_by_signature,
        categories,
    })
}

fn push_index_varint(encoded: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        encoded.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn push_index_prefixed(encoded: &mut Vec<u8>, previous: &[u8], current: &[u8]) {
    let common = previous
        .iter()
        .zip(current)
        .take_while(|(left, right)| left == right)
        .count();
    push_index_varint(encoded, common as u64);
    push_index_varint(encoded, (current.len() - common) as u64);
    encoded.extend_from_slice(&current[common..]);
}

struct CompactIndexCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> CompactIndexCursor<'a> {
    fn take(&mut self, length: usize, context: &'static str) -> Result<&'a [u8], ArchiveV2Error> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| ArchiveV2Error::Bounds(format!("{context} offset overflow")))?;
        if end > self.bytes.len() {
            return Err(ArchiveV2Error::Truncated(context));
        }
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn varint(&mut self, context: &'static str) -> Result<u64, ArchiveV2Error> {
        let start = self.offset;
        let mut value = 0u64;
        for index in 0..10 {
            let byte = self.take(1, context)?[0];
            if index == 9 && byte > 1 {
                return Err(ArchiveV2Error::Bounds(format!(
                    "{context} varint overflows u64"
                )));
            }
            value |= ((byte & 0x7f) as u64) << (index * 7);
            if byte & 0x80 == 0 {
                let mut canonical = Vec::new();
                push_index_varint(&mut canonical, value);
                if canonical.len() != self.offset - start {
                    return Err(ArchiveV2Error::Malformed(format!(
                        "{context} varint is not canonical"
                    )));
                }
                return Ok(value);
            }
        }
        Err(ArchiveV2Error::Bounds(format!(
            "{context} varint is too long"
        )))
    }

    fn u32_varint(&mut self, context: &'static str) -> Result<u32, ArchiveV2Error> {
        u32::try_from(self.varint(context)?)
            .map_err(|_| ArchiveV2Error::Bounds(format!("{context} exceeds u32")))
    }

    fn count(&mut self, context: &'static str, maximum: usize) -> Result<usize, ArchiveV2Error> {
        let value = usize::try_from(self.varint(context)?)
            .map_err(|_| ArchiveV2Error::Bounds(format!("{context} exceeds usize")))?;
        if value > maximum {
            return Err(ArchiveV2Error::Bounds(format!(
                "{context} {value} exceeds {maximum}"
            )));
        }
        Ok(value)
    }

    fn prefixed(
        &mut self,
        previous: &[u8],
        maximum: usize,
        context: &'static str,
    ) -> Result<Vec<u8>, ArchiveV2Error> {
        let common = self.count(context, previous.len())?;
        let suffix_len = self.count(context, maximum)?;
        let total = common
            .checked_add(suffix_len)
            .ok_or_else(|| ArchiveV2Error::Bounds(format!("{context} length overflow")))?;
        if total > maximum {
            return Err(ArchiveV2Error::Bounds(format!(
                "{context} length {total} exceeds {maximum}"
            )));
        }
        let mut value = Vec::with_capacity(total);
        value.extend_from_slice(&previous[..common]);
        value.extend_from_slice(self.take(suffix_len, context)?);
        Ok(value)
    }

    fn finish(self) -> Result<(), ArchiveV2Error> {
        if self.offset != self.bytes.len() {
            return Err(ArchiveV2Error::Malformed(
                "compact public indexes contain trailing bytes".to_string(),
            ));
        }
        Ok(())
    }
}

fn serialize_record<T: Serialize>(record: &T, context: &str) -> Result<Vec<u8>, ArchiveV2Error> {
    let payload = serialize_legacy_bincode(record, context).map_err(ArchiveV2Error::Codec)?;
    serialize_raw_record(&payload, context)
}

fn serialize_raw_record(payload: &[u8], context: &str) -> Result<Vec<u8>, ArchiveV2Error> {
    if payload.len() > MAX_RECORD_BYTES {
        return Err(ArchiveV2Error::Bounds(format!(
            "{context} is {} bytes",
            payload.len()
        )));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| ArchiveV2Error::Bounds(format!("{context} length overflow")))?;
    let mut encoded = Vec::with_capacity(4 + payload.len());
    encoded.extend_from_slice(&length.to_le_bytes());
    encoded.extend_from_slice(payload);
    Ok(encoded)
}

fn decode_records(frame: &[u8]) -> Result<Vec<&[u8]>, ArchiveV2Error> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < frame.len() {
        let length_end = offset
            .checked_add(4)
            .ok_or_else(|| ArchiveV2Error::Bounds("record offset overflow".to_string()))?;
        if length_end > frame.len() {
            return Err(ArchiveV2Error::Truncated("record length"));
        }
        let length = u32::from_le_bytes(
            frame[offset..length_end]
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated("record length"))?,
        ) as usize;
        if length > MAX_RECORD_BYTES {
            return Err(ArchiveV2Error::Bounds(format!("record is {length} bytes")));
        }
        let end = length_end
            .checked_add(length)
            .ok_or_else(|| ArchiveV2Error::Bounds("record length overflow".to_string()))?;
        if end > frame.len() {
            return Err(ArchiveV2Error::Truncated("record payload"));
        }
        records.push(&frame[length_end..end]);
        if records.len() > MAX_SEGMENT_RECORDS {
            return Err(ArchiveV2Error::Bounds(
                "frame has too many records".to_string(),
            ));
        }
        offset = end;
    }
    Ok(records)
}

fn build_frames(
    kind: ArchiveV2FrameKind,
    records: &[Vec<u8>],
    config: &ArchiveV2CodecConfig,
) -> Result<Vec<FrameBuild>, ArchiveV2Error> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let mut frames = Vec::new();
    let mut raw = Vec::new();
    let mut first_ordinal = 0u64;
    let mut count = 0u32;
    for (ordinal, record) in records.iter().enumerate() {
        if record.len() > MAX_FRAME_UNCOMPRESSED_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "{kind:?} record is {} bytes",
                record.len()
            )));
        }
        if !raw.is_empty()
            && raw.len().saturating_add(record.len()) > config.target_frame_bytes as usize
        {
            frames.push(compress_frame(kind, first_ordinal, count, &raw, config)?);
            raw.clear();
            first_ordinal = ordinal as u64;
            count = 0;
        }
        raw.extend_from_slice(record);
        count = count
            .checked_add(1)
            .ok_or_else(|| ArchiveV2Error::Bounds("frame record count overflow".to_string()))?;
        if raw.len() > MAX_FRAME_UNCOMPRESSED_BYTES {
            return Err(ArchiveV2Error::Bounds(
                "uncompressed frame exceeds 128 MiB".to_string(),
            ));
        }
    }
    if !raw.is_empty() {
        frames.push(compress_frame(kind, first_ordinal, count, &raw, config)?);
    }
    Ok(frames)
}

fn frame_hash(kind: ArchiveV2FrameKind, first: u64, count: u32, raw: &[u8]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(ARCHIVE_V2_FRAME_HASH_DOMAIN);
    hasher.update([kind as u8]);
    hasher.update(first.to_le_bytes());
    hasher.update(count.to_le_bytes());
    hasher.update((raw.len() as u64).to_le_bytes());
    hasher.update(raw);
    Hash(hasher.finalize().into())
}

fn compress_frame(
    kind: ArchiveV2FrameKind,
    first_ordinal: u64,
    record_count: u32,
    raw: &[u8],
    config: &ArchiveV2CodecConfig,
) -> Result<FrameBuild, ArchiveV2Error> {
    let compressed = if config.dictionary.is_empty() {
        zstd::bulk::compress(raw, config.zstd_level)
    } else {
        let mut compressor =
            zstd::bulk::Compressor::with_dictionary(config.zstd_level, &config.dictionary)
                .map_err(|error| ArchiveV2Error::Codec(error.to_string()))?;
        compressor.compress(raw)
    }
    .map_err(|error| ArchiveV2Error::Codec(error.to_string()))?;
    if compressed.len() > config.max_frame_bytes as usize
        || compressed.len() > MAX_FRAME_COMPRESSED_BYTES
    {
        return Err(ArchiveV2Error::Bounds(format!(
            "compressed {kind:?} frame is {} bytes",
            compressed.len()
        )));
    }
    Ok(FrameBuild {
        kind,
        first_ordinal,
        record_count,
        content_hash: frame_hash(kind, first_ordinal, record_count, raw),
        raw: raw.to_vec(),
        compressed,
    })
}

fn segment_root(frames: &[FrameBuild]) -> Hash {
    let mut leaves = frames
        .iter()
        .map(|frame| frame.content_hash)
        .collect::<Vec<_>>();
    if leaves.is_empty() {
        return Hash::hash(ARCHIVE_V2_ROOT_DOMAIN);
    }
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity(leaves.len().div_ceil(2));
        for pair in leaves.chunks(2) {
            let right = pair.get(1).copied().unwrap_or(pair[0]);
            let mut hasher = Sha256::new();
            hasher.update(ARCHIVE_V2_ROOT_DOMAIN);
            hasher.update(pair[0].0);
            hasher.update(right.0);
            next.push(Hash(hasher.finalize().into()));
        }
        leaves = next;
    }
    leaves[0]
}

fn push_string(encoded: &mut Vec<u8>, value: &str) -> Result<(), ArchiveV2Error> {
    if value.len() > MAX_NETWORK_ID_BYTES {
        return Err(ArchiveV2Error::Bounds("network id is too long".to_string()));
    }
    encoded.extend_from_slice(&(value.len() as u16).to_le_bytes());
    encoded.extend_from_slice(value.as_bytes());
    Ok(())
}

fn encode_segment_file(
    identity: &ArchiveV2Identity,
    start_slot: u64,
    end_slot: u64,
    previous_segment_hash: Option<Hash>,
    previous_block_hash: Hash,
    config: &ArchiveV2CodecConfig,
    frames: &[FrameBuild],
) -> Result<(Vec<u8>, Vec<ArchiveV2FrameDescriptor>, Hash), ArchiveV2Error> {
    if frames.is_empty() || frames.len() > MAX_FRAME_COUNT {
        return Err(ArchiveV2Error::Bounds(
            "segment frame count is invalid".to_string(),
        ));
    }
    let mut encoded = Vec::new();
    encoded.extend_from_slice(ARCHIVE_V2_SEGMENT_MAGIC);
    encoded.extend_from_slice(&ARCHIVE_V2_FORMAT_VERSION.to_le_bytes());
    push_string(&mut encoded, &identity.network_id)?;
    encoded.extend_from_slice(&identity.genesis_hash.0);
    encoded.extend_from_slice(&start_slot.to_le_bytes());
    encoded.extend_from_slice(&end_slot.to_le_bytes());
    match previous_segment_hash {
        Some(hash) => {
            encoded.push(1);
            encoded.extend_from_slice(&hash.0);
        }
        None => encoded.push(0),
    }
    encoded.extend_from_slice(&previous_block_hash.0);
    encoded.extend_from_slice(&config.zstd_level.to_le_bytes());
    encoded.extend_from_slice(&config.target_frame_bytes.to_le_bytes());
    encoded.extend_from_slice(&config.max_frame_bytes.to_le_bytes());
    encoded.extend_from_slice(&(config.dictionary.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&config.dictionary);
    encoded.extend_from_slice(&(frames.len() as u32).to_le_bytes());

    let mut descriptors = Vec::with_capacity(frames.len());
    for frame in frames {
        let compressed_bytes = u32::try_from(frame.compressed.len())
            .map_err(|_| ArchiveV2Error::Bounds("compressed frame overflow".to_string()))?;
        let uncompressed_bytes = u32::try_from(frame.raw.len())
            .map_err(|_| ArchiveV2Error::Bounds("uncompressed frame overflow".to_string()))?;
        encoded.push(frame.kind as u8);
        encoded.extend_from_slice(&frame.first_ordinal.to_le_bytes());
        encoded.extend_from_slice(&frame.record_count.to_le_bytes());
        encoded.extend_from_slice(&compressed_bytes.to_le_bytes());
        encoded.extend_from_slice(&uncompressed_bytes.to_le_bytes());
        encoded.extend_from_slice(&frame.content_hash.0);
        let file_offset = encoded.len() as u64;
        encoded.extend_from_slice(&frame.compressed);
        descriptors.push(ArchiveV2FrameDescriptor {
            kind: frame.kind,
            first_ordinal: frame.first_ordinal,
            record_count: frame.record_count,
            file_offset,
            compressed_bytes,
            uncompressed_bytes,
            content_hash: frame.content_hash,
        });
    }
    let root = segment_root(frames);
    encoded.extend_from_slice(&root.0);
    Ok((encoded, descriptors, root))
}

struct SegmentCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SegmentCursor<'a> {
    fn take(&mut self, length: usize, context: &'static str) -> Result<&'a [u8], ArchiveV2Error> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| ArchiveV2Error::Bounds(format!("{context} offset overflow")))?;
        if end > self.bytes.len() {
            return Err(ArchiveV2Error::Truncated(context));
        }
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self, context: &'static str) -> Result<u8, ArchiveV2Error> {
        Ok(self.take(1, context)?[0])
    }

    fn u16(&mut self, context: &'static str) -> Result<u16, ArchiveV2Error> {
        Ok(u16::from_le_bytes(
            self.take(2, context)?
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated(context))?,
        ))
    }

    fn u32(&mut self, context: &'static str) -> Result<u32, ArchiveV2Error> {
        Ok(u32::from_le_bytes(
            self.take(4, context)?
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated(context))?,
        ))
    }

    fn i32(&mut self, context: &'static str) -> Result<i32, ArchiveV2Error> {
        Ok(i32::from_le_bytes(
            self.take(4, context)?
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated(context))?,
        ))
    }

    fn u64(&mut self, context: &'static str) -> Result<u64, ArchiveV2Error> {
        Ok(u64::from_le_bytes(
            self.take(8, context)?
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated(context))?,
        ))
    }

    fn hash(&mut self, context: &'static str) -> Result<Hash, ArchiveV2Error> {
        Ok(Hash(
            self.take(32, context)?
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated(context))?,
        ))
    }

    fn string(&mut self, context: &'static str) -> Result<String, ArchiveV2Error> {
        let length = self.u16(context)? as usize;
        if length == 0 || length > MAX_NETWORK_ID_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "{context} length is {length}"
            )));
        }
        String::from_utf8(self.take(length, context)?.to_vec())
            .map_err(|_| ArchiveV2Error::Malformed(format!("{context} is not UTF-8")))
    }
}

fn verify_seekable_object(
    bytes: &[u8],
    manifest: &ArchiveV2Manifest,
    expected_identity: &ArchiveV2Identity,
) -> Result<ArchiveV2CodecConfig, ArchiveV2Error> {
    manifest.validate()?;
    expected_identity.validate()?;
    if manifest.identity.network_id != expected_identity.network_id {
        return Err(ArchiveV2Error::WrongNetwork {
            expected: expected_identity.network_id.clone(),
            actual: manifest.identity.network_id.clone(),
        });
    }
    if manifest.identity.genesis_hash != expected_identity.genesis_hash {
        return Err(ArchiveV2Error::WrongGenesis);
    }
    if Hash::hash(bytes) != manifest.segment_object_hash {
        return Err(ArchiveV2Error::WrongObjectHash);
    }
    if bytes.len() < ARCHIVE_V2_SEGMENT_MAGIC.len() + SEGMENT_TRAILER_BYTES {
        return Err(ArchiveV2Error::Truncated("seekable segment"));
    }
    let mut cursor = SegmentCursor { bytes, offset: 0 };
    if cursor.take(ARCHIVE_V2_SEGMENT_MAGIC.len(), "segment magic")? != ARCHIVE_V2_SEGMENT_MAGIC {
        return Err(ArchiveV2Error::Malformed(
            "segment magic mismatch".to_string(),
        ));
    }
    if cursor.u16("segment version")? != ARCHIVE_V2_FORMAT_VERSION {
        return Err(ArchiveV2Error::Malformed(
            "unsupported seekable segment version".to_string(),
        ));
    }
    let network_id = cursor.string("segment network id")?;
    if network_id != manifest.identity.network_id {
        return Err(ArchiveV2Error::WrongNetwork {
            expected: manifest.identity.network_id.clone(),
            actual: network_id,
        });
    }
    if cursor.hash("segment genesis hash")? != manifest.identity.genesis_hash {
        return Err(ArchiveV2Error::WrongGenesis);
    }
    if cursor.u64("segment start slot")? != manifest.start_slot
        || cursor.u64("segment end slot")? != manifest.end_slot
    {
        return Err(ArchiveV2Error::Continuity(
            "segment slot range differs from manifest".to_string(),
        ));
    }
    let previous_segment = match cursor.u8("previous segment tag")? {
        0 => None,
        1 => Some(cursor.hash("previous segment hash")?),
        value => {
            return Err(ArchiveV2Error::Malformed(format!(
                "invalid previous segment tag {value}"
            )))
        }
    };
    if previous_segment != manifest.previous_segment_hash
        || cursor.hash("previous block hash")? != manifest.previous_block_hash
    {
        return Err(ArchiveV2Error::Continuity(
            "segment predecessor commitments differ from manifest".to_string(),
        ));
    }
    let zstd_level = cursor.i32("zstd level")?;
    let target_frame_bytes = cursor.u32("target frame bytes")?;
    let max_frame_bytes = cursor.u32("max frame bytes")?;
    let dictionary_len = cursor.u32("dictionary length")? as usize;
    if dictionary_len > MAX_DICTIONARY_BYTES {
        return Err(ArchiveV2Error::Bounds(format!(
            "dictionary is {dictionary_len} bytes"
        )));
    }
    let config = ArchiveV2CodecConfig {
        zstd_level,
        target_frame_bytes,
        max_frame_bytes,
        dictionary: cursor.take(dictionary_len, "dictionary")?.to_vec(),
    };
    config.validate()?;
    validate_manifest_codec_config(manifest, &config)?;
    if config.dictionary_hash() != manifest.dictionary_hash {
        return Err(ArchiveV2Error::WrongRoot);
    }
    if cursor.u32("frame count")? as usize != manifest.frames.len() {
        return Err(ArchiveV2Error::Ordering(
            "segment frame count differs from manifest".to_string(),
        ));
    }
    let trailer = bytes
        .get(bytes.len().saturating_sub(SEGMENT_TRAILER_BYTES)..)
        .ok_or(ArchiveV2Error::Truncated("segment content root"))?;
    if trailer != manifest.segment_content_root.0
        || descriptor_root(&manifest.frames) != manifest.segment_content_root
    {
        return Err(ArchiveV2Error::WrongRoot);
    }
    Ok(config)
}

fn descriptor_root(frames: &[ArchiveV2FrameDescriptor]) -> Hash {
    let mut leaves = frames
        .iter()
        .map(|descriptor| descriptor.content_hash)
        .collect::<Vec<_>>();
    if leaves.is_empty() {
        return Hash::hash(ARCHIVE_V2_ROOT_DOMAIN);
    }
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity(leaves.len().div_ceil(2));
        for pair in leaves.chunks(2) {
            let right = pair.get(1).copied().unwrap_or(pair[0]);
            let mut hasher = Sha256::new();
            hasher.update(ARCHIVE_V2_ROOT_DOMAIN);
            hasher.update(pair[0].0);
            hasher.update(right.0);
            next.push(Hash(hasher.finalize().into()));
        }
        leaves = next;
    }
    leaves[0]
}

fn decode_seekable_frame(
    bytes: &[u8],
    descriptor: &ArchiveV2FrameDescriptor,
    dictionary: &[u8],
) -> Result<Vec<u8>, ArchiveV2Error> {
    const FRAME_HEADER_BYTES: usize = 1 + 8 + 4 + 4 + 4 + 32;
    let offset = usize::try_from(descriptor.file_offset)
        .map_err(|_| ArchiveV2Error::Bounds("frame offset overflow".to_string()))?;
    let header_start = offset
        .checked_sub(FRAME_HEADER_BYTES)
        .ok_or_else(|| ArchiveV2Error::Ordering("frame offset precedes its header".to_string()))?;
    let mut header = SegmentCursor {
        bytes,
        offset: header_start,
    };
    let encoded = ArchiveV2FrameDescriptor {
        kind: ArchiveV2FrameKind::try_from(header.u8("frame kind")?)?,
        first_ordinal: header.u64("frame first ordinal")?,
        record_count: header.u32("frame record count")?,
        compressed_bytes: header.u32("frame compressed bytes")?,
        uncompressed_bytes: header.u32("frame uncompressed bytes")?,
        content_hash: header.hash("frame content hash")?,
        file_offset: offset as u64,
    };
    if encoded != *descriptor {
        return Err(ArchiveV2Error::WrongRoot);
    }
    let compressed_end = offset
        .checked_add(descriptor.compressed_bytes as usize)
        .ok_or_else(|| ArchiveV2Error::Bounds("compressed frame overflow".to_string()))?;
    if compressed_end > bytes.len().saturating_sub(SEGMENT_TRAILER_BYTES) {
        return Err(ArchiveV2Error::Truncated("compressed frame"));
    }
    let compressed = &bytes[offset..compressed_end];
    let raw = if dictionary.is_empty() {
        zstd::bulk::decompress(compressed, descriptor.uncompressed_bytes as usize)
    } else {
        let mut decompressor = zstd::bulk::Decompressor::with_dictionary(dictionary)
            .map_err(|error| ArchiveV2Error::Codec(error.to_string()))?;
        decompressor.decompress(compressed, descriptor.uncompressed_bytes as usize)
    }
    .map_err(|error| ArchiveV2Error::Codec(error.to_string()))?;
    if raw.len() != descriptor.uncompressed_bytes as usize
        || frame_hash(
            descriptor.kind,
            descriptor.first_ordinal,
            descriptor.record_count,
            &raw,
        ) != descriptor.content_hash
    {
        return Err(ArchiveV2Error::WrongRoot);
    }
    Ok(raw)
}

fn decode_seekable_indexes(
    bytes: &[u8],
    manifest: &ArchiveV2Manifest,
    config: &ArchiveV2CodecConfig,
) -> Result<ArchiveV2PublicIndexes, ArchiveV2Error> {
    let mut descriptors = manifest
        .frames
        .iter()
        .filter(|descriptor| descriptor.kind == ArchiveV2FrameKind::PublicIndexes);
    let descriptor = descriptors
        .next()
        .ok_or_else(|| ArchiveV2Error::Ordering("segment has no public index frame".to_string()))?;
    if descriptors.next().is_some() {
        return Err(ArchiveV2Error::Ordering(
            "segment has multiple public index frames".to_string(),
        ));
    }
    let raw = decode_seekable_frame(bytes, descriptor, &config.dictionary)?;
    let record = record_at(&raw, 0, 1)?;
    let indexes = decode_compact_indexes(record)?;
    indexes.validate(manifest.start_slot, manifest.end_slot)?;
    Ok(indexes)
}

fn decode_segment_file(
    bytes: &[u8],
    manifest: &ArchiveV2Manifest,
) -> Result<Vec<(ArchiveV2FrameDescriptor, Vec<u8>)>, ArchiveV2Error> {
    if bytes.len() < ARCHIVE_V2_SEGMENT_MAGIC.len() + SEGMENT_TRAILER_BYTES {
        return Err(ArchiveV2Error::Truncated("segment"));
    }
    let mut cursor = SegmentCursor { bytes, offset: 0 };
    if cursor.take(ARCHIVE_V2_SEGMENT_MAGIC.len(), "segment magic")? != ARCHIVE_V2_SEGMENT_MAGIC {
        return Err(ArchiveV2Error::Malformed(
            "segment magic mismatch".to_string(),
        ));
    }
    let version = cursor.u16("segment version")?;
    if version != ARCHIVE_V2_FORMAT_VERSION {
        return Err(ArchiveV2Error::Malformed(format!(
            "unsupported segment version {version}"
        )));
    }
    let network_id = cursor.string("segment network id")?;
    if network_id != manifest.identity.network_id {
        return Err(ArchiveV2Error::WrongNetwork {
            expected: manifest.identity.network_id.clone(),
            actual: network_id,
        });
    }
    if cursor.hash("segment genesis hash")? != manifest.identity.genesis_hash {
        return Err(ArchiveV2Error::WrongGenesis);
    }
    if cursor.u64("segment start slot")? != manifest.start_slot
        || cursor.u64("segment end slot")? != manifest.end_slot
    {
        return Err(ArchiveV2Error::Continuity(
            "segment slot range differs from manifest".to_string(),
        ));
    }
    let previous_segment = match cursor.u8("previous segment tag")? {
        0 => None,
        1 => Some(cursor.hash("previous segment hash")?),
        value => {
            return Err(ArchiveV2Error::Malformed(format!(
                "invalid previous segment tag {value}"
            )))
        }
    };
    if previous_segment != manifest.previous_segment_hash
        || cursor.hash("previous block hash")? != manifest.previous_block_hash
    {
        return Err(ArchiveV2Error::Continuity(
            "segment predecessor commitments differ from manifest".to_string(),
        ));
    }
    let zstd_level = cursor.i32("zstd level")?;
    let target_frame_bytes = cursor.u32("target frame bytes")?;
    let max_frame_bytes = cursor.u32("max frame bytes")?;
    let dictionary_len = cursor.u32("dictionary length")? as usize;
    if dictionary_len > MAX_DICTIONARY_BYTES {
        return Err(ArchiveV2Error::Bounds(format!(
            "dictionary is {dictionary_len} bytes"
        )));
    }
    let dictionary = cursor.take(dictionary_len, "dictionary")?.to_vec();
    let config = ArchiveV2CodecConfig {
        zstd_level,
        target_frame_bytes,
        max_frame_bytes,
        dictionary,
    };
    config.validate()?;
    validate_manifest_codec_config(manifest, &config)?;
    if config.dictionary_hash() != manifest.dictionary_hash {
        return Err(ArchiveV2Error::WrongRoot);
    }
    let frame_count = cursor.u32("frame count")? as usize;
    if frame_count == 0 || frame_count > MAX_FRAME_COUNT || frame_count != manifest.frames.len() {
        return Err(ArchiveV2Error::Bounds(format!(
            "frame count is {frame_count}"
        )));
    }

    let mut decoded = Vec::with_capacity(frame_count);
    let mut frame_builds = Vec::with_capacity(frame_count);
    for expected in &manifest.frames {
        let kind = ArchiveV2FrameKind::try_from(cursor.u8("frame kind")?)?;
        let first_ordinal = cursor.u64("frame first ordinal")?;
        let record_count = cursor.u32("frame record count")?;
        let compressed_bytes = cursor.u32("frame compressed bytes")?;
        let uncompressed_bytes = cursor.u32("frame uncompressed bytes")?;
        let content_hash = cursor.hash("frame content hash")?;
        if record_count == 0
            || compressed_bytes as usize > MAX_FRAME_COMPRESSED_BYTES
            || uncompressed_bytes as usize > MAX_FRAME_UNCOMPRESSED_BYTES
        {
            return Err(ArchiveV2Error::Bounds(
                "frame bounds are invalid".to_string(),
            ));
        }
        let file_offset = cursor.offset as u64;
        let descriptor = ArchiveV2FrameDescriptor {
            kind,
            first_ordinal,
            record_count,
            file_offset,
            compressed_bytes,
            uncompressed_bytes,
            content_hash,
        };
        if &descriptor != expected {
            return Err(ArchiveV2Error::WrongRoot);
        }
        let compressed = cursor.take(compressed_bytes as usize, "compressed frame")?;
        let raw = if config.dictionary.is_empty() {
            zstd::bulk::decompress(compressed, uncompressed_bytes as usize)
        } else {
            let mut decompressor = zstd::bulk::Decompressor::with_dictionary(&config.dictionary)
                .map_err(|error| ArchiveV2Error::Codec(error.to_string()))?;
            decompressor.decompress(compressed, uncompressed_bytes as usize)
        }
        .map_err(|error| ArchiveV2Error::Codec(error.to_string()))?;
        if raw.len() != uncompressed_bytes as usize
            || frame_hash(kind, first_ordinal, record_count, &raw) != content_hash
        {
            return Err(ArchiveV2Error::WrongRoot);
        }
        frame_builds.push(FrameBuild {
            kind,
            first_ordinal,
            record_count,
            raw: raw.clone(),
            compressed: compressed.to_vec(),
            content_hash,
        });
        decoded.push((descriptor, raw));
    }
    let stored_root = cursor.hash("segment content root")?;
    if cursor.offset != bytes.len()
        || stored_root != manifest.segment_content_root
        || segment_root(&frame_builds) != manifest.segment_content_root
    {
        return Err(ArchiveV2Error::WrongRoot);
    }
    Ok(decoded)
}

fn validate_manifest_codec_config(
    manifest: &ArchiveV2Manifest,
    config: &ArchiveV2CodecConfig,
) -> Result<(), ArchiveV2Error> {
    if manifest.zstd_level != config.zstd_level
        || manifest.target_frame_bytes != config.target_frame_bytes
        || manifest.max_frame_bytes != config.max_frame_bytes
        || manifest.dictionary_bytes as usize != config.dictionary.len()
    {
        return Err(ArchiveV2Error::WrongRoot);
    }
    Ok(())
}

fn validate_decoded_blocks(
    blocks: &[Block],
    manifest: &ArchiveV2Manifest,
) -> Result<(), ArchiveV2Error> {
    if blocks.len() as u64 != manifest.block_count {
        return Err(ArchiveV2Error::Ordering(
            "decoded block count does not match manifest".to_string(),
        ));
    }
    let first = blocks
        .first()
        .ok_or(ArchiveV2Error::Truncated("decoded blocks"))?;
    let last = blocks
        .last()
        .ok_or(ArchiveV2Error::Truncated("decoded blocks"))?;
    if first.header.slot != manifest.start_slot
        || last.header.slot != manifest.end_slot
        || first.hash() != manifest.first_block_hash
        || last.hash() != manifest.last_block_hash
        || (first.header.slot > 0 && first.header.parent_hash != manifest.previous_block_hash)
    {
        return Err(ArchiveV2Error::Continuity(
            "decoded boundary blocks differ from manifest".to_string(),
        ));
    }
    for pair in blocks.windows(2) {
        if pair[1].header.slot != pair[0].header.slot.saturating_add(1)
            || pair[1].header.parent_hash != pair[0].hash()
        {
            return Err(ArchiveV2Error::Continuity(format!(
                "decoded slot {} does not follow {}",
                pair[1].header.slot, pair[0].header.slot
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommitSignature, Instruction, Keypair, Message, Pubkey};

    fn fixture_blocks(count: u64) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut parent = Hash::default();
        for slot in 0..count {
            let marker = slot as u8;
            let transaction = Transaction::new(Message::new(
                vec![Instruction {
                    program_id: Pubkey([marker.wrapping_add(1); 32]),
                    accounts: vec![Pubkey([marker.wrapping_add(2); 32])],
                    data: vec![marker; 1024],
                }],
                Hash::hash(&slot.to_le_bytes()),
            ));
            let block = Block::new_with_timestamp(
                slot,
                parent,
                Hash::hash(b"archive-v2-state"),
                [0xA2; 32],
                vec![transaction],
                1_700_200_000 + slot,
            );
            parent = block.hash();
            blocks.push(block);
        }
        blocks
    }

    #[test]
    fn deterministic_segment_roundtrip_reconstructs_exact_objects() {
        let identity = ArchiveV2Identity {
            network_id: "archive-v2-testnet".to_string(),
            genesis_hash: Hash::hash(b"archive-v2-genesis"),
        };
        let contents = ArchiveV2SegmentContents::from_blocks(fixture_blocks(12));
        let config = ArchiveV2CodecConfig {
            target_frame_bytes: 1024 * 1024,
            ..ArchiveV2CodecConfig::default()
        };
        let first = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &contents,
            &config,
        )
        .unwrap();
        let second = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &contents,
            &config,
        )
        .unwrap();
        assert_eq!(first.0, second.0);
        assert_eq!(
            first.1.encode_canonical().unwrap(),
            second.1.encode_canonical().unwrap()
        );
        assert_eq!(
            first.1.category_commitments.len(),
            crate::state::PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.len()
        );
        assert_eq!(
            first
                .1
                .category_commitments
                .iter()
                .map(|commitment| commitment.row_count)
                .sum::<u64>(),
            first.1.public_index_rows
        );

        let decoded = ArchiveV2SegmentCodec::decode(&first.0, &first.1, &identity).unwrap();
        assert_eq!(decoded.blocks.len(), contents.blocks.len());
        for (actual, expected) in decoded.blocks.iter().zip(&contents.blocks) {
            assert_eq!(actual.hash(), expected.hash());
            assert_eq!(actual.transactions.len(), expected.transactions.len());
            for (actual_tx, expected_tx) in actual.transactions.iter().zip(&expected.transactions) {
                assert_eq!(
                    serialize_legacy_bincode(actual_tx, "actual").unwrap(),
                    serialize_legacy_bincode(expected_tx, "expected").unwrap()
                );
            }
        }
        let seekable_block =
            ArchiveV2SegmentCodec::decode_block_at(&first.0, &first.1, &identity, 7)
                .unwrap()
                .unwrap();
        assert_eq!(seekable_block.hash(), contents.blocks[7].hash());
        let signature = contents.blocks[7].transactions[0].signature();
        let (seekable_transaction, slot) =
            ArchiveV2SegmentCodec::decode_transaction_at(&first.0, &first.1, &identity, &signature)
                .unwrap()
                .unwrap();
        assert_eq!(slot, 7);
        assert_eq!(seekable_transaction.signature(), signature);
    }

    #[test]
    fn segment_bytes_ignore_node_local_commit_certificate_subsets() {
        let identity = ArchiveV2Identity {
            network_id: "archive-v2-testnet".to_string(),
            genesis_hash: Hash::hash(b"archive-v2-genesis"),
        };
        let mut left = ArchiveV2SegmentContents::from_blocks(fixture_blocks(3));
        let mut right = left.clone();
        let left_signer = Keypair::generate();
        let right_signer = Keypair::generate();
        left.blocks[1].commit_round = 2;
        left.blocks[1].commit_signatures = vec![CommitSignature {
            validator: left_signer.pubkey().0,
            signature: left_signer.sign(b"left-local-commit-proof"),
            timestamp: 10,
        }];
        right.blocks[1].commit_round = 9;
        right.blocks[1].commit_signatures = vec![CommitSignature {
            validator: right_signer.pubkey().0,
            signature: right_signer.sign(b"right-local-commit-proof"),
            timestamp: 20,
        }];
        assert_eq!(left.blocks[1].hash(), right.blocks[1].hash());

        let config = ArchiveV2CodecConfig {
            target_frame_bytes: 1024 * 1024,
            ..ArchiveV2CodecConfig::default()
        };
        let left_encoded =
            ArchiveV2SegmentCodec::encode(identity.clone(), None, Hash::default(), &left, &config)
                .unwrap();
        let right_encoded =
            ArchiveV2SegmentCodec::encode(identity.clone(), None, Hash::default(), &right, &config)
                .unwrap();
        assert_eq!(left_encoded, right_encoded);

        let decoded =
            ArchiveV2SegmentCodec::decode(&left_encoded.0, &left_encoded.1, &identity).unwrap();
        assert_eq!(decoded.blocks[1].commit_round, 0);
        assert!(decoded.blocks[1].commit_signatures.is_empty());
    }

    #[test]
    fn segment_rejects_truncation_corruption_wrong_network_and_wrong_root() {
        let identity = ArchiveV2Identity {
            network_id: "archive-v2-testnet".to_string(),
            genesis_hash: Hash::hash(b"archive-v2-genesis"),
        };
        let contents = ArchiveV2SegmentContents::from_blocks(fixture_blocks(2));
        let config = ArchiveV2CodecConfig {
            target_frame_bytes: 1024 * 1024,
            ..ArchiveV2CodecConfig::default()
        };
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &contents,
            &config,
        )
        .unwrap();

        assert!(
            ArchiveV2SegmentCodec::decode(&bytes[..bytes.len() - 1], &manifest, &identity).is_err()
        );
        let mut corrupt = bytes.clone();
        let corrupt_offset = corrupt.len() / 2;
        corrupt[corrupt_offset] ^= 0x40;
        assert!(ArchiveV2SegmentCodec::decode(&corrupt, &manifest, &identity).is_err());
        let wrong_network = ArchiveV2Identity {
            network_id: "other-testnet".to_string(),
            genesis_hash: identity.genesis_hash,
        };
        assert!(matches!(
            ArchiveV2SegmentCodec::decode(&bytes, &manifest, &wrong_network),
            Err(ArchiveV2Error::WrongNetwork { .. })
        ));
        let mut wrong_root = manifest.clone();
        wrong_root.segment_content_root = Hash::hash(b"wrong");
        assert!(ArchiveV2SegmentCodec::decode(&bytes, &wrong_root, &identity).is_err());
        let mut wrong_category = manifest.clone();
        wrong_category.category_commitments[0].digest = Hash::hash(b"wrong-category");
        assert!(matches!(
            ArchiveV2SegmentCodec::decode(&bytes, &wrong_category, &identity),
            Err(ArchiveV2Error::WrongRoot)
        ));
        let mut wrong_codec = manifest.clone();
        wrong_codec.zstd_level += 1;
        assert!(matches!(
            ArchiveV2SegmentCodec::decode(&bytes, &wrong_codec, &identity),
            Err(ArchiveV2Error::WrongRoot)
        ));
        let mut wrong_filter = manifest.clone();
        wrong_filter.transaction_filter.bits[0] ^= 1;
        assert!(matches!(
            ArchiveV2SegmentCodec::decode(&bytes, &wrong_filter, &identity),
            Err(ArchiveV2Error::WrongRoot)
        ));
    }

    #[test]
    fn manifest_encoding_rejects_trailing_and_checksum_damage() {
        let identity = ArchiveV2Identity {
            network_id: "archive-v2-testnet".to_string(),
            genesis_hash: Hash::hash(b"archive-v2-genesis"),
        };
        let contents = ArchiveV2SegmentContents::from_blocks(fixture_blocks(1));
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity,
            None,
            Hash::default(),
            &contents,
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        assert!(!bytes.is_empty());
        let canonical = manifest.encode_canonical().unwrap();
        assert_eq!(
            ArchiveV2Manifest::decode_canonical(&canonical).unwrap(),
            manifest
        );
        let mut trailing = canonical.clone();
        trailing.push(0);
        assert!(ArchiveV2Manifest::decode_canonical(&trailing).is_err());
        let mut damaged = canonical;
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        assert!(ArchiveV2Manifest::decode_canonical(&damaged).is_err());
    }

    #[test]
    fn compact_indexes_roundtrip_prefix_rows_and_reject_noncanonical_varints() {
        let mut indexes = ArchiveV2PublicIndexes::default();
        indexes.blocks_by_slot.insert(100, (2, 0));
        indexes.blocks_by_slot.insert(101, (2, 1));
        indexes.categories.insert(
            "account_snapshots".to_string(),
            vec![
                ArchiveV2PublicRow {
                    slot: 100,
                    key: [vec![7; 32], 100u64.to_be_bytes().to_vec()].concat(),
                    value: vec![9; 256],
                },
                ArchiveV2PublicRow {
                    slot: 101,
                    key: [vec![7; 32], 101u64.to_be_bytes().to_vec()].concat(),
                    value: vec![9; 256],
                },
            ],
        );
        let compact = encode_compact_indexes(&indexes).unwrap();
        assert_eq!(decode_compact_indexes(&compact).unwrap(), indexes);
        let bincode = serialize_legacy_bincode(&indexes, "test public indexes").unwrap();
        assert!(compact.len() < bincode.len());

        let mut noncanonical = compact;
        // First count follows the eight-byte magic and two-byte version.
        noncanonical.splice(10..11, [0x82, 0x00]);
        assert!(matches!(
            decode_compact_indexes(&noncanonical),
            Err(ArchiveV2Error::Malformed(_))
        ));
    }
}
