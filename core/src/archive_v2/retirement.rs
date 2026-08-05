use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    ArchiveV2Error, ArchiveV2Identity, ArchiveV2Manifest, ARCHIVE_V2_CATALOG_VERSION,
    ARCHIVE_V2_FORMAT_VERSION,
};
use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};
use crate::{Hash, Keypair, PqSignature, Pubkey};

const RETIREMENT_MAGIC: &[u8] = b"LICHEN-AV2-RETIRE\0";
const RETIREMENT_FORMAT_VERSION: u16 = 3;
const RETIREMENT_DOMAIN: &[u8] = b"lichen:archive-v2:retirement:v3";
const CATEGORY_PROOF_DOMAIN: &[u8] = b"lichen:archive-v2:category-proof:v2";
const MAX_RETIREMENT_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_RETIREMENT_CATEGORIES: usize = 128;
const MAX_REPLICA_EVIDENCE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CategoryProof {
    pub category: String,
    pub row_count: u64,
    pub logical_bytes: u64,
    pub rows_root: Hash,
}

impl ArchiveV2CategoryProof {
    pub fn from_rows(
        category: impl Into<String>,
        rows: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<Self, ArchiveV2Error> {
        let category = category.into();
        if category.is_empty() || category.len() > 128 {
            return Err(ArchiveV2Error::Bounds(
                "retirement category name is invalid".to_string(),
            ));
        }
        if rows.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(ArchiveV2Error::Ordering(format!(
                "retirement category {category} rows are duplicated or out of order"
            )));
        }
        let mut hasher = Sha256::new();
        hasher.update(CATEGORY_PROOF_DOMAIN);
        hasher.update((category.len() as u64).to_le_bytes());
        hasher.update(category.as_bytes());
        hasher.update((rows.len() as u64).to_le_bytes());
        let mut logical_bytes = 0u64;
        for (key, value) in rows {
            hasher.update((key.len() as u64).to_le_bytes());
            hasher.update(key);
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value);
            logical_bytes = logical_bytes.saturating_add((key.len() + value.len()) as u64);
        }
        Ok(Self {
            category,
            row_count: rows.len() as u64,
            logical_bytes,
            rows_root: Hash(hasher.finalize().into()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2ReplicaEvidence {
    pub destination: String,
    pub failure_domain: String,
    pub segment_object_hash: Hash,
    pub verified_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2RollbackAnchor {
    pub release_tag: String,
    pub release_commit: String,
    pub artifact_sha256: Hash,
    pub detached_pq_checksum_signature_sha256: Hash,
    pub archive_format_version: u16,
    pub catalog_format_version: u16,
    pub deployed_validator_count: u16,
    pub activated_unix_seconds: u64,
}

impl ArchiveV2RollbackAnchor {
    fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.release_tag.is_empty()
            || self.release_tag.len() > 128
            || !matches!(self.release_commit.len(), 40 | 64)
            || !self
                .release_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.archive_format_version < ARCHIVE_V2_FORMAT_VERSION
            || self.catalog_format_version < ARCHIVE_V2_CATALOG_VERSION
            || self.deployed_validator_count == 0
            || self.activated_unix_seconds == 0
            || self.artifact_sha256 == Hash::default()
            || self.detached_pq_checksum_signature_sha256 == Hash::default()
        {
            return Err(ArchiveV2Error::Role(
                "dual-reader rollback anchor evidence is incomplete".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveV2RetirementPayload {
    format_version: u16,
    identity: ArchiveV2Identity,
    catalog_root: Hash,
    segment_object_hash: Hash,
    segment_content_root: Hash,
    start_slot: u64,
    end_slot: u64,
    category_proofs: Vec<ArchiveV2CategoryProof>,
    replica_evidence: Vec<ArchiveV2ReplicaEvidence>,
    required_replica_count: u16,
    required_failure_domains: u16,
    rollback_anchor: ArchiveV2RollbackAnchor,
    authorized_unix_seconds: u64,
    signer: Pubkey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2RetirementManifest {
    payload: ArchiveV2RetirementPayload,
    pub signature: PqSignature,
}

#[derive(Debug, Clone)]
pub struct ArchiveV2RetirementRequest {
    pub identity: ArchiveV2Identity,
    pub catalog_root: Hash,
    pub segment_manifest: ArchiveV2Manifest,
    pub category_proofs: Vec<ArchiveV2CategoryProof>,
    pub replica_evidence: Vec<ArchiveV2ReplicaEvidence>,
    pub required_replica_count: u16,
    pub required_failure_domains: u16,
    pub rollback_anchor: ArchiveV2RollbackAnchor,
    pub authorized_unix_seconds: u64,
}

impl ArchiveV2RetirementManifest {
    pub fn sign(
        request: ArchiveV2RetirementRequest,
        signer: &Keypair,
    ) -> Result<Self, ArchiveV2Error> {
        let payload = ArchiveV2RetirementPayload {
            format_version: RETIREMENT_FORMAT_VERSION,
            identity: request.identity,
            catalog_root: request.catalog_root,
            segment_object_hash: request.segment_manifest.segment_object_hash,
            segment_content_root: request.segment_manifest.segment_content_root,
            start_slot: request.segment_manifest.start_slot,
            end_slot: request.segment_manifest.end_slot,
            category_proofs: request.category_proofs,
            replica_evidence: request.replica_evidence,
            required_replica_count: request.required_replica_count,
            required_failure_domains: request.required_failure_domains,
            rollback_anchor: request.rollback_anchor,
            authorized_unix_seconds: request.authorized_unix_seconds,
            signer: signer.pubkey(),
        };
        let payload_bytes = encode_payload(&payload)?;
        let manifest = Self {
            signature: signer.sign(&payload_bytes),
            payload,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        let payload = &self.payload;
        if payload.format_version != RETIREMENT_FORMAT_VERSION
            || payload.end_slot < payload.start_slot
            || payload.authorized_unix_seconds == 0
            || payload.catalog_root == Hash::default()
            || payload.segment_object_hash == Hash::default()
            || payload.segment_content_root == Hash::default()
        {
            return Err(ArchiveV2Error::Malformed(
                "retirement payload bounds are invalid".to_string(),
            ));
        }
        payload.identity.validate()?;
        payload.rollback_anchor.validate()?;
        if payload.category_proofs.is_empty()
            || payload.category_proofs.len() > MAX_RETIREMENT_CATEGORIES
            || payload
                .category_proofs
                .windows(2)
                .any(|pair| pair[0].category >= pair[1].category)
        {
            return Err(ArchiveV2Error::Ordering(
                "retirement category proofs are empty, duplicated, or unordered".to_string(),
            ));
        }
        if payload.replica_evidence.len() > MAX_REPLICA_EVIDENCE
            || payload.required_replica_count == 0
            || payload.replica_evidence.len() < payload.required_replica_count as usize
        {
            return Err(ArchiveV2Error::Unavailable(
                "retirement replica evidence is below policy".to_string(),
            ));
        }
        if payload.replica_evidence.windows(2).any(|pair| {
            (
                pair[0].failure_domain.as_str(),
                pair[0].destination.as_str(),
            ) >= (
                pair[1].failure_domain.as_str(),
                pair[1].destination.as_str(),
            )
        }) {
            return Err(ArchiveV2Error::Ordering(
                "retirement replica evidence is duplicated or unordered".to_string(),
            ));
        }
        let failure_domains = payload
            .replica_evidence
            .iter()
            .map(|evidence| evidence.failure_domain.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if failure_domains.len() < payload.required_failure_domains as usize
            || payload.required_failure_domains == 0
            || payload.replica_evidence.iter().any(|evidence| {
                evidence.destination.is_empty()
                    || evidence.failure_domain.is_empty()
                    || evidence.segment_object_hash != payload.segment_object_hash
                    || evidence.verified_unix_seconds == 0
            })
        {
            return Err(ArchiveV2Error::Unavailable(
                "retirement replica failure-domain evidence is invalid".to_string(),
            ));
        }
        let payload_bytes = encode_payload(payload)?;
        if !Keypair::verify(&payload.signer, &payload_bytes, &self.signature) {
            return Err(ArchiveV2Error::WrongRoot);
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, ArchiveV2Error> {
        self.validate()?;
        let payload = serialize_legacy_bincode(self, "archive v2 retirement manifest")
            .map_err(ArchiveV2Error::Codec)?;
        if payload.len() > MAX_RETIREMENT_MANIFEST_BYTES {
            return Err(ArchiveV2Error::Bounds(
                "retirement manifest is too large".to_string(),
            ));
        }
        let mut encoded = Vec::with_capacity(RETIREMENT_MAGIC.len() + 4 + payload.len() + 32);
        encoded.extend_from_slice(RETIREMENT_MAGIC);
        encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&payload);
        encoded.extend_from_slice(&Hash::hash(&payload).0);
        Ok(encoded)
    }

    pub fn decode_canonical(encoded: &[u8]) -> Result<Self, ArchiveV2Error> {
        let minimum = RETIREMENT_MAGIC.len() + 4 + 32;
        if encoded.len() < minimum || !encoded.starts_with(RETIREMENT_MAGIC) {
            return Err(ArchiveV2Error::Truncated("retirement manifest"));
        }
        let payload_len = u32::from_le_bytes(
            encoded[RETIREMENT_MAGIC.len()..RETIREMENT_MAGIC.len() + 4]
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated("retirement manifest length"))?,
        ) as usize;
        if payload_len > MAX_RETIREMENT_MANIFEST_BYTES {
            return Err(ArchiveV2Error::Bounds(
                "retirement manifest is too large".to_string(),
            ));
        }
        let start = RETIREMENT_MAGIC.len() + 4;
        let end = start
            .checked_add(payload_len)
            .ok_or_else(|| ArchiveV2Error::Bounds("retirement length overflow".to_string()))?;
        if end.checked_add(32) != Some(encoded.len())
            || Hash::hash(&encoded[start..end]).0 != encoded[end..]
        {
            return Err(ArchiveV2Error::WrongRoot);
        }
        let manifest = deserialize_legacy_bincode_strict(
            &encoded[start..end],
            MAX_RETIREMENT_MANIFEST_BYTES as u64,
            "archive v2 retirement manifest",
        )
        .map_err(ArchiveV2Error::Codec)?;
        Self::validate(&manifest)?;
        Ok(manifest)
    }

    pub fn identity(&self) -> &ArchiveV2Identity {
        &self.payload.identity
    }

    pub fn catalog_root(&self) -> Hash {
        self.payload.catalog_root
    }

    pub fn segment_object_hash(&self) -> Hash {
        self.payload.segment_object_hash
    }

    pub fn segment_content_root(&self) -> Hash {
        self.payload.segment_content_root
    }

    pub fn slot_range(&self) -> (u64, u64) {
        (self.payload.start_slot, self.payload.end_slot)
    }

    pub fn category_proofs(&self) -> &[ArchiveV2CategoryProof] {
        &self.payload.category_proofs
    }

    pub fn signer(&self) -> Pubkey {
        self.payload.signer
    }
}

fn encode_payload(payload: &ArchiveV2RetirementPayload) -> Result<Vec<u8>, ArchiveV2Error> {
    let encoded = serialize_legacy_bincode(payload, "archive v2 retirement payload")
        .map_err(ArchiveV2Error::Codec)?;
    if encoded.len() > MAX_RETIREMENT_MANIFEST_BYTES {
        return Err(ArchiveV2Error::Bounds(
            "retirement payload is too large".to_string(),
        ));
    }
    let mut domain_bound = Vec::with_capacity(RETIREMENT_DOMAIN.len() + encoded.len());
    domain_bound.extend_from_slice(RETIREMENT_DOMAIN);
    domain_bound.extend_from_slice(&encoded);
    Ok(domain_bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_v2::{
        ArchiveV2CodecConfig, ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
    };
    use crate::Block;

    #[test]
    fn signed_retirement_manifest_rejects_tampering_and_weak_evidence() {
        let identity = ArchiveV2Identity {
            network_id: "retirement-testnet".to_string(),
            genesis_hash: Hash::hash(b"retirement-genesis"),
        };
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"retirement-state"),
            [1; 32],
            Vec::new(),
            1,
        );
        let (_, segment_manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let object_hash = segment_manifest.segment_object_hash;
        let proof = ArchiveV2CategoryProof::from_rows("blocks", &[(vec![1], vec![2])]).unwrap();
        let signer = Keypair::from_seed(&[7; 32]);
        let request = ArchiveV2RetirementRequest {
            identity,
            catalog_root: Hash::hash(b"catalog"),
            segment_manifest,
            category_proofs: vec![proof],
            replica_evidence: vec![
                ArchiveV2ReplicaEvidence {
                    destination: "provider-a".to_string(),
                    failure_domain: "region-a".to_string(),
                    segment_object_hash: object_hash,
                    verified_unix_seconds: 1,
                },
                ArchiveV2ReplicaEvidence {
                    destination: "provider-b".to_string(),
                    failure_domain: "region-b".to_string(),
                    segment_object_hash: object_hash,
                    verified_unix_seconds: 1,
                },
            ],
            required_replica_count: 2,
            required_failure_domains: 2,
            rollback_anchor: ArchiveV2RollbackAnchor {
                release_tag: "v0.6.0".to_string(),
                release_commit: "a".repeat(40),
                artifact_sha256: Hash::hash(b"artifact"),
                detached_pq_checksum_signature_sha256: Hash::hash(b"pq-signature"),
                archive_format_version: ARCHIVE_V2_FORMAT_VERSION,
                catalog_format_version: ARCHIVE_V2_CATALOG_VERSION,
                deployed_validator_count: 4,
                activated_unix_seconds: 1,
            },
            authorized_unix_seconds: 1,
        };
        let mut stale_catalog_anchor = request.clone();
        stale_catalog_anchor.rollback_anchor.catalog_format_version =
            ARCHIVE_V2_CATALOG_VERSION - 1;
        assert!(matches!(
            ArchiveV2RetirementManifest::sign(stale_catalog_anchor, &signer),
            Err(ArchiveV2Error::Role(_))
        ));
        let manifest = ArchiveV2RetirementManifest::sign(request, &signer).unwrap();
        let encoded = manifest.encode_canonical().unwrap();
        assert_eq!(
            ArchiveV2RetirementManifest::decode_canonical(&encoded).unwrap(),
            manifest
        );

        let mut tampered = manifest.clone();
        tampered.payload.end_slot = 1;
        assert!(matches!(
            tampered.validate(),
            Err(ArchiveV2Error::WrongRoot)
        ));
    }
}
