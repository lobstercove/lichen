use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    format::{
        ArchiveV2Error, ArchiveV2Identity, ArchiveV2Manifest, ARCHIVE_V2_CATALOG_MAGIC,
        ARCHIVE_V2_FORMAT_VERSION, ARCHIVE_V2_ROOT_DOMAIN,
    },
    ArchiveV2SegmentCodec,
};
use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};
use crate::Hash;

const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;
const MAX_CATALOG_ENTRIES: usize = 1_000_000;
const MAX_CATALOG_SUPERSESSIONS: usize = 100_000;
static CATALOG_TEMPORARY_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CatalogEntry {
    pub manifest_hash: Hash,
    pub manifest: ArchiveV2Manifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CatalogSupersession {
    pub sequence: u64,
    pub supersedes_object_hash: Hash,
    pub manifest_hash: Hash,
    pub manifest: ArchiveV2Manifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2Catalog {
    pub format_version: u16,
    pub identity: ArchiveV2Identity,
    pub entries: Vec<ArchiveV2CatalogEntry>,
    pub supersessions: Vec<ArchiveV2CatalogSupersession>,
    pub catalog_root: Hash,
}

impl ArchiveV2Catalog {
    pub fn empty(identity: ArchiveV2Identity) -> Result<Self, ArchiveV2Error> {
        identity.validate()?;
        let mut catalog = Self {
            format_version: ARCHIVE_V2_FORMAT_VERSION,
            identity,
            entries: Vec::new(),
            supersessions: Vec::new(),
            catalog_root: Hash::default(),
        };
        catalog.catalog_root = catalog.compute_root()?;
        Ok(catalog)
    }

    pub fn append(&mut self, manifest: ArchiveV2Manifest) -> Result<(), ArchiveV2Error> {
        manifest.validate()?;
        self.validate()?;
        if manifest.identity != self.identity {
            if manifest.identity.network_id != self.identity.network_id {
                return Err(ArchiveV2Error::WrongNetwork {
                    expected: self.identity.network_id.clone(),
                    actual: manifest.identity.network_id,
                });
            }
            return Err(ArchiveV2Error::WrongGenesis);
        }
        if let Some(previous) = self.entries.last() {
            let valid_predecessor = manifest.previous_segment_hash.is_some_and(|hash| {
                hash == previous.manifest.segment_object_hash
                    || self.supersessions.iter().any(|supersession| {
                        supersession.manifest.start_slot == previous.manifest.start_slot
                            && supersession.manifest.end_slot == previous.manifest.end_slot
                            && supersession.manifest.segment_object_hash == hash
                    })
            });
            if manifest.start_slot != previous.manifest.end_slot.saturating_add(1)
                || !valid_predecessor
                || manifest.previous_block_hash != previous.manifest.last_block_hash
            {
                return Err(ArchiveV2Error::Continuity(format!(
                    "segment {}..{} does not extend catalog tip {}",
                    manifest.start_slot, manifest.end_slot, previous.manifest.end_slot
                )));
            }
        } else if manifest.previous_segment_hash.is_some() {
            return Err(ArchiveV2Error::Continuity(
                "first catalog segment has an unknown predecessor".to_string(),
            ));
        }
        let encoded = manifest.encode_canonical()?;
        self.entries.push(ArchiveV2CatalogEntry {
            manifest_hash: Hash::hash(&encoded),
            manifest,
        });
        if self.entries.len() > MAX_CATALOG_ENTRIES {
            return Err(ArchiveV2Error::Bounds(
                "catalog has too many entries".to_string(),
            ));
        }
        self.catalog_root = self.compute_root()?;
        self.validate()
    }

    pub fn supersede(&mut self, manifest: ArchiveV2Manifest) -> Result<(), ArchiveV2Error> {
        manifest.validate()?;
        self.validate()?;
        if manifest.identity != self.identity {
            return Err(
                if manifest.identity.network_id != self.identity.network_id {
                    ArchiveV2Error::WrongNetwork {
                        expected: self.identity.network_id.clone(),
                        actual: manifest.identity.network_id,
                    }
                } else {
                    ArchiveV2Error::WrongGenesis
                },
            );
        }
        let supersedes = self
            .manifest_for_range(manifest.start_slot, manifest.end_slot)
            .ok_or_else(|| {
                ArchiveV2Error::Continuity(format!(
                    "no active catalog segment covers {}..{}",
                    manifest.start_slot, manifest.end_slot
                ))
            })?
            .clone();
        if manifest.segment_object_hash == supersedes.segment_object_hash {
            return Err(ArchiveV2Error::Ordering(
                "supersession object is already active".to_string(),
            ));
        }
        if manifest.previous_segment_hash != supersedes.previous_segment_hash
            || manifest.previous_block_hash != supersedes.previous_block_hash
            || manifest.first_block_hash != supersedes.first_block_hash
            || manifest.last_block_hash != supersedes.last_block_hash
            || manifest.block_count != supersedes.block_count
            || manifest.transaction_count != supersedes.transaction_count
            || manifest.public_index_rows != supersedes.public_index_rows
        {
            return Err(ArchiveV2Error::Continuity(
                "supersession changes the segment's canonical history commitments".to_string(),
            ));
        }
        let encoded = manifest.encode_canonical()?;
        self.supersessions.push(ArchiveV2CatalogSupersession {
            sequence: self.supersessions.len() as u64,
            supersedes_object_hash: supersedes.segment_object_hash,
            manifest_hash: Hash::hash(&encoded),
            manifest,
        });
        if self.supersessions.len() > MAX_CATALOG_SUPERSESSIONS {
            return Err(ArchiveV2Error::Bounds(
                "catalog has too many supersessions".to_string(),
            ));
        }
        self.catalog_root = self.compute_root()?;
        self.validate()
    }

    pub fn active_manifest(
        &self,
        base_object_hash: &Hash,
    ) -> Result<&ArchiveV2Manifest, ArchiveV2Error> {
        let mut manifest = self
            .entries
            .iter()
            .find(|entry| entry.manifest.segment_object_hash == *base_object_hash)
            .map(|entry| &entry.manifest)
            .or_else(|| {
                self.supersessions
                    .iter()
                    .find(|entry| entry.manifest.segment_object_hash == *base_object_hash)
                    .map(|entry| &entry.manifest)
            })
            .ok_or_else(|| {
                ArchiveV2Error::Continuity(format!(
                    "catalog has no segment object {base_object_hash}"
                ))
            })?;
        for _ in 0..=self.supersessions.len() {
            let Some(next) = self
                .supersessions
                .iter()
                .find(|entry| entry.supersedes_object_hash == manifest.segment_object_hash)
            else {
                return Ok(manifest);
            };
            manifest = &next.manifest;
        }
        Err(ArchiveV2Error::Continuity(
            "catalog supersession cycle detected".to_string(),
        ))
    }

    pub fn manifest_for_range(&self, start_slot: u64, end_slot: u64) -> Option<&ArchiveV2Manifest> {
        let base = self.entries.iter().find(|entry| {
            entry.manifest.start_slot == start_slot && entry.manifest.end_slot == end_slot
        })?;
        self.active_manifest(&base.manifest.segment_object_hash)
            .ok()
    }

    pub fn manifest_by_object_hash(&self, object_hash: &Hash) -> Option<&ArchiveV2Manifest> {
        self.entries
            .iter()
            .map(|entry| &entry.manifest)
            .chain(self.supersessions.iter().map(|entry| &entry.manifest))
            .find(|manifest| manifest.segment_object_hash == *object_hash)
    }

    pub fn recover_from_manifests(
        identity: ArchiveV2Identity,
        mut manifests: Vec<ArchiveV2Manifest>,
    ) -> Result<Self, ArchiveV2Error> {
        manifests.sort_by(|left, right| {
            left.start_slot
                .cmp(&right.start_slot)
                .then_with(|| left.end_slot.cmp(&right.end_slot))
                .then_with(|| left.segment_object_hash.cmp(&right.segment_object_hash))
        });
        if manifests
            .windows(2)
            .any(|pair| pair[0].start_slot == pair[1].start_slot)
        {
            return Err(ArchiveV2Error::Ordering(
                "catalog recovery contains duplicate or superseding ranges; import the original catalog to preserve supersession order"
                    .to_string(),
            ));
        }
        let mut recovered = Self::empty(identity)?;
        for manifest in manifests {
            recovered.append(manifest)?;
        }
        Ok(recovered)
    }

    /// Reconstruct a non-superseded catalog from immutable promoted manifests
    /// and their exact content-addressed objects. Duplicate logical ranges are
    /// rejected because supersession ordering cannot be inferred safely after
    /// catalog loss.
    pub fn recover_from_directory(
        root: &Path,
        identity: ArchiveV2Identity,
    ) -> Result<Self, ArchiveV2Error> {
        identity.validate()?;
        let manifest_root = root.join("manifests");
        let mut manifests = Vec::new();
        for entry in fs::read_dir(&manifest_root).map_err(|error| {
            ArchiveV2Error::Io(format!(
                "failed reading manifest directory {}: {error}",
                manifest_root.display()
            ))
        })? {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("av2m")
            {
                continue;
            }
            if manifests.len() >= MAX_CATALOG_ENTRIES {
                return Err(ArchiveV2Error::Bounds(
                    "manifest directory exceeds catalog entry limit".to_string(),
                ));
            }
            let manifest = ArchiveV2Manifest::decode_canonical(&fs::read(entry.path())?)?;
            if manifest.identity != identity {
                if manifest.identity.network_id != identity.network_id {
                    return Err(ArchiveV2Error::WrongNetwork {
                        expected: identity.network_id.clone(),
                        actual: manifest.identity.network_id,
                    });
                }
                return Err(ArchiveV2Error::WrongGenesis);
            }
            let expected_name = format!("{}.av2m", manifest.segment_object_hash.to_hex());
            if entry.file_name().to_str() != Some(expected_name.as_str()) {
                return Err(ArchiveV2Error::Ordering(format!(
                    "manifest filename {} does not match segment object hash",
                    entry.path().display()
                )));
            }
            let object_path = root
                .join("objects")
                .join(format!("{}.av2s", manifest.segment_object_hash.to_hex()));
            let object = fs::read(&object_path).map_err(|error| {
                ArchiveV2Error::Io(format!(
                    "failed reading recovered object {}: {error}",
                    object_path.display()
                ))
            })?;
            ArchiveV2SegmentCodec::decode(&object, &manifest, &identity)?;
            manifests.push(manifest);
        }
        Self::recover_from_manifests(identity, manifests)
    }

    pub fn merge_verified_extension(
        &mut self,
        incoming: &ArchiveV2Catalog,
    ) -> Result<bool, ArchiveV2Error> {
        self.validate()?;
        incoming.validate()?;
        if self.identity != incoming.identity || self.format_version != incoming.format_version {
            return Err(
                if self.identity.network_id != incoming.identity.network_id {
                    ArchiveV2Error::WrongNetwork {
                        expected: self.identity.network_id.clone(),
                        actual: incoming.identity.network_id.clone(),
                    }
                } else {
                    ArchiveV2Error::WrongGenesis
                },
            );
        }
        if self.entries.len() > incoming.entries.len()
            || self.supersessions.len() > incoming.supersessions.len()
            || self.entries != incoming.entries[..self.entries.len()]
            || self.supersessions != incoming.supersessions[..self.supersessions.len()]
        {
            return Err(ArchiveV2Error::Continuity(
                "incoming catalog is not an exact append-only extension".to_string(),
            ));
        }
        if self.catalog_root == incoming.catalog_root {
            return Ok(false);
        }
        *self = incoming.clone();
        Ok(true)
    }

    pub fn import_extension_atomic(
        path: &Path,
        encoded: &[u8],
        expected_identity: &ArchiveV2Identity,
        expected_root: Option<Hash>,
    ) -> Result<Self, ArchiveV2Error> {
        let incoming = Self::decode_canonical(encoded)?;
        if &incoming.identity != expected_identity {
            return Err(
                if incoming.identity.network_id != expected_identity.network_id {
                    ArchiveV2Error::WrongNetwork {
                        expected: expected_identity.network_id.clone(),
                        actual: incoming.identity.network_id,
                    }
                } else {
                    ArchiveV2Error::WrongGenesis
                },
            );
        }
        if expected_root.is_some_and(|root| incoming.catalog_root != root) {
            return Err(ArchiveV2Error::WrongRoot);
        }
        let mut merged = if path.exists() {
            Self::load(path)?
        } else {
            Self::empty(expected_identity.clone())?
        };
        let changed = merged.merge_verified_extension(&incoming)?;
        if changed || !path.exists() {
            merged.store_atomic(path)?;
        }
        Ok(merged)
    }

    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.format_version != ARCHIVE_V2_FORMAT_VERSION {
            return Err(ArchiveV2Error::Malformed(format!(
                "unsupported catalog version {}",
                self.format_version
            )));
        }
        self.identity.validate()?;
        if self.entries.len() > MAX_CATALOG_ENTRIES
            || self.supersessions.len() > MAX_CATALOG_SUPERSESSIONS
        {
            return Err(ArchiveV2Error::Bounds(
                "catalog has too many entries or supersessions".to_string(),
            ));
        }
        let mut versions_by_range =
            std::collections::BTreeMap::<(u64, u64), std::collections::BTreeSet<Hash>>::new();
        for (index, entry) in self.entries.iter().enumerate() {
            entry.manifest.validate()?;
            if entry.manifest.identity != self.identity {
                return Err(ArchiveV2Error::WrongGenesis);
            }
            if Hash::hash(&entry.manifest.encode_canonical()?) != entry.manifest_hash {
                return Err(ArchiveV2Error::WrongRoot);
            }
            if let Some(previous) = index
                .checked_sub(1)
                .and_then(|previous| self.entries.get(previous))
            {
                if entry.manifest.start_slot != previous.manifest.end_slot.saturating_add(1)
                    || entry.manifest.previous_block_hash != previous.manifest.last_block_hash
                {
                    return Err(ArchiveV2Error::Continuity(format!(
                        "catalog entry {index} is not continuous"
                    )));
                }
            } else if entry.manifest.previous_segment_hash.is_some() {
                return Err(ArchiveV2Error::Continuity(
                    "catalog begins with an external predecessor".to_string(),
                ));
            }
            let range = (entry.manifest.start_slot, entry.manifest.end_slot);
            if versions_by_range
                .insert(
                    range,
                    std::iter::once(entry.manifest.segment_object_hash).collect(),
                )
                .is_some()
            {
                return Err(ArchiveV2Error::Ordering(
                    "catalog contains duplicate base ranges".to_string(),
                ));
            }
        }
        let mut known_manifests = self
            .entries
            .iter()
            .map(|entry| (entry.manifest.segment_object_hash, &entry.manifest))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut superseded = std::collections::BTreeSet::new();
        for (index, entry) in self.supersessions.iter().enumerate() {
            if entry.sequence != index as u64 {
                return Err(ArchiveV2Error::Ordering(
                    "catalog supersession sequence is not canonical".to_string(),
                ));
            }
            entry.manifest.validate()?;
            if entry.manifest.identity != self.identity {
                return Err(ArchiveV2Error::WrongGenesis);
            }
            if Hash::hash(&entry.manifest.encode_canonical()?) != entry.manifest_hash {
                return Err(ArchiveV2Error::WrongRoot);
            }
            if !superseded.insert(entry.supersedes_object_hash) {
                return Err(ArchiveV2Error::Ordering(
                    "a catalog object is superseded more than once".to_string(),
                ));
            }
            let prior = known_manifests
                .get(&entry.supersedes_object_hash)
                .ok_or_else(|| {
                    ArchiveV2Error::Continuity(
                        "catalog supersession references an unknown object".to_string(),
                    )
                })?;
            if entry.manifest.segment_object_hash == entry.supersedes_object_hash
                || entry.manifest.start_slot != prior.start_slot
                || entry.manifest.end_slot != prior.end_slot
                || entry.manifest.previous_segment_hash != prior.previous_segment_hash
                || entry.manifest.previous_block_hash != prior.previous_block_hash
                || entry.manifest.first_block_hash != prior.first_block_hash
                || entry.manifest.last_block_hash != prior.last_block_hash
                || entry.manifest.block_count != prior.block_count
                || entry.manifest.transaction_count != prior.transaction_count
                || entry.manifest.public_index_rows != prior.public_index_rows
            {
                return Err(ArchiveV2Error::Continuity(
                    "catalog supersession changes canonical history commitments".to_string(),
                ));
            }
            if known_manifests
                .insert(entry.manifest.segment_object_hash, &entry.manifest)
                .is_some()
            {
                return Err(ArchiveV2Error::Ordering(
                    "catalog reuses a segment object hash".to_string(),
                ));
            }
            versions_by_range
                .get_mut(&(entry.manifest.start_slot, entry.manifest.end_slot))
                .ok_or_else(|| {
                    ArchiveV2Error::Continuity(
                        "catalog supersession range has no base segment".to_string(),
                    )
                })?
                .insert(entry.manifest.segment_object_hash);
        }
        for (index, pair) in self.entries.windows(2).enumerate() {
            let previous = &pair[0].manifest;
            let current = &pair[1].manifest;
            let valid_predecessors = versions_by_range
                .get(&(previous.start_slot, previous.end_slot))
                .ok_or_else(|| {
                    ArchiveV2Error::Continuity(
                        "catalog predecessor versions are missing".to_string(),
                    )
                })?;
            if !current
                .previous_segment_hash
                .is_some_and(|hash| valid_predecessors.contains(&hash))
            {
                return Err(ArchiveV2Error::Continuity(format!(
                    "catalog entry {} has an unknown predecessor version",
                    index + 1
                )));
            }
        }
        if self.compute_root()? != self.catalog_root {
            return Err(ArchiveV2Error::WrongRoot);
        }
        Ok(())
    }

    pub fn compute_root(&self) -> Result<Hash, ArchiveV2Error> {
        let mut hasher = Sha256::new();
        hasher.update(ARCHIVE_V2_ROOT_DOMAIN);
        hasher.update(b"catalog");
        hasher.update(self.format_version.to_le_bytes());
        hasher.update((self.identity.network_id.len() as u64).to_le_bytes());
        hasher.update(self.identity.network_id.as_bytes());
        hasher.update(self.identity.genesis_hash.0);
        hasher.update((self.entries.len() as u64).to_le_bytes());
        for entry in &self.entries {
            hasher.update(entry.manifest_hash.0);
            hasher.update(entry.manifest.segment_object_hash.0);
            hasher.update(entry.manifest.segment_content_root.0);
            hasher.update(entry.manifest.start_slot.to_le_bytes());
            hasher.update(entry.manifest.end_slot.to_le_bytes());
        }
        hasher.update((self.supersessions.len() as u64).to_le_bytes());
        for entry in &self.supersessions {
            hasher.update(entry.sequence.to_le_bytes());
            hasher.update(entry.supersedes_object_hash.0);
            hasher.update(entry.manifest_hash.0);
            hasher.update(entry.manifest.segment_object_hash.0);
            hasher.update(entry.manifest.segment_content_root.0);
        }
        Ok(Hash(hasher.finalize().into()))
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, ArchiveV2Error> {
        self.validate()?;
        let payload =
            serialize_legacy_bincode(self, "archive v2 catalog").map_err(ArchiveV2Error::Codec)?;
        if payload.len() > MAX_CATALOG_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "catalog is {} bytes",
                payload.len()
            )));
        }
        let mut encoded =
            Vec::with_capacity(ARCHIVE_V2_CATALOG_MAGIC.len() + 4 + payload.len() + 32);
        encoded.extend_from_slice(ARCHIVE_V2_CATALOG_MAGIC);
        encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&payload);
        encoded.extend_from_slice(&Hash::hash(&payload).0);
        Ok(encoded)
    }

    pub fn decode_canonical(encoded: &[u8]) -> Result<Self, ArchiveV2Error> {
        let minimum = ARCHIVE_V2_CATALOG_MAGIC.len() + 4 + 32;
        if encoded.len() < minimum || !encoded.starts_with(ARCHIVE_V2_CATALOG_MAGIC) {
            return Err(ArchiveV2Error::Truncated("catalog"));
        }
        let offset = ARCHIVE_V2_CATALOG_MAGIC.len();
        let payload_len = u32::from_le_bytes(
            encoded[offset..offset + 4]
                .try_into()
                .map_err(|_| ArchiveV2Error::Truncated("catalog length"))?,
        ) as usize;
        if payload_len > MAX_CATALOG_BYTES {
            return Err(ArchiveV2Error::Bounds(format!(
                "catalog is {payload_len} bytes"
            )));
        }
        let payload_start = offset + 4;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or_else(|| ArchiveV2Error::Bounds("catalog length overflow".to_string()))?;
        if payload_end.checked_add(32) != Some(encoded.len()) {
            return Err(ArchiveV2Error::Truncated("catalog payload"));
        }
        let payload = &encoded[payload_start..payload_end];
        if Hash::hash(payload).0 != encoded[payload_end..] {
            return Err(ArchiveV2Error::WrongRoot);
        }
        let catalog = deserialize_legacy_bincode_strict(
            payload,
            MAX_CATALOG_BYTES as u64,
            "archive v2 catalog",
        )
        .map_err(ArchiveV2Error::Codec)?;
        ArchiveV2Catalog::validate(&catalog)?;
        Ok(catalog)
    }

    pub fn load(path: &Path) -> Result<Self, ArchiveV2Error> {
        let bytes = fs::read(path).map_err(|error| {
            ArchiveV2Error::Io(format!("failed reading {}: {error}", path.display()))
        })?;
        Self::decode_canonical(&bytes)
    }

    pub fn store_atomic(&self, path: &Path) -> Result<(), ArchiveV2Error> {
        let encoded = self.encode_canonical()?;
        let parent = path.parent().ok_or_else(|| {
            ArchiveV2Error::Io("catalog path has no parent directory".to_string())
        })?;
        fs::create_dir_all(parent)?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| ArchiveV2Error::Io("catalog filename is invalid".to_string()))?;
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            CATALOG_TEMPORARY_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| {
                ArchiveV2Error::Io(format!(
                    "failed creating catalog staging file {}: {error}",
                    temporary.display()
                ))
            })?;
        let result = (|| {
            file.write_all(&encoded)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            OpenOptions::new()
                .read(true)
                .open(parent)?
                .sync_all()
                .map_err(ArchiveV2Error::from)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::archive_v2::{
        ArchiveV2CodecConfig, ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
    };
    use crate::Block;

    fn manifest(
        identity: &ArchiveV2Identity,
        start: u64,
        parent: Hash,
        previous: Option<Hash>,
    ) -> ArchiveV2Manifest {
        let block = Block::new_with_timestamp(
            start,
            parent,
            Hash::hash(b"catalog-state"),
            [7; 32],
            Vec::new(),
            start + 1,
        );
        let (_, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            previous,
            parent,
            &ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        manifest
    }

    #[test]
    fn catalog_append_roundtrip_and_atomic_storage_are_deterministic() {
        let identity = ArchiveV2Identity {
            network_id: "catalog-testnet".to_string(),
            genesis_hash: Hash::hash(b"catalog-genesis"),
        };
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        let first = manifest(&identity, 0, Hash::default(), None);
        let first_hash = first.segment_object_hash;
        let first_block = first.last_block_hash;
        catalog.append(first).unwrap();
        catalog
            .append(manifest(&identity, 1, first_block, Some(first_hash)))
            .unwrap();
        let encoded = catalog.encode_canonical().unwrap();
        assert_eq!(
            ArchiveV2Catalog::decode_canonical(&encoded).unwrap(),
            catalog
        );

        let root = tempdir().unwrap();
        let path = root.path().join("catalog.av2");
        catalog.store_atomic(&path).unwrap();
        assert_eq!(ArchiveV2Catalog::load(&path).unwrap(), catalog);
    }

    #[test]
    fn catalog_recovers_only_from_verified_promoted_manifest_object_pairs() {
        let identity = ArchiveV2Identity {
            network_id: "catalog-recovery-directory-testnet".to_string(),
            genesis_hash: Hash::hash(b"catalog-recovery-directory-genesis"),
        };
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"catalog-recovery-directory-state"),
            [12; 32],
            Vec::new(),
            1,
        );
        let (object, manifest) = ArchiveV2SegmentCodec::encode(
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
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("objects")).unwrap();
        fs::create_dir_all(root.path().join("manifests")).unwrap();
        fs::write(
            root.path()
                .join("objects")
                .join(format!("{}.av2s", manifest.segment_object_hash)),
            &object,
        )
        .unwrap();
        let manifest_path = root
            .path()
            .join("manifests")
            .join(format!("{}.av2m", manifest.segment_object_hash));
        fs::write(&manifest_path, manifest.encode_canonical().unwrap()).unwrap();

        let recovered =
            ArchiveV2Catalog::recover_from_directory(root.path(), identity.clone()).unwrap();
        assert_eq!(recovered.entries.len(), 1);
        assert_eq!(
            recovered.entries[0].manifest.segment_object_hash,
            manifest.segment_object_hash
        );

        fs::write(&manifest_path, b"corrupt").unwrap();
        assert!(ArchiveV2Catalog::recover_from_directory(root.path(), identity).is_err());
    }

    #[test]
    fn catalog_rejects_gap_and_wrong_predecessor() {
        let identity = ArchiveV2Identity {
            network_id: "catalog-testnet".to_string(),
            genesis_hash: Hash::hash(b"catalog-genesis"),
        };
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        let first = manifest(&identity, 0, Hash::default(), None);
        let first_block = first.last_block_hash;
        catalog.append(first).unwrap();
        let gap = manifest(&identity, 2, first_block, Some(Hash::hash(b"wrong")));
        assert!(matches!(
            catalog.append(gap),
            Err(ArchiveV2Error::Continuity(_))
        ));
    }

    #[test]
    fn catalog_supersession_is_append_only_and_accepts_old_predecessor_versions() {
        let identity = ArchiveV2Identity {
            network_id: "catalog-supersession-testnet".to_string(),
            genesis_hash: Hash::hash(b"catalog-supersession-genesis"),
        };
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        let first = manifest(&identity, 0, Hash::default(), None);
        let original_hash = first.segment_object_hash;
        let first_block_hash = first.last_block_hash;
        catalog.append(first).unwrap();

        let replacement_block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"catalog-state"),
            [7; 32],
            Vec::new(),
            1,
        );
        let (_, replacement) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![replacement_block]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                dictionary: b"catalog-supersession-dictionary".to_vec(),
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        assert_ne!(replacement.segment_object_hash, original_hash);
        let replacement_hash = replacement.segment_object_hash;
        catalog.supersede(replacement).unwrap();
        assert_eq!(
            catalog
                .active_manifest(&original_hash)
                .unwrap()
                .segment_object_hash,
            replacement_hash
        );

        // A later segment may commit to any catalog-proven predecessor version.
        // This preserves old append-only entries across a later supersession.
        catalog
            .append(manifest(
                &identity,
                1,
                first_block_hash,
                Some(original_hash),
            ))
            .unwrap();
        let encoded = catalog.encode_canonical().unwrap();
        assert_eq!(
            ArchiveV2Catalog::decode_canonical(&encoded).unwrap(),
            catalog
        );
    }

    #[test]
    fn catalog_recovery_and_import_require_an_exact_extension() {
        let identity = ArchiveV2Identity {
            network_id: "catalog-recovery-testnet".to_string(),
            genesis_hash: Hash::hash(b"catalog-recovery-genesis"),
        };
        let first = manifest(&identity, 0, Hash::default(), None);
        let second = manifest(
            &identity,
            1,
            first.last_block_hash,
            Some(first.segment_object_hash),
        );
        let recovered =
            ArchiveV2Catalog::recover_from_manifests(identity.clone(), vec![second, first])
                .unwrap();
        assert_eq!(recovered.entries.len(), 2);

        let root = tempdir().unwrap();
        let path = root.path().join("imported-catalog.av2");
        let imported = ArchiveV2Catalog::import_extension_atomic(
            &path,
            &recovered.encode_canonical().unwrap(),
            &identity,
            Some(recovered.catalog_root),
        )
        .unwrap();
        assert_eq!(imported, recovered);

        let mut divergent = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        divergent
            .append(manifest(&identity, 0, Hash::default(), None))
            .unwrap();
        divergent.supersessions.clear();
        divergent.entries[0].manifest.segment_content_root = Hash::hash(b"conflict");
        divergent.entries[0].manifest_hash =
            Hash::hash(&divergent.entries[0].manifest.encode_canonical().unwrap());
        divergent.catalog_root = divergent.compute_root().unwrap();
        assert!(ArchiveV2Catalog::import_extension_atomic(
            &path,
            &divergent.encode_canonical().unwrap(),
            &identity,
            Some(divergent.catalog_root),
        )
        .is_err());
    }
}
