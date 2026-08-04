use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{
    codec::canonical_archive_block, ArchiveV2CodecConfig, ArchiveV2Error, ArchiveV2Identity,
    ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
};
use crate::codec::serialize_legacy_bincode;
use crate::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveV2DictionaryKind {
    None,
    RepeatedPublicKeys,
    Trained64Kib,
    Trained128Kib,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2BenchmarkCandidate {
    pub zstd_level: i32,
    pub frame_bytes: u32,
    pub dictionary: ArchiveV2DictionaryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveV2BenchmarkPlan {
    pub candidates: Vec<ArchiveV2BenchmarkCandidate>,
    pub random_lookup_samples: usize,
}

impl ArchiveV2BenchmarkPlan {
    pub fn required_matrix() -> Self {
        let mut candidates = Vec::new();
        for zstd_level in [3, 6, 9, 12, 15] {
            for frame_bytes in [1024 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024] {
                for dictionary in [
                    ArchiveV2DictionaryKind::None,
                    ArchiveV2DictionaryKind::RepeatedPublicKeys,
                    ArchiveV2DictionaryKind::Trained64Kib,
                    ArchiveV2DictionaryKind::Trained128Kib,
                ] {
                    candidates.push(ArchiveV2BenchmarkCandidate {
                        zstd_level,
                        frame_bytes,
                        dictionary,
                    });
                }
            }
        }
        Self {
            candidates,
            random_lookup_samples: 64,
        }
    }

    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.candidates.is_empty() || self.candidates.len() > 256 {
            return Err(ArchiveV2Error::Bounds(
                "benchmark candidate count must be in 1..=256".to_string(),
            ));
        }
        if self.random_lookup_samples == 0 || self.random_lookup_samples > 10_000 {
            return Err(ArchiveV2Error::Bounds(
                "benchmark lookup samples must be in 1..=10000".to_string(),
            ));
        }
        for candidate in &self.candidates {
            ArchiveV2CodecConfig {
                zstd_level: candidate.zstd_level,
                target_frame_bytes: candidate.frame_bytes,
                max_frame_bytes: 64 * 1024 * 1024,
                dictionary: Vec::new(),
            }
            .validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2BenchmarkMeasurement {
    pub candidate: Option<ArchiveV2BenchmarkCandidate>,
    pub output_hash: Option<Hash>,
    pub second_output_hash: Option<Hash>,
    pub deterministic_segment_bytes: bool,
    pub deterministic_manifest: bool,
    pub deterministic_output: bool,
    pub exact_reconstruction: bool,
    pub source_bytes: u64,
    pub source_block_bytes: u64,
    pub source_transaction_bytes: u64,
    pub source_public_index_bytes: u64,
    pub segment_bytes: u64,
    pub dictionary_bytes: u64,
    pub dictionary_hash: Option<Hash>,
    pub bytes_per_block: f64,
    pub bytes_per_unique_transaction: f64,
    pub compression_ratio: f64,
    pub build_millis: u64,
    pub second_build_millis: u64,
    pub estimated_scratch_peak_bytes: u64,
    pub sequential_decode_millis: u64,
    pub sequential_decode_mib_per_second: f64,
    pub random_get_block_p50_micros: u64,
    pub random_get_block_p95_micros: u64,
    pub random_get_transaction_p50_micros: u64,
    pub random_get_transaction_p95_micros: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2BenchmarkReport {
    pub range_label: String,
    pub start_slot: u64,
    pub end_slot: u64,
    pub block_count: u64,
    pub unique_transaction_count: u64,
    pub measurements: Vec<ArchiveV2BenchmarkMeasurement>,
}

pub fn benchmark_archive_v2_range(
    range_label: impl Into<String>,
    identity: ArchiveV2Identity,
    previous_segment_hash: Option<Hash>,
    previous_block_hash: Hash,
    contents: &ArchiveV2SegmentContents,
    plan: &ArchiveV2BenchmarkPlan,
) -> Result<ArchiveV2BenchmarkReport, ArchiveV2Error> {
    plan.validate()?;
    let first = contents
        .blocks
        .first()
        .ok_or_else(|| ArchiveV2Error::Bounds("benchmark range is empty".to_string()))?;
    let last = contents
        .blocks
        .last()
        .ok_or_else(|| ArchiveV2Error::Bounds("benchmark range is empty".to_string()))?;
    let source = benchmark_source_sizes(contents)?;
    let unique_transactions = contents
        .blocks
        .iter()
        .flat_map(|block| block.transactions.iter())
        .map(crate::Transaction::signature)
        .collect::<std::collections::BTreeSet<_>>();
    let unique_transaction_signatures = unique_transactions.iter().copied().collect::<Vec<_>>();
    let canonical_blocks = contents
        .blocks
        .iter()
        .cloned()
        .map(canonical_archive_block)
        .collect::<Vec<_>>();
    let training_samples = benchmark_training_samples(contents)?;
    let repeated_dictionary = repeated_public_key_dictionary(contents);
    let mut report = ArchiveV2BenchmarkReport {
        range_label: range_label.into(),
        start_slot: first.header.slot,
        end_slot: last.header.slot,
        block_count: contents.blocks.len() as u64,
        unique_transaction_count: unique_transactions.len() as u64,
        measurements: Vec::with_capacity(plan.candidates.len()),
    };
    for candidate in &plan.candidates {
        let mut measurement = ArchiveV2BenchmarkMeasurement {
            candidate: Some(candidate.clone()),
            source_bytes: source.total,
            source_block_bytes: source.blocks,
            source_transaction_bytes: source.transactions,
            source_public_index_bytes: source.public_indexes,
            ..ArchiveV2BenchmarkMeasurement::default()
        };
        let dictionary = match benchmark_dictionary(
            candidate.dictionary,
            &repeated_dictionary,
            &training_samples,
        ) {
            Ok(dictionary) => dictionary,
            Err(error) => {
                measurement.error = Some(error.to_string());
                report.measurements.push(measurement);
                continue;
            }
        };
        measurement.dictionary_bytes = dictionary.len() as u64;
        measurement.dictionary_hash = Some(Hash::hash(&dictionary));
        let config = ArchiveV2CodecConfig {
            zstd_level: candidate.zstd_level,
            target_frame_bytes: candidate.frame_bytes,
            max_frame_bytes: 64 * 1024 * 1024,
            dictionary,
        };
        let build_started = Instant::now();
        let first_build = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            previous_segment_hash,
            previous_block_hash,
            contents,
            &config,
        );
        measurement.build_millis = build_started.elapsed().as_millis() as u64;
        let (bytes, manifest) = match first_build {
            Ok(result) => result,
            Err(error) => {
                measurement.error = Some(error.to_string());
                report.measurements.push(measurement);
                continue;
            }
        };
        let second_started = Instant::now();
        let second_build = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            previous_segment_hash,
            previous_block_hash,
            contents,
            &config,
        );
        measurement.second_build_millis = second_started.elapsed().as_millis() as u64;
        let (second_bytes, second_manifest) = match second_build {
            Ok(result) => result,
            Err(error) => {
                measurement.error = Some(error.to_string());
                report.measurements.push(measurement);
                continue;
            }
        };
        measurement.output_hash = Some(manifest.segment_object_hash);
        measurement.second_output_hash = Some(second_manifest.segment_object_hash);
        measurement.deterministic_segment_bytes = bytes == second_bytes;
        measurement.deterministic_manifest = manifest == second_manifest;
        measurement.deterministic_output =
            measurement.deterministic_segment_bytes && measurement.deterministic_manifest;
        measurement.segment_bytes = bytes.len() as u64;
        measurement.bytes_per_block = bytes.len() as f64 / contents.blocks.len() as f64;
        measurement.bytes_per_unique_transaction = if unique_transactions.is_empty() {
            0.0
        } else {
            bytes.len() as f64 / unique_transactions.len() as f64
        };
        measurement.compression_ratio = if bytes.is_empty() {
            0.0
        } else {
            source.total as f64 / bytes.len() as f64
        };
        measurement.estimated_scratch_peak_bytes = source
            .total
            .saturating_add((bytes.len() as u64).saturating_mul(2))
            .saturating_add(config.dictionary.len() as u64);

        let decode_started = Instant::now();
        let decoded = ArchiveV2SegmentCodec::decode(&bytes, &manifest, &identity);
        measurement.sequential_decode_millis = decode_started.elapsed().as_millis() as u64;
        match decoded {
            Ok(decoded) => {
                measurement.exact_reconstruction = serialize_legacy_bincode(
                    &decoded.blocks,
                    "Archive V2 benchmark decoded blocks",
                )
                .map_err(ArchiveV2Error::Codec)?
                    == serialize_legacy_bincode(
                        &canonical_blocks,
                        "Archive V2 benchmark canonical source blocks",
                    )
                    .map_err(ArchiveV2Error::Codec)?;
            }
            Err(error) => {
                measurement.error = Some(error.to_string());
                report.measurements.push(measurement);
                continue;
            }
        }
        let decode_seconds = (measurement.sequential_decode_millis.max(1) as f64) / 1000.0;
        measurement.sequential_decode_mib_per_second =
            (source.total as f64 / (1024.0 * 1024.0)) / decode_seconds;

        let block_latencies = benchmark_block_lookups(
            &bytes,
            &manifest,
            &identity,
            &canonical_blocks,
            plan.random_lookup_samples,
        )?;
        measurement.random_get_block_p50_micros = percentile(&block_latencies, 50);
        measurement.random_get_block_p95_micros = percentile(&block_latencies, 95);
        let transaction_latencies = benchmark_transaction_lookups(
            &bytes,
            &manifest,
            &identity,
            &unique_transaction_signatures,
            plan.random_lookup_samples,
        )?;
        measurement.random_get_transaction_p50_micros = percentile(&transaction_latencies, 50);
        measurement.random_get_transaction_p95_micros = percentile(&transaction_latencies, 95);
        if !measurement.deterministic_output || !measurement.exact_reconstruction {
            measurement.error =
                Some("determinism or exact reconstruction check failed".to_string());
        }
        report.measurements.push(measurement);
    }
    Ok(report)
}

#[derive(Debug, Clone, Copy)]
struct BenchmarkSourceSizes {
    total: u64,
    blocks: u64,
    transactions: u64,
    public_indexes: u64,
}

fn benchmark_source_sizes(
    contents: &ArchiveV2SegmentContents,
) -> Result<BenchmarkSourceSizes, ArchiveV2Error> {
    let blocks = serialize_legacy_bincode(&contents.blocks, "Archive V2 benchmark blocks")
        .map_err(ArchiveV2Error::Codec)?
        .len() as u64;
    let transactions = serialize_legacy_bincode(
        &contents
            .blocks
            .iter()
            .flat_map(|block| block.transactions.iter())
            .collect::<Vec<_>>(),
        "Archive V2 benchmark transactions",
    )
    .map_err(ArchiveV2Error::Codec)?
    .len() as u64;
    let public_indexes = serialize_legacy_bincode(
        &contents.public_categories,
        "Archive V2 benchmark public indexes",
    )
    .map_err(ArchiveV2Error::Codec)?
    .len() as u64;
    Ok(BenchmarkSourceSizes {
        total: blocks
            .saturating_add(transactions)
            .saturating_add(public_indexes),
        blocks,
        transactions,
        public_indexes,
    })
}

fn benchmark_training_samples(
    contents: &ArchiveV2SegmentContents,
) -> Result<Vec<Vec<u8>>, ArchiveV2Error> {
    let mut samples = Vec::new();
    for block in &contents.blocks {
        samples.push(
            serialize_legacy_bincode(block, "Archive V2 dictionary block")
                .map_err(ArchiveV2Error::Codec)?,
        );
        for transaction in &block.transactions {
            samples.push(
                serialize_legacy_bincode(transaction, "Archive V2 dictionary transaction")
                    .map_err(ArchiveV2Error::Codec)?,
            );
        }
    }
    for rows in contents.public_categories.values() {
        for row in rows {
            samples.push(
                serialize_legacy_bincode(row, "Archive V2 dictionary public row")
                    .map_err(ArchiveV2Error::Codec)?,
            );
        }
    }
    Ok(samples)
}

fn repeated_public_key_dictionary(contents: &ArchiveV2SegmentContents) -> Vec<u8> {
    let mut keys = Vec::new();
    for block in &contents.blocks {
        keys.extend_from_slice(&block.header.validator);
        for signature in &block.commit_signatures {
            keys.extend_from_slice(&signature.validator);
        }
        for transaction in &block.transactions {
            for instruction in &transaction.message.instructions {
                for account in &instruction.accounts {
                    keys.extend_from_slice(&account.0);
                }
            }
        }
    }
    if keys.len() > 128 * 1024 {
        keys.truncate(128 * 1024);
    }
    keys
}

fn benchmark_dictionary(
    kind: ArchiveV2DictionaryKind,
    repeated_public_keys: &[u8],
    samples: &[Vec<u8>],
) -> Result<Vec<u8>, ArchiveV2Error> {
    match kind {
        ArchiveV2DictionaryKind::None => Ok(Vec::new()),
        ArchiveV2DictionaryKind::RepeatedPublicKeys => Ok(repeated_public_keys.to_vec()),
        ArchiveV2DictionaryKind::Trained64Kib | ArchiveV2DictionaryKind::Trained128Kib => {
            let target = if kind == ArchiveV2DictionaryKind::Trained64Kib {
                64 * 1024
            } else {
                128 * 1024
            };
            let references = samples.iter().map(Vec::as_slice).collect::<Vec<_>>();
            zstd::dict::from_samples(&references, target).map_err(|error| {
                ArchiveV2Error::Codec(format!(
                    "failed training {target}-byte deterministic dictionary: {error}"
                ))
            })
        }
    }
}

fn benchmark_block_lookups(
    bytes: &[u8],
    manifest: &super::ArchiveV2Manifest,
    identity: &ArchiveV2Identity,
    blocks: &[crate::Block],
    samples: usize,
) -> Result<Vec<u64>, ArchiveV2Error> {
    let count = samples.min(blocks.len());
    let mut latencies = Vec::with_capacity(count);
    for sample in 0..count {
        let index = sample.saturating_mul(blocks.len()) / count.max(1);
        let slot = blocks[index].header.slot;
        let started = Instant::now();
        let block = ArchiveV2SegmentCodec::decode_block_at(bytes, manifest, identity, slot)?;
        latencies.push(started.elapsed().as_micros() as u64);
        let Some(block) = block else {
            return Err(ArchiveV2Error::WrongRoot);
        };
        if serialize_legacy_bincode(&block, "Archive V2 benchmark lookup block")
            .map_err(ArchiveV2Error::Codec)?
            != serialize_legacy_bincode(&blocks[index], "Archive V2 benchmark expected block")
                .map_err(ArchiveV2Error::Codec)?
        {
            return Err(ArchiveV2Error::WrongRoot);
        }
    }
    Ok(latencies)
}

fn benchmark_transaction_lookups(
    bytes: &[u8],
    manifest: &super::ArchiveV2Manifest,
    identity: &ArchiveV2Identity,
    signatures: &[Hash],
    samples: usize,
) -> Result<Vec<u64>, ArchiveV2Error> {
    let count = samples.min(signatures.len());
    let mut latencies = Vec::with_capacity(count);
    for sample in 0..count {
        let index = sample.saturating_mul(signatures.len()) / count.max(1);
        let started = Instant::now();
        if ArchiveV2SegmentCodec::decode_transaction_at(
            bytes,
            manifest,
            identity,
            &signatures[index],
        )?
        .is_none()
        {
            return Err(ArchiveV2Error::WrongRoot);
        }
        latencies.push(started.elapsed().as_micros() as u64);
    }
    Ok(latencies)
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (sorted.len() - 1).saturating_mul(percentile) / 100;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, CommitSignature, PqPublicKey, PqSignature};

    #[test]
    fn benchmark_records_determinism_reconstruction_and_lookup_metrics() {
        let identity = ArchiveV2Identity {
            network_id: "benchmark-testnet".to_string(),
            genesis_hash: Hash::hash(b"benchmark-genesis"),
        };
        let mut block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"benchmark-state"),
            [5; 32],
            Vec::new(),
            1,
        );
        block.commit_round = 7;
        block.commit_signatures.push(CommitSignature {
            validator: [6; 32],
            signature: PqSignature {
                scheme_version: 1,
                public_key: PqPublicKey {
                    scheme_version: 1,
                    bytes: vec![7; 32],
                },
                sig: vec![8; 64],
            },
            timestamp: 2,
        });
        let contents = ArchiveV2SegmentContents::from_blocks(vec![block]);
        let report = benchmark_archive_v2_range(
            "sparse",
            identity,
            None,
            Hash::default(),
            &contents,
            &ArchiveV2BenchmarkPlan {
                candidates: vec![
                    ArchiveV2BenchmarkCandidate {
                        zstd_level: 3,
                        frame_bytes: 1024 * 1024,
                        dictionary: ArchiveV2DictionaryKind::None,
                    },
                    ArchiveV2BenchmarkCandidate {
                        zstd_level: 6,
                        frame_bytes: 1024 * 1024,
                        dictionary: ArchiveV2DictionaryKind::RepeatedPublicKeys,
                    },
                ],
                random_lookup_samples: 1,
            },
        )
        .unwrap();
        assert_eq!(report.measurements.len(), 2);
        assert!(report.measurements.iter().all(|measurement| {
            measurement.error.is_none()
                && measurement.deterministic_output
                && measurement.exact_reconstruction
                && measurement.output_hash.is_some()
        }));
    }

    #[test]
    fn required_benchmark_matrix_covers_all_requested_parameters() {
        let plan = ArchiveV2BenchmarkPlan::required_matrix();
        assert_eq!(plan.candidates.len(), 60);
        for level in [3, 6, 9, 12, 15] {
            for frame in [1024 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024] {
                assert_eq!(
                    plan.candidates
                        .iter()
                        .filter(|candidate| {
                            candidate.zstd_level == level && candidate.frame_bytes == frame
                        })
                        .count(),
                    4
                );
            }
        }
    }
}
