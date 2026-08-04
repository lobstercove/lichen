use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    ArchiveV2Catalog, ArchiveV2Error, ArchiveV2Identity, ArchiveV2Manifest, ArchiveV2SegmentCodec,
};
use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};
use crate::Hash;

const MIRROR_JOURNAL_MAGIC: &[u8] = b"LICHEN-AV2-MIRROR\0";
const MIRROR_JOURNAL_VERSION: u16 = 1;
const MAX_MIRROR_JOURNAL_BYTES: usize = 1024 * 1024;
static REPLICA_TEMPORARY_NONCE: AtomicU64 = AtomicU64::new(0);

pub trait ArchiveV2ReplicaTransport: Send + Sync {
    fn name(&self) -> &str;
    fn failure_domain(&self) -> &str;
    fn authenticated(&self) -> bool;
    fn fetch_object(&self, object_hash: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error>;
    fn put_object_immutable(&self, object_hash: &Hash, bytes: &[u8]) -> Result<(), ArchiveV2Error>;
    fn fetch_manifest(&self, object_hash: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error>;
    fn put_manifest_immutable(
        &self,
        object_hash: &Hash,
        bytes: &[u8],
    ) -> Result<(), ArchiveV2Error>;
    fn fetch_catalog(&self) -> Result<Option<Vec<u8>>, ArchiveV2Error>;
    fn put_catalog_extension(&self, catalog: &ArchiveV2Catalog) -> Result<(), ArchiveV2Error>;
}

#[derive(Debug, Clone)]
pub struct ArchiveV2DirectoryReplica {
    name: String,
    failure_domain: String,
    root: PathBuf,
    authenticated: bool,
}

impl ArchiveV2DirectoryReplica {
    pub fn new(
        name: impl Into<String>,
        failure_domain: impl Into<String>,
        root: impl Into<PathBuf>,
        authenticated: bool,
    ) -> Result<Self, ArchiveV2Error> {
        let replica = Self {
            name: name.into(),
            failure_domain: failure_domain.into(),
            root: root.into(),
            authenticated,
        };
        if replica.name.is_empty() || replica.failure_domain.is_empty() {
            return Err(ArchiveV2Error::Bounds(
                "replica name and failure domain must be non-empty".to_string(),
            ));
        }
        Ok(replica)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl ArchiveV2ReplicaTransport for ArchiveV2DirectoryReplica {
    fn name(&self) -> &str {
        &self.name
    }

    fn failure_domain(&self) -> &str {
        &self.failure_domain
    }

    fn authenticated(&self) -> bool {
        self.authenticated
    }

    fn fetch_object(&self, object_hash: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        read_optional(&replica_object_path(&self.root, object_hash))
    }

    fn put_object_immutable(&self, object_hash: &Hash, bytes: &[u8]) -> Result<(), ArchiveV2Error> {
        if Hash::hash(bytes) != *object_hash {
            return Err(ArchiveV2Error::WrongObjectHash);
        }
        write_immutable_atomic(&replica_object_path(&self.root, object_hash), bytes)
    }

    fn fetch_manifest(&self, object_hash: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        read_optional(&replica_manifest_path(&self.root, object_hash))
    }

    fn put_manifest_immutable(
        &self,
        object_hash: &Hash,
        bytes: &[u8],
    ) -> Result<(), ArchiveV2Error> {
        let manifest = ArchiveV2Manifest::decode_canonical(bytes)?;
        if manifest.segment_object_hash != *object_hash {
            return Err(ArchiveV2Error::WrongObjectHash);
        }
        write_immutable_atomic(&replica_manifest_path(&self.root, object_hash), bytes)
    }

    fn fetch_catalog(&self) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        read_optional(&self.root.join("catalog.av2"))
    }

    fn put_catalog_extension(&self, catalog: &ArchiveV2Catalog) -> Result<(), ArchiveV2Error> {
        let encoded = catalog.encode_canonical()?;
        ArchiveV2Catalog::import_extension_atomic(
            &self.root.join("catalog.av2"),
            &encoded,
            &catalog.identity,
            Some(catalog.catalog_root),
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveV2ReplicaPolicy {
    pub required_replicas: usize,
    pub required_failure_domains: usize,
    pub require_authenticated: bool,
}

impl ArchiveV2ReplicaPolicy {
    pub fn validate(self, configured: usize) -> Result<Self, ArchiveV2Error> {
        if self.required_replicas == 0
            || self.required_replicas > configured
            || self.required_failure_domains == 0
            || self.required_failure_domains > self.required_replicas
        {
            return Err(ArchiveV2Error::Bounds(
                "replica policy exceeds configured destinations".to_string(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2ReplicaObjectInventory {
    pub object_hash: Hash,
    pub start_slot: u64,
    pub end_slot: u64,
    pub verified_replicas: Vec<String>,
    pub verified_failure_domains: Vec<String>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2ReplicaInventory {
    pub catalog_root: Hash,
    pub objects: Vec<ArchiveV2ReplicaObjectInventory>,
    pub complete_replicas: Vec<String>,
    pub complete_failure_domains: Vec<String>,
    pub policy_satisfied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveV2MirrorLimits {
    pub max_objects: u64,
    pub max_bytes: u64,
}

impl ArchiveV2MirrorLimits {
    pub fn validate(self) -> Result<Self, ArchiveV2Error> {
        if self.max_objects == 0 || self.max_objects > 1_000 {
            return Err(ArchiveV2Error::Bounds(
                "mirror max_objects must be in 1..=1000".to_string(),
            ));
        }
        if self.max_bytes < 1024 * 1024 || self.max_bytes > 1024 * 1024 * 1024 * 1024 {
            return Err(ArchiveV2Error::Bounds(
                "mirror max_bytes must be in 1 MiB..=1 TiB".to_string(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2MirrorReport {
    pub catalog_root: Hash,
    pub mirrored_objects: u64,
    pub mirrored_bytes: u64,
    pub next_object_index: u64,
    pub complete: bool,
    pub source_failures: Vec<String>,
    pub destination_failures: Vec<String>,
    pub acknowledgements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveV2MirrorJournal {
    journal_version: u16,
    identity: ArchiveV2Identity,
    catalog_root: Hash,
    object_count: u64,
    sources: Vec<(String, String, bool)>,
    destinations: Vec<(String, String, bool)>,
    policy: ArchiveV2ReplicaPolicy,
    next_object_index: u64,
    last_object_hash: Option<Hash>,
    catalog_published: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveV2MirrorFaultPoint {
    AfterDestinationWrite,
    AfterProgressJournal,
}

pub struct ArchiveV2Replicator {
    sources: Vec<Arc<dyn ArchiveV2ReplicaTransport>>,
    destinations: Vec<Arc<dyn ArchiveV2ReplicaTransport>>,
    policy: ArchiveV2ReplicaPolicy,
}

impl ArchiveV2Replicator {
    pub fn new(
        sources: Vec<Arc<dyn ArchiveV2ReplicaTransport>>,
        destinations: Vec<Arc<dyn ArchiveV2ReplicaTransport>>,
        policy: ArchiveV2ReplicaPolicy,
    ) -> Result<Self, ArchiveV2Error> {
        policy.validate(destinations.len())?;
        validate_transport_set(&sources, "source")?;
        validate_transport_set(&destinations, "destination")?;
        if policy.require_authenticated
            && destinations
                .iter()
                .filter(|destination| destination.authenticated())
                .count()
                < policy.required_replicas
        {
            return Err(ArchiveV2Error::Role(
                "replica policy requires more authenticated destinations".to_string(),
            ));
        }
        if !sources
            .iter()
            .any(|source| source.authenticated() || !policy.require_authenticated)
        {
            return Err(ArchiveV2Error::Role(
                "replication has no permitted source".to_string(),
            ));
        }
        Ok(Self {
            sources,
            destinations,
            policy,
        })
    }

    pub fn mirror_pass(
        &self,
        catalog: &ArchiveV2Catalog,
        journal_path: &Path,
        limits: ArchiveV2MirrorLimits,
    ) -> Result<ArchiveV2MirrorReport, ArchiveV2Error> {
        self.mirror_pass_inner(catalog, journal_path, limits, None)
    }

    #[cfg(test)]
    fn mirror_pass_faulted(
        &self,
        catalog: &ArchiveV2Catalog,
        journal_path: &Path,
        limits: ArchiveV2MirrorLimits,
        fault: ArchiveV2MirrorFaultPoint,
    ) -> Result<ArchiveV2MirrorReport, ArchiveV2Error> {
        self.mirror_pass_inner(catalog, journal_path, limits, Some(fault))
    }

    fn mirror_pass_inner(
        &self,
        catalog: &ArchiveV2Catalog,
        journal_path: &Path,
        limits: ArchiveV2MirrorLimits,
        fault: Option<ArchiveV2MirrorFaultPoint>,
    ) -> Result<ArchiveV2MirrorReport, ArchiveV2Error> {
        catalog.validate()?;
        let limits = limits.validate()?;
        let manifests = active_manifests(catalog)?;
        let mut journal = if journal_path.exists() {
            let journal = load_mirror_journal(journal_path)?;
            validate_mirror_journal(
                &journal,
                catalog,
                manifests.len(),
                &self.sources,
                &self.destinations,
                self.policy,
            )?;
            journal
        } else {
            let journal = ArchiveV2MirrorJournal {
                journal_version: MIRROR_JOURNAL_VERSION,
                identity: catalog.identity.clone(),
                catalog_root: catalog.catalog_root,
                object_count: manifests.len() as u64,
                sources: transport_identities(&self.sources),
                destinations: transport_identities(&self.destinations),
                policy: self.policy,
                next_object_index: 0,
                last_object_hash: None,
                catalog_published: false,
            };
            store_mirror_journal(journal_path, &journal)?;
            journal
        };
        let mut report = ArchiveV2MirrorReport {
            catalog_root: catalog.catalog_root,
            next_object_index: journal.next_object_index,
            complete: journal.catalog_published,
            ..ArchiveV2MirrorReport::default()
        };
        if journal.catalog_published {
            return Ok(report);
        }
        while report.mirrored_objects < limits.max_objects
            && (journal.next_object_index as usize) < manifests.len()
        {
            let manifest = manifests[journal.next_object_index as usize];
            let bytes = self.fetch_verified_object(manifest, &catalog.identity, &mut report)?;
            if report.mirrored_bytes.saturating_add(bytes.len() as u64) > limits.max_bytes {
                if report.mirrored_objects == 0 {
                    return Err(ArchiveV2Error::Bounds(format!(
                        "next mirror object is {} bytes, above the per-pass byte limit",
                        bytes.len()
                    )));
                }
                break;
            }
            let acknowledgements =
                self.replicate_verified_object(manifest, &catalog.identity, &bytes, &mut report)?;
            validate_replica_acknowledgements(&acknowledgements, self.policy)?;
            maybe_mirror_fault(fault, ArchiveV2MirrorFaultPoint::AfterDestinationWrite)?;

            journal.next_object_index = journal.next_object_index.saturating_add(1);
            journal.last_object_hash = Some(manifest.segment_object_hash);
            store_mirror_journal(journal_path, &journal)?;
            maybe_mirror_fault(fault, ArchiveV2MirrorFaultPoint::AfterProgressJournal)?;
            report.mirrored_objects = report.mirrored_objects.saturating_add(1);
            report.mirrored_bytes = report.mirrored_bytes.saturating_add(bytes.len() as u64);
            report.next_object_index = journal.next_object_index;
            report.acknowledgements = acknowledgements
                .iter()
                .map(|(name, _)| name.clone())
                .collect();
        }

        if journal.next_object_index as usize == manifests.len() {
            let mut catalog_acknowledgements = Vec::new();
            for destination in &self.destinations {
                if self.policy.require_authenticated && !destination.authenticated() {
                    continue;
                }
                match destination
                    .put_catalog_extension(catalog)
                    .and_then(|()| verify_remote_catalog(destination.as_ref(), catalog))
                {
                    Ok(()) => catalog_acknowledgements.push((
                        destination.name().to_string(),
                        destination.failure_domain().to_string(),
                    )),
                    Err(error) => report.destination_failures.push(format!(
                        "{} catalog publication failed: {error}",
                        destination.name()
                    )),
                }
            }
            validate_replica_acknowledgements(&catalog_acknowledgements, self.policy)?;
            journal.catalog_published = true;
            store_mirror_journal(journal_path, &journal)?;
            report.complete = true;
            report.acknowledgements = catalog_acknowledgements
                .iter()
                .map(|(name, _)| name.clone())
                .collect();
        }
        Ok(report)
    }

    fn fetch_verified_object(
        &self,
        manifest: &ArchiveV2Manifest,
        identity: &ArchiveV2Identity,
        report: &mut ArchiveV2MirrorReport,
    ) -> Result<Vec<u8>, ArchiveV2Error> {
        for source in &self.sources {
            if self.policy.require_authenticated && !source.authenticated() {
                continue;
            }
            let expected_manifest = manifest.encode_canonical()?;
            match source.fetch_manifest(&manifest.segment_object_hash) {
                Ok(Some(bytes)) if bytes == expected_manifest => {}
                Ok(Some(_)) => {
                    report
                        .source_failures
                        .push(format!("{} returned a conflicting manifest", source.name()));
                    continue;
                }
                Ok(None) => {
                    report
                        .source_failures
                        .push(format!("{} does not have the manifest", source.name()));
                    continue;
                }
                Err(error) => {
                    report
                        .source_failures
                        .push(format!("{} manifest fetch failed: {error}", source.name()));
                    continue;
                }
            }
            match source.fetch_object(&manifest.segment_object_hash) {
                Ok(Some(bytes)) => {
                    if Hash::hash(&bytes) != manifest.segment_object_hash {
                        report
                            .source_failures
                            .push(format!("{} returned the wrong object hash", source.name()));
                        continue;
                    }
                    match ArchiveV2SegmentCodec::decode(&bytes, manifest, identity) {
                        Ok(_) => return Ok(bytes),
                        Err(error) => report.source_failures.push(format!(
                            "{} returned an invalid object: {error}",
                            source.name()
                        )),
                    }
                }
                Ok(None) => report
                    .source_failures
                    .push(format!("{} does not have the object", source.name())),
                Err(error) => report
                    .source_failures
                    .push(format!("{} fetch failed: {error}", source.name())),
            }
        }
        Err(ArchiveV2Error::Unavailable(format!(
            "no verified source supplied segment {}",
            manifest.segment_object_hash
        )))
    }

    fn replicate_verified_object(
        &self,
        manifest: &ArchiveV2Manifest,
        identity: &ArchiveV2Identity,
        bytes: &[u8],
        report: &mut ArchiveV2MirrorReport,
    ) -> Result<Vec<(String, String)>, ArchiveV2Error> {
        let mut acknowledgements = Vec::new();
        for destination in &self.destinations {
            if self.policy.require_authenticated && !destination.authenticated() {
                continue;
            }
            let manifest_bytes = manifest.encode_canonical()?;
            let result = destination
                .put_object_immutable(&manifest.segment_object_hash, bytes)
                .and_then(|()| {
                    destination
                        .put_manifest_immutable(&manifest.segment_object_hash, &manifest_bytes)
                })
                .and_then(|()| {
                    destination
                        .fetch_object(&manifest.segment_object_hash)?
                        .ok_or_else(|| {
                            ArchiveV2Error::Unavailable(
                                "destination lost object after upload".to_string(),
                            )
                        })
                })
                .and_then(|replica| {
                    if Hash::hash(&replica) != manifest.segment_object_hash {
                        return Err(ArchiveV2Error::WrongObjectHash);
                    }
                    ArchiveV2SegmentCodec::decode(&replica, manifest, identity)?;
                    let replica_manifest = destination
                        .fetch_manifest(&manifest.segment_object_hash)?
                        .ok_or_else(|| {
                            ArchiveV2Error::Unavailable(
                                "destination lost manifest after upload".to_string(),
                            )
                        })?;
                    if replica_manifest != manifest_bytes {
                        return Err(ArchiveV2Error::WrongRoot);
                    }
                    Ok(())
                });
            match result {
                Ok(()) => acknowledgements.push((
                    destination.name().to_string(),
                    destination.failure_domain().to_string(),
                )),
                Err(ArchiveV2Error::Ordering(error)) => {
                    return Err(ArchiveV2Error::Ordering(format!(
                        "immutable destination {} conflicts: {error}",
                        destination.name()
                    )));
                }
                Err(error) => report.destination_failures.push(format!(
                    "{} object replication failed: {error}",
                    destination.name()
                )),
            }
        }
        Ok(acknowledgements)
    }
}

pub fn inspect_archive_v2_replica_inventory(
    catalog: &ArchiveV2Catalog,
    replicas: &[Arc<dyn ArchiveV2ReplicaTransport>],
    policy: ArchiveV2ReplicaPolicy,
) -> Result<ArchiveV2ReplicaInventory, ArchiveV2Error> {
    catalog.validate()?;
    policy.validate(replicas.len())?;
    validate_transport_set(replicas, "inventory replica")?;
    let manifests = active_manifests(catalog)?;
    let mut inventory = ArchiveV2ReplicaInventory {
        catalog_root: catalog.catalog_root,
        ..ArchiveV2ReplicaInventory::default()
    };
    let mut complete = replicas
        .iter()
        .filter(|replica| replica.authenticated() || !policy.require_authenticated)
        .map(|replica| replica.name().to_string())
        .collect::<BTreeSet<_>>();
    for manifest in manifests {
        let mut object = ArchiveV2ReplicaObjectInventory {
            object_hash: manifest.segment_object_hash,
            start_slot: manifest.start_slot,
            end_slot: manifest.end_slot,
            ..ArchiveV2ReplicaObjectInventory::default()
        };
        let mut domains = BTreeSet::new();
        for replica in replicas {
            if policy.require_authenticated && !replica.authenticated() {
                complete.remove(replica.name());
                continue;
            }
            let verified = replica
                .fetch_manifest(&manifest.segment_object_hash)
                .and_then(|bytes| {
                    bytes.ok_or_else(|| {
                        ArchiveV2Error::Unavailable("manifest is missing".to_string())
                    })
                })
                .and_then(|bytes| {
                    if ArchiveV2Manifest::decode_canonical(&bytes)? != *manifest {
                        return Err(ArchiveV2Error::WrongRoot);
                    }
                    Ok(())
                })
                .and_then(|()| replica.fetch_object(&manifest.segment_object_hash))
                .and_then(|bytes| {
                    bytes
                        .ok_or_else(|| ArchiveV2Error::Unavailable("object is missing".to_string()))
                })
                .and_then(|bytes| {
                    if Hash::hash(&bytes) != manifest.segment_object_hash {
                        return Err(ArchiveV2Error::WrongObjectHash);
                    }
                    ArchiveV2SegmentCodec::decode(&bytes, manifest, &catalog.identity)?;
                    Ok(())
                });
            match verified {
                Ok(()) => {
                    object.verified_replicas.push(replica.name().to_string());
                    domains.insert(replica.failure_domain().to_string());
                }
                Err(error) => {
                    complete.remove(replica.name());
                    object.failures.push(format!("{}: {error}", replica.name()));
                }
            }
        }
        object.verified_failure_domains = domains.into_iter().collect();
        inventory.objects.push(object);
    }
    let mut complete_domains = BTreeSet::new();
    for replica in replicas {
        if complete.contains(replica.name()) {
            if verify_remote_catalog(replica.as_ref(), catalog).is_ok() {
                complete_domains.insert(replica.failure_domain().to_string());
            } else {
                complete.remove(replica.name());
            }
        }
    }
    inventory.complete_replicas = complete.into_iter().collect();
    inventory.complete_failure_domains = complete_domains.into_iter().collect();
    inventory.policy_satisfied = inventory.complete_replicas.len() >= policy.required_replicas
        && inventory.complete_failure_domains.len() >= policy.required_failure_domains;
    Ok(inventory)
}

fn active_manifests(catalog: &ArchiveV2Catalog) -> Result<Vec<&ArchiveV2Manifest>, ArchiveV2Error> {
    catalog
        .entries
        .iter()
        .map(|entry| catalog.active_manifest(&entry.manifest.segment_object_hash))
        .collect()
}

fn validate_transport_set(
    transports: &[Arc<dyn ArchiveV2ReplicaTransport>],
    kind: &str,
) -> Result<(), ArchiveV2Error> {
    let names = transports
        .iter()
        .map(|transport| transport.name())
        .collect::<BTreeSet<_>>();
    if names.len() != transports.len() || names.iter().any(|name| name.is_empty()) {
        return Err(ArchiveV2Error::Bounds(format!(
            "{kind} names must be non-empty and unique"
        )));
    }
    if transports
        .iter()
        .any(|transport| transport.failure_domain().is_empty())
    {
        return Err(ArchiveV2Error::Bounds(format!(
            "{kind} failure domains must be non-empty"
        )));
    }
    Ok(())
}

fn validate_replica_acknowledgements(
    acknowledgements: &[(String, String)],
    policy: ArchiveV2ReplicaPolicy,
) -> Result<(), ArchiveV2Error> {
    let replicas = acknowledgements
        .iter()
        .map(|(name, _)| name)
        .collect::<BTreeSet<_>>();
    let domains = acknowledgements
        .iter()
        .map(|(_, domain)| domain)
        .collect::<BTreeSet<_>>();
    if replicas.len() < policy.required_replicas || domains.len() < policy.required_failure_domains
    {
        return Err(ArchiveV2Error::Unavailable(format!(
            "replication acknowledged {} replicas in {} failure domains; policy requires {} and {}",
            replicas.len(),
            domains.len(),
            policy.required_replicas,
            policy.required_failure_domains
        )));
    }
    Ok(())
}

fn verify_remote_catalog(
    transport: &dyn ArchiveV2ReplicaTransport,
    catalog: &ArchiveV2Catalog,
) -> Result<(), ArchiveV2Error> {
    let bytes = transport.fetch_catalog()?.ok_or_else(|| {
        ArchiveV2Error::Unavailable(format!("{} catalog is missing", transport.name()))
    })?;
    let remote = ArchiveV2Catalog::decode_canonical(&bytes)?;
    if remote.identity != catalog.identity || remote.catalog_root != catalog.catalog_root {
        return Err(ArchiveV2Error::WrongRoot);
    }
    Ok(())
}

fn validate_mirror_journal(
    journal: &ArchiveV2MirrorJournal,
    catalog: &ArchiveV2Catalog,
    object_count: usize,
    sources: &[Arc<dyn ArchiveV2ReplicaTransport>],
    destinations: &[Arc<dyn ArchiveV2ReplicaTransport>],
    policy: ArchiveV2ReplicaPolicy,
) -> Result<(), ArchiveV2Error> {
    if journal.journal_version != MIRROR_JOURNAL_VERSION
        || journal.identity != catalog.identity
        || journal.catalog_root != catalog.catalog_root
        || journal.object_count != object_count as u64
        || journal.sources != transport_identities(sources)
        || journal.destinations != transport_identities(destinations)
        || journal.policy != policy
        || journal.next_object_index > journal.object_count
        || (journal.catalog_published && journal.next_object_index != journal.object_count)
        || (journal.next_object_index == 0 && journal.last_object_hash.is_some())
    {
        return Err(ArchiveV2Error::WrongRoot);
    }
    if journal.next_object_index > 0 {
        let manifests = active_manifests(catalog)?;
        if manifests
            .get(journal.next_object_index as usize - 1)
            .map(|manifest| manifest.segment_object_hash)
            != journal.last_object_hash
        {
            return Err(ArchiveV2Error::Continuity(
                "mirror journal cursor does not match the catalog".to_string(),
            ));
        }
    }
    Ok(())
}

fn transport_identities(
    transports: &[Arc<dyn ArchiveV2ReplicaTransport>],
) -> Vec<(String, String, bool)> {
    transports
        .iter()
        .map(|transport| {
            (
                transport.name().to_string(),
                transport.failure_domain().to_string(),
                transport.authenticated(),
            )
        })
        .collect()
}

fn maybe_mirror_fault(
    requested: Option<ArchiveV2MirrorFaultPoint>,
    point: ArchiveV2MirrorFaultPoint,
) -> Result<(), ArchiveV2Error> {
    if requested == Some(point) {
        Err(ArchiveV2Error::Io(format!(
            "injected Archive V2 mirror fault at {point:?}"
        )))
    } else {
        Ok(())
    }
}

fn replica_object_path(root: &Path, object_hash: &Hash) -> PathBuf {
    root.join("objects")
        .join(format!("{}.av2s", object_hash.to_hex()))
}

fn replica_manifest_path(root: &Path, object_hash: &Hash) -> PathBuf {
    root.join("manifests")
        .join(format!("{}.av2m", object_hash.to_hex()))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ArchiveV2Error::Io(format!(
            "failed reading {}: {error}",
            path.display()
        ))),
    }
}

fn write_immutable_atomic(path: &Path, bytes: &[u8]) -> Result<(), ArchiveV2Error> {
    let parent = path
        .parent()
        .ok_or_else(|| ArchiveV2Error::Io("replica object has no parent".to_string()))?;
    fs::create_dir_all(parent)?;
    if path.exists() {
        if fs::read(path)? == bytes {
            return Ok(());
        }
        return Err(ArchiveV2Error::Ordering(format!(
            "{} already contains different immutable bytes",
            path.display()
        )));
    }
    let temporary = parent.join(format!(
        ".replica.{}.{}.tmp",
        std::process::id(),
        REPLICA_TEMPORARY_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
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

fn encode_mirror_journal(journal: &ArchiveV2MirrorJournal) -> Result<Vec<u8>, ArchiveV2Error> {
    let payload = serialize_legacy_bincode(journal, "Archive V2 mirror journal")
        .map_err(ArchiveV2Error::Codec)?;
    if payload.len() > MAX_MIRROR_JOURNAL_BYTES {
        return Err(ArchiveV2Error::Bounds(
            "mirror journal is too large".to_string(),
        ));
    }
    let mut encoded = Vec::with_capacity(MIRROR_JOURNAL_MAGIC.len() + 4 + payload.len() + 32);
    encoded.extend_from_slice(MIRROR_JOURNAL_MAGIC);
    encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&payload);
    encoded.extend_from_slice(&Hash::hash(&payload).0);
    Ok(encoded)
}

fn load_mirror_journal(path: &Path) -> Result<ArchiveV2MirrorJournal, ArchiveV2Error> {
    let encoded = fs::read(path)?;
    let minimum = MIRROR_JOURNAL_MAGIC.len() + 4 + 32;
    if encoded.len() < minimum || !encoded.starts_with(MIRROR_JOURNAL_MAGIC) {
        return Err(ArchiveV2Error::Truncated("mirror journal"));
    }
    let offset = MIRROR_JOURNAL_MAGIC.len();
    let payload_len = u32::from_le_bytes(
        encoded[offset..offset + 4]
            .try_into()
            .map_err(|_| ArchiveV2Error::Truncated("mirror journal length"))?,
    ) as usize;
    if payload_len > MAX_MIRROR_JOURNAL_BYTES {
        return Err(ArchiveV2Error::Bounds(
            "mirror journal is too large".to_string(),
        ));
    }
    let start = offset + 4;
    let end = start
        .checked_add(payload_len)
        .ok_or_else(|| ArchiveV2Error::Bounds("mirror journal length overflow".to_string()))?;
    if end.checked_add(32) != Some(encoded.len())
        || Hash::hash(&encoded[start..end]).0 != encoded[end..]
    {
        return Err(ArchiveV2Error::WrongRoot);
    }
    deserialize_legacy_bincode_strict(
        &encoded[start..end],
        MAX_MIRROR_JOURNAL_BYTES as u64,
        "Archive V2 mirror journal",
    )
    .map_err(ArchiveV2Error::Codec)
}

fn store_mirror_journal(
    path: &Path,
    journal: &ArchiveV2MirrorJournal,
) -> Result<(), ArchiveV2Error> {
    let encoded = encode_mirror_journal(journal)?;
    let parent = path
        .parent()
        .ok_or_else(|| ArchiveV2Error::Io("mirror journal has no parent".to_string()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".mirror-journal.{}.{}.tmp",
        std::process::id(),
        REPLICA_TEMPORARY_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::archive_v2::{
        ArchiveV2CodecConfig, ArchiveV2SegmentContents, ARCHIVE_V2_FORMAT_VERSION,
    };
    use crate::Block;

    fn fixture() -> (
        ArchiveV2Catalog,
        Vec<u8>,
        Arc<dyn ArchiveV2ReplicaTransport>,
    ) {
        let identity = ArchiveV2Identity {
            network_id: "replication-testnet".to_string(),
            genesis_hash: Hash::hash(b"replication-genesis"),
        };
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"replication-state"),
            [4; 32],
            Vec::new(),
            1,
        );
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
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
        assert_eq!(manifest.format_version, ARCHIVE_V2_FORMAT_VERSION);
        let mut catalog = ArchiveV2Catalog::empty(identity).unwrap();
        catalog.append(manifest.clone()).unwrap();
        let source_root = tempdir().unwrap().keep();
        let source = Arc::new(
            ArchiveV2DirectoryReplica::new("source", "source-region", &source_root, true).unwrap(),
        );
        source
            .put_object_immutable(&manifest.segment_object_hash, &bytes)
            .unwrap();
        source
            .put_manifest_immutable(
                &manifest.segment_object_hash,
                &manifest.encode_canonical().unwrap(),
            )
            .unwrap();
        source.put_catalog_extension(&catalog).unwrap();
        (catalog, bytes, source)
    }

    #[test]
    fn mirror_resumes_after_destination_write_and_inventory_proves_domains() {
        let (catalog, _, source) = fixture();
        let destination_a_root = tempdir().unwrap();
        let destination_b_root = tempdir().unwrap();
        let destination_a: Arc<dyn ArchiveV2ReplicaTransport> = Arc::new(
            ArchiveV2DirectoryReplica::new(
                "destination-a",
                "region-a",
                destination_a_root.path(),
                true,
            )
            .unwrap(),
        );
        let destination_b: Arc<dyn ArchiveV2ReplicaTransport> = Arc::new(
            ArchiveV2DirectoryReplica::new(
                "destination-b",
                "region-b",
                destination_b_root.path(),
                true,
            )
            .unwrap(),
        );
        let destinations = vec![destination_a.clone(), destination_b.clone()];
        let policy = ArchiveV2ReplicaPolicy {
            required_replicas: 2,
            required_failure_domains: 2,
            require_authenticated: true,
        };
        let replicator =
            ArchiveV2Replicator::new(vec![source], destinations.clone(), policy).unwrap();
        let journal_root = tempdir().unwrap();
        let journal = journal_root.path().join("mirror.journal");
        assert!(replicator
            .mirror_pass_faulted(
                &catalog,
                &journal,
                ArchiveV2MirrorLimits {
                    max_objects: 1,
                    max_bytes: 1024 * 1024,
                },
                ArchiveV2MirrorFaultPoint::AfterDestinationWrite,
            )
            .is_err());
        let report = replicator
            .mirror_pass(
                &catalog,
                &journal,
                ArchiveV2MirrorLimits {
                    max_objects: 1,
                    max_bytes: 1024 * 1024,
                },
            )
            .unwrap();
        assert!(report.complete);
        let inventory =
            inspect_archive_v2_replica_inventory(&catalog, &destinations, policy).unwrap();
        assert!(inventory.policy_satisfied);
        assert_eq!(inventory.complete_replicas.len(), 2);
        assert_eq!(inventory.complete_failure_domains.len(), 2);
    }

    #[test]
    fn mirror_journal_is_bound_to_exact_destination_set_and_policy() {
        let (catalog, _, source) = fixture();
        let destination_a_root = tempdir().unwrap();
        let destination_b_root = tempdir().unwrap();
        let destination_a: Arc<dyn ArchiveV2ReplicaTransport> = Arc::new(
            ArchiveV2DirectoryReplica::new(
                "destination-a",
                "region-a",
                destination_a_root.path(),
                true,
            )
            .unwrap(),
        );
        let destination_b: Arc<dyn ArchiveV2ReplicaTransport> = Arc::new(
            ArchiveV2DirectoryReplica::new(
                "destination-b",
                "region-b",
                destination_b_root.path(),
                true,
            )
            .unwrap(),
        );
        let policy = ArchiveV2ReplicaPolicy {
            required_replicas: 1,
            required_failure_domains: 1,
            require_authenticated: true,
        };
        let first =
            ArchiveV2Replicator::new(vec![source.clone()], vec![destination_a], policy).unwrap();
        let journal_root = tempdir().unwrap();
        let journal = journal_root.path().join("destination-bound.journal");
        assert!(first
            .mirror_pass_faulted(
                &catalog,
                &journal,
                ArchiveV2MirrorLimits {
                    max_objects: 1,
                    max_bytes: 1024 * 1024,
                },
                ArchiveV2MirrorFaultPoint::AfterProgressJournal,
            )
            .is_err());

        let changed = ArchiveV2Replicator::new(vec![source], vec![destination_b], policy).unwrap();
        assert!(matches!(
            changed.mirror_pass(
                &catalog,
                &journal,
                ArchiveV2MirrorLimits {
                    max_objects: 1,
                    max_bytes: 1024 * 1024,
                },
            ),
            Err(ArchiveV2Error::WrongRoot)
        ));
    }

    #[test]
    fn mirror_fails_over_from_corrupt_source_and_rejects_destination_conflict() {
        let (catalog, bytes, good_source) = fixture();
        let manifest = &catalog.entries[0].manifest;
        let corrupt_root = tempdir().unwrap();
        let corrupt_source: Arc<dyn ArchiveV2ReplicaTransport> = Arc::new(
            ArchiveV2DirectoryReplica::new(
                "corrupt-source",
                "source-region-b",
                corrupt_root.path(),
                true,
            )
            .unwrap(),
        );
        fs::create_dir_all(corrupt_root.path().join("objects")).unwrap();
        fs::write(
            replica_object_path(corrupt_root.path(), &manifest.segment_object_hash),
            b"corrupt",
        )
        .unwrap();
        let destination_root = tempdir().unwrap();
        let destination: Arc<dyn ArchiveV2ReplicaTransport> = Arc::new(
            ArchiveV2DirectoryReplica::new(
                "destination",
                "region-a",
                destination_root.path(),
                true,
            )
            .unwrap(),
        );
        let policy = ArchiveV2ReplicaPolicy {
            required_replicas: 1,
            required_failure_domains: 1,
            require_authenticated: true,
        };
        let replicator = ArchiveV2Replicator::new(
            vec![corrupt_source, good_source],
            vec![destination.clone()],
            policy,
        )
        .unwrap();
        let journal_root = tempdir().unwrap();
        let report = replicator
            .mirror_pass(
                &catalog,
                &journal_root.path().join("mirror.journal"),
                ArchiveV2MirrorLimits {
                    max_objects: 1,
                    max_bytes: 1024 * 1024,
                },
            )
            .unwrap();
        assert!(report.complete);
        assert_eq!(report.source_failures.len(), 1);
        assert_eq!(
            destination
                .fetch_object(&manifest.segment_object_hash)
                .unwrap()
                .unwrap(),
            bytes
        );

        fs::write(
            replica_object_path(destination_root.path(), &manifest.segment_object_hash),
            b"conflicting immutable bytes",
        )
        .unwrap();
        let second_journal = journal_root.path().join("mirror-conflict.journal");
        assert!(matches!(
            replicator.mirror_pass(
                &catalog,
                &second_journal,
                ArchiveV2MirrorLimits {
                    max_objects: 1,
                    max_bytes: 1024 * 1024,
                },
            ),
            Err(ArchiveV2Error::Ordering(_))
        ));
    }
}
