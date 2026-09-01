use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lichen_core::archive_v2::{
    archive_v2_state_admission_fingerprint, benchmark_archive_v2_range,
    discover_archive_v2_catalog, load_archive_v2_role_marker,
    store_archive_v2_role_marker_create_new, ArchiveV2AdaptiveReservePolicy,
    ArchiveV2BenchmarkCandidate, ArchiveV2BenchmarkPlan, ArchiveV2BuildOptions, ArchiveV2Builder,
    ArchiveV2CapabilityAdvertisement, ArchiveV2CapacityDecision, ArchiveV2CapacityGuard,
    ArchiveV2CapacityInputs, ArchiveV2CapacityThresholds, ArchiveV2CapacityTotals,
    ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2DictionaryKind, ArchiveV2DirectoryReplica,
    ArchiveV2DirectorySource, ArchiveV2Identity, ArchiveV2LegacyLossDeclaration, ArchiveV2Manifest,
    ArchiveV2MirrorLimits, ArchiveV2ObjectSource, ArchiveV2PressureAction, ArchiveV2Reader,
    ArchiveV2ReaderConfig, ArchiveV2ReplicaEvidence, ArchiveV2ReplicaPolicy,
    ArchiveV2ReplicaTransport, ArchiveV2Replicator, ArchiveV2RetirementManifest, ArchiveV2Role,
    ArchiveV2RoleAdmission, ArchiveV2RoleConfig, ArchiveV2RoleMarker, ArchiveV2RoleRequirements,
    ArchiveV2RollbackAnchor, ArchiveV2SegmentCodec, ARCHIVE_V2_CATALOG_VERSION,
    ARCHIVE_V2_FORMAT_VERSION, ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS, ARCHIVE_V2_ROLE_CONFIG_VERSION,
    ARCHIVE_V2_ROLE_MARKER_FILENAME, ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY,
};
use lichen_core::codec::serialized_size_legacy_bincode;
use lichen_core::{
    genesis_block_declares_mossstake_slot_only, keypair_password_from_env,
    plaintext_keypair_allowed_for_local_dev, ArchiveV2RetirementLimits,
    ArchiveV2RetirementPassReport, ArchiveV2RetirementPhase, ArchiveV2RetirementReclaimLimits,
    CheckpointMeta, CheckpointSnapshotProfile, Hash, KeypairFile, StateStore,
    PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
};
use serde_json::json;

const DEFAULT_MAX_MIRROR_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_VERIFY_OBJECTS: u64 = 10_000;
const TESTNET_CAPACITY_FLOOR_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const DEFAULT_CAPACITY_FLOOR_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const CAPACITY_RESERVE_BASIS_POINTS: u16 = 500;
const SEGMENT_OPERATION_PEAK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const EVIDENCE_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RETIREMENT_PASSES_PER_OPEN: u64 = 16;
const RUNTIME_MUTABLE_WRITE_PEAK_BYTES: u64 = 1024 * 1024 * 1024;
const RUNTIME_WAL_PEAK_BYTES: u64 = 1024 * 1024 * 1024;
const RUNTIME_COMPACTION_PEAK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const RUNTIME_CHECKPOINT_PEAK_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const RUNTIME_CACHE_EVICTION_MARGIN_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PREFLIGHT_MANIFEST_FILE_BYTES: u64 = 16 * 1024 * 1024 + 64;

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("lichen-archive-v2: {error}");
        std::process::exit(2);
    }
}

fn run(raw: Vec<String>) -> Result<(), String> {
    let (command, args) = CommandArgs::parse(raw)?;
    match command.as_str() {
        "status" => run_status(&args),
        "catalog-extension-check" => run_catalog_extension_check(&args),
        "role-preflight" => run_role_preflight(&args),
        "role-bootstrap" => run_role_bootstrap(&args),
        "snapshot-hot" => run_snapshot_hot(&args),
        "verify" => run_verify(&args),
        "repair" => run_repair(&args),
        "declare-legacy-loss" => run_declare_legacy_loss(&args),
        "retirement-authorize" => run_retirement_authorize(&args),
        "retirement-pass" => run_retirement_pass(&args),
        "retirement-reclaim" => run_retirement_reclaim(&args),
        "build" => run_build(&args),
        "mirror" => run_mirror(&args),
        "restore" => run_restore(&args),
        "public-history-manifest" => run_public_history_manifest(&args),
        "profile-source" => run_profile_source(&args),
        "benchmark" => run_benchmark(&args),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        _ => Err(format!(
            "unknown command {command:?}; expected status, catalog-extension-check, role-preflight, role-bootstrap, snapshot-hot, verify, repair, declare-legacy-loss, build, mirror, restore, retirement-authorize, retirement-pass, retirement-reclaim, public-history-manifest, profile-source, or benchmark"
        )),
    }
}

#[derive(Debug, Default)]
struct CommandArgs {
    values: BTreeMap<String, Vec<String>>,
    flags: BTreeSet<String>,
}

impl CommandArgs {
    fn parse(raw: Vec<String>) -> Result<(String, Self), String> {
        let mut iter = raw.into_iter();
        let command = iter.next().unwrap_or_else(|| "help".to_string());
        let tokens = iter.collect::<Vec<_>>();
        let mut parsed = Self::default();
        let mut index = 0;
        while index < tokens.len() {
            let key = tokens[index]
                .strip_prefix("--")
                .filter(|key| !key.is_empty())
                .ok_or_else(|| format!("unexpected positional argument {:?}", tokens[index]))?
                .to_string();
            if index + 1 < tokens.len() && !tokens[index + 1].starts_with("--") {
                parsed
                    .values
                    .entry(key)
                    .or_default()
                    .push(tokens[index + 1].clone());
                index += 2;
            } else {
                if !parsed.flags.insert(key.clone()) {
                    return Err(format!("flag --{key} was provided more than once"));
                }
                index += 1;
            }
        }
        Ok((command, parsed))
    }

    fn ensure_only(&self, values: &[&str], flags: &[&str]) -> Result<(), String> {
        let allowed_values = values.iter().copied().collect::<BTreeSet<_>>();
        let allowed_flags = flags.iter().copied().collect::<BTreeSet<_>>();
        if let Some(key) = self
            .values
            .keys()
            .find(|key| !allowed_values.contains(key.as_str()))
        {
            return Err(format!("unsupported option --{key}"));
        }
        if let Some(key) = self
            .flags
            .iter()
            .find(|key| !allowed_flags.contains(key.as_str()))
        {
            return Err(format!("unsupported flag --{key}"));
        }
        Ok(())
    }

    fn required(&self, name: &str) -> Result<&str, String> {
        let values = self
            .values
            .get(name)
            .ok_or_else(|| format!("missing required --{name}"))?;
        if values.len() != 1 {
            return Err(format!("--{name} must be provided exactly once"));
        }
        Ok(&values[0])
    }

    fn optional(&self, name: &str) -> Result<Option<&str>, String> {
        match self.values.get(name) {
            Some(values) if values.len() == 1 => Ok(Some(&values[0])),
            Some(_) => Err(format!("--{name} must be provided at most once")),
            None => Ok(None),
        }
    }

    fn repeated(&self, name: &str) -> Vec<&str> {
        self.values
            .get(name)
            .map(|values| values.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn run_status(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(&["root", "history-start-slot"], &[])?;
    let root = PathBuf::from(args.required("root")?);
    let catalog =
        ArchiveV2Catalog::load(&root.join("catalog.av2")).map_err(|error| error.to_string())?;
    let checkpoint_handoff_root = args
        .optional("history-start-slot")?
        .map(|value| parse_u64(value, "history-start-slot"))
        .transpose()?
        .map(|history_start_slot| {
            catalog
                .checkpoint_handoff_root(history_start_slot)
                .map(|root| root.to_hex())
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let manifests = active_manifests(&catalog)?;
    let mut object_bytes = 0u64;
    let mut manifest_bytes = 0u64;
    let mut missing_object_count = 0u64;
    let mut missing_manifest_count = 0u64;
    let mut missing_objects = Vec::new();
    let mut missing_manifests = Vec::new();
    for manifest in &manifests {
        let hash = manifest.segment_object_hash;
        match fs::metadata(object_path(&root, &hash)) {
            Ok(metadata) if metadata.is_file() => {
                object_bytes = object_bytes.saturating_add(metadata.len())
            }
            _ => {
                missing_object_count = missing_object_count.saturating_add(1);
                push_bounded(&mut missing_objects, hash.to_hex());
            }
        }
        match fs::metadata(manifest_path(&root, &hash)) {
            Ok(metadata) if metadata.is_file() => {
                manifest_bytes = manifest_bytes.saturating_add(metadata.len())
            }
            _ => {
                missing_manifest_count = missing_manifest_count.saturating_add(1);
                push_bounded(&mut missing_manifests, hash.to_hex());
            }
        }
    }
    let slot_range = manifests
        .first()
        .zip(manifests.last())
        .map(|(first, last)| vec![first.start_slot, last.end_slot]);
    print_json(&json!({
        "operation": "status",
        "root": root,
        "network_id": catalog.identity.network_id,
        "genesis_hash": catalog.identity.genesis_hash.to_hex(),
        "catalog_root": catalog.catalog_root.to_hex(),
        "checkpoint_handoff_root": checkpoint_handoff_root,
        "segments": manifests.len(),
        "supersessions": catalog.supersessions.len(),
        "legacy_loss_declarations": catalog.legacy_loss_declarations,
        "slot_range": slot_range,
        "object_bytes": object_bytes,
        "manifest_bytes": manifest_bytes,
        "missing_object_count": missing_object_count,
        "missing_manifest_count": missing_manifest_count,
        "missing_objects_first_100": missing_objects,
        "missing_manifests_first_100": missing_manifests,
        "complete_local_inventory": missing_object_count == 0 && missing_manifest_count == 0,
    }))
}

fn run_catalog_extension_check(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(&["base-root", "incoming-root"], &[])?;
    let base_root = PathBuf::from(args.required("base-root")?);
    let incoming_root = PathBuf::from(args.required("incoming-root")?);
    let base = ArchiveV2Catalog::load(&base_root.join("catalog.av2"))
        .map_err(|error| error.to_string())?;
    let incoming = ArchiveV2Catalog::load(&incoming_root.join("catalog.av2"))
        .map_err(|error| error.to_string())?;
    let base_catalog_root = base.catalog_root;
    let incoming_catalog_root = incoming.catalog_root;
    let mut merged = base;
    let changed = merged
        .merge_verified_extension(&incoming)
        .map_err(|error| error.to_string())?;
    if merged != incoming {
        return Err("catalog extension check did not converge on incoming catalog".to_string());
    }
    print_json(&json!({
        "operation": "catalog-extension-check",
        "base_root": base_root,
        "incoming_root": incoming_root,
        "base_catalog_root": base_catalog_root.to_hex(),
        "incoming_catalog_root": incoming_catalog_root.to_hex(),
        "changed": changed,
        "exact_append_only_extension": true,
    }))
}

fn regular_nonempty_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_file() && metadata.len() > 0)
}

fn catalog_inventory_entry_matches(
    root: &Path,
    manifest: &ArchiveV2Manifest,
    expected_manifest_hash: Hash,
    maximum_object_bytes: u64,
) -> bool {
    let object = root
        .join("objects")
        .join(format!("{}.av2s", manifest.segment_object_hash.to_hex()));
    let manifest_path = root
        .join("manifests")
        .join(format!("{}.av2m", manifest.segment_object_hash.to_hex()));
    if !matches!(
        fs::symlink_metadata(&object),
        Ok(metadata)
            if metadata.file_type().is_file()
                && metadata.len() > 0
                && metadata.len() <= maximum_object_bytes
    ) || !matches!(
        fs::symlink_metadata(&manifest_path),
        Ok(metadata)
            if metadata.file_type().is_file()
                && metadata.len() > 0
                && metadata.len() <= MAX_PREFLIGHT_MANIFEST_FILE_BYTES
    ) {
        return false;
    }
    let encoded = match fs::read(&manifest_path) {
        Ok(encoded) => encoded,
        Err(_) => return false,
    };
    Hash::hash(&encoded) == expected_manifest_hash
        && ArchiveV2Manifest::decode_canonical(&encoded).is_ok_and(|decoded| decoded == *manifest)
}

fn complete_catalog_inventory(
    root: &Path,
    catalog: &ArchiveV2Catalog,
    maximum_object_bytes: u64,
) -> bool {
    catalog.entries.iter().all(|entry| {
        catalog_inventory_entry_matches(
            root,
            &entry.manifest,
            entry.manifest_hash,
            maximum_object_bytes,
        )
    })
}

fn runtime_role_capacity_decision(
    role: ArchiveV2Role,
    state_dir: &Path,
    archive_root: &Path,
    cache_root: Option<&Path>,
    source_max_object_bytes: u64,
    network_id: &str,
) -> Result<ArchiveV2CapacityDecision, String> {
    let hot = cli_filesystem_capacity(&capacity_probe_path(state_dir)?)?;
    let archive = cli_filesystem_capacity(&capacity_probe_path(archive_root)?)?;
    let cache = cache_root
        .map(capacity_probe_path)
        .transpose()?
        .as_deref()
        .map(cli_filesystem_capacity)
        .transpose()?
        .unwrap_or(archive);
    let absolute_reserve = if network_id == "lichen-testnet-1" {
        TESTNET_CAPACITY_FLOOR_BYTES
    } else {
        DEFAULT_CAPACITY_FLOOR_BYTES
    };
    ArchiveV2CapacityGuard::evaluate_adaptive(
        ArchiveV2CapacityInputs {
            segment_build_enabled: role == ArchiveV2Role::FullArchive,
            verified_cache_enabled: role == ArchiveV2Role::VerifiedCache,
            checkpoint_enabled: true,
            hot_available_bytes: hot.available_bytes,
            archive_available_bytes: archive.available_bytes,
            cache_available_bytes: cache.available_bytes,
            mutable_state_write_peak_bytes: RUNTIME_MUTABLE_WRITE_PEAK_BYTES,
            wal_peak_bytes: RUNTIME_WAL_PEAK_BYTES,
            bounded_compaction_peak_bytes: RUNTIME_COMPACTION_PEAK_BYTES,
            checkpoint_peak_bytes: RUNTIME_CHECKPOINT_PEAK_BYTES,
            segment_staging_peak_bytes: SEGMENT_OPERATION_PEAK_BYTES,
            verification_copy_bytes: SEGMENT_OPERATION_PEAK_BYTES,
            replication_retry_bytes: SEGMENT_OPERATION_PEAK_BYTES,
            filesystem_reserve_bytes: absolute_reserve,
            cache_fetch_staging_bytes: source_max_object_bytes,
            cache_eviction_margin_bytes: RUNTIME_CACHE_EVICTION_MARGIN_BYTES,
        },
        ArchiveV2CapacityThresholds {
            hot_warning_bytes: absolute_reserve,
            hot_fatal_bytes: absolute_reserve,
            archive_warning_bytes: absolute_reserve,
            cache_warning_bytes: absolute_reserve,
        },
        ArchiveV2CapacityTotals {
            hot_total_bytes: hot.total_bytes,
            archive_total_bytes: archive.total_bytes,
            cache_total_bytes: cache.total_bytes,
        },
        ArchiveV2AdaptiveReservePolicy {
            reserve_basis_points: CAPACITY_RESERVE_BASIS_POINTS,
            emergency_evidence_reserve_bytes: EVIDENCE_RESERVE_BYTES,
            ..ArchiveV2AdaptiveReservePolicy::default()
        },
    )
    .map_err(|error| error.to_string())
}

#[derive(Debug)]
struct RolePreflightAssessment {
    role: ArchiveV2Role,
    role_config: ArchiveV2RoleConfig,
    admission: ArchiveV2RoleAdmission,
    capability: Option<ArchiveV2CapabilityAdvertisement>,
    capacity: ArchiveV2CapacityDecision,
    identity: ArchiveV2Identity,
    catalog_root: Hash,
    catalog_segments: usize,
    catalog_end_slot: Option<u64>,
    finalized_slot: u64,
    required_archive_end: Option<u64>,
    hot_start: u64,
    complete_hot_window: bool,
    complete_catalog_verified: bool,
    catalog_tip_matches_state: bool,
    every_segment_local: bool,
    independent_consensus_state: bool,
    consensus_wal_and_identity: bool,
    recovery_data_present: bool,
    authenticated_sources: u32,
    source_catalogs_match: bool,
    source_complete_inventories: u32,
    genesis_mossstake_slot_only: bool,
}

const ROLE_PREFLIGHT_VALUES: &[&str] = &[
    "state-dir",
    "cold-store",
    "root",
    "role",
    "recent-history-slots",
    "cache-root",
    "cache-quota-bytes",
    "source-root",
    "source-max-object-bytes",
    "wal",
    "identity-file",
    "recovery-file",
];

fn role_preflight_policy_config(
    role_config: &ArchiveV2RoleConfig,
    allow_local_dev_short_history: bool,
    local_dev_mode: bool,
) -> Result<ArchiveV2RoleConfig, String> {
    if role_config.recent_history_slots >= ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS {
        return Ok(role_config.clone());
    }
    if !allow_local_dev_short_history || !local_dev_mode {
        return Err(format!(
            "--recent-history-slots must be at least {ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS}"
        ));
    }

    // Match the validator's accelerated local-gate admission path: evaluate
    // every semantic requirement against the public-network policy while
    // retaining the explicit short window in the dev-only runtime marker.
    // A production validator still rejects that marker because it does not
    // enter the corresponding --dev-mode admission path.
    let mut policy_config = role_config.clone();
    policy_config.recent_history_slots = ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS;
    Ok(policy_config)
}

fn verify_full_archive_preflight_local_range(
    state: &StateStore,
    start_slot: u64,
    end_slot: u64,
) -> Result<(), String> {
    if end_slot < start_slot {
        return Ok(());
    }
    let mut previous_hash = None;
    for slot in start_slot..=end_slot {
        let block = state.get_block_by_slot(slot)?.ok_or_else(|| {
            format!("canonical block {slot} is missing from local hot/cold storage")
        })?;
        if let Some(expected_parent) = previous_hash {
            if block.header.parent_hash != expected_parent {
                return Err(format!(
                    "canonical block {slot} does not extend local block {}",
                    slot.saturating_sub(1)
                ));
            }
        }
        previous_hash = Some(block.hash());
    }
    Ok(())
}

fn evaluate_role_preflight(
    args: &CommandArgs,
    allow_local_dev_short_history: bool,
    local_dev_mode: bool,
) -> Result<RolePreflightAssessment, String> {
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let root = PathBuf::from(args.required("root")?);
    let role = args
        .required("role")?
        .parse::<ArchiveV2Role>()
        .map_err(|error| error.to_string())?;
    let recent_history_slots = parse_u64(
        args.optional("recent-history-slots")?.unwrap_or("50000"),
        "recent-history-slots",
    )?;
    let cache_root = args.optional("cache-root")?.map(PathBuf::from);
    let cache_quota_bytes = parse_u64(
        args.optional("cache-quota-bytes")?.unwrap_or("0"),
        "cache-quota-bytes",
    )?;
    let source_max_object_bytes = parse_u64(
        args.optional("source-max-object-bytes")?
            .unwrap_or("2147483648"),
        "source-max-object-bytes",
    )?;
    if !(1024..=2 * 1024 * 1024 * 1024).contains(&source_max_object_bytes) {
        return Err("--source-max-object-bytes must be in 1 KiB..=2 GiB".to_string());
    }
    let wal = PathBuf::from(args.required("wal")?);
    let identity_file = PathBuf::from(args.required("identity-file")?);
    let recovery_file = PathBuf::from(args.required("recovery-file")?);
    let source_roots = args
        .repeated("source-root")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    match role {
        ArchiveV2Role::VerifiedCache => {
            if cache_root.is_none() || cache_quota_bytes == 0 || source_roots.is_empty() {
                return Err(
                    "verified-cache preflight requires --cache-root, non-zero --cache-quota-bytes, and at least one --source-root"
                        .to_string(),
                );
            }
        }
        ArchiveV2Role::FullArchive | ArchiveV2Role::Consensus => {
            if cache_root.is_some() || cache_quota_bytes != 0 || !source_roots.is_empty() {
                return Err(format!(
                    "{role} preflight must not configure cache paths, quota, or remote sources"
                ));
            }
        }
    }

    let state_path = fs::canonicalize(&state_dir)
        .map_err(|error| format!("failed resolving state directory: {error}"))?;
    let archive_path = fs::canonicalize(&root)
        .map_err(|error| format!("failed resolving Archive V2 root: {error}"))?;
    let cache_path = cache_root
        .as_deref()
        .map(fs::canonicalize)
        .transpose()
        .map_err(|error| format!("failed resolving Archive V2 cache root: {error}"))?;
    let independent_consensus_state = state_path != archive_path
        && !archive_path.starts_with(&state_path)
        && !state_path.starts_with(&archive_path)
        && cache_path.as_ref().is_none_or(|cache| {
            cache != &state_path
                && !cache.starts_with(&state_path)
                && !state_path.starts_with(cache)
        });
    let canonical_wal = fs::canonicalize(&wal).ok();
    let consensus_wal_and_identity = regular_nonempty_file(&wal)
        && regular_nonempty_file(&identity_file)
        && canonical_wal
            .as_ref()
            .is_some_and(|canonical| canonical.starts_with(&state_path));
    let recovery_data_present = regular_nonempty_file(&recovery_file);

    let mut state = StateStore::open_read_only_with_cache_mb(&state_dir, Some(256))?;
    if let Some(cold) = args.optional("cold-store")? {
        state.open_cold_store_read_only(cold)?;
    }
    let catalog =
        ArchiveV2Catalog::load(&root.join("catalog.av2")).map_err(|error| error.to_string())?;
    let finalized_slot = state.get_last_finalized_slot()?;
    let local_genesis = state
        .get_block_by_slot(0)?
        .ok_or_else(|| "local state has no canonical genesis block".to_string())?;
    if local_genesis.hash() != catalog.identity.genesis_hash {
        return Err("Archive V2 catalog genesis conflicts with local state".to_string());
    }
    let genesis_mossstake_slot_only = genesis_block_declares_mossstake_slot_only(&local_genesis)?;
    let hot_start = finalized_slot.saturating_sub(recent_history_slots.saturating_sub(1));
    // Full-archive migration owns both hot and legacy-cold local storage until
    // signed retirement. Match runtime admission by accepting that physically
    // verified unpublished tail; cache/consensus roles must keep it hot.
    let complete_hot_window = match role {
        ArchiveV2Role::FullArchive => {
            verify_full_archive_preflight_local_range(&state, hot_start, finalized_slot).is_ok()
        }
        ArchiveV2Role::VerifiedCache | ArchiveV2Role::Consensus => state
            .verify_hot_canonical_block_range(hot_start, finalized_slot)
            .is_ok(),
    };
    let required_archive_end = finalized_slot.checked_sub(recent_history_slots);
    let complete_catalog_verified = match required_archive_end {
        Some(end) => catalog
            .covers_genesis_through(end)
            .map_err(|error| error.to_string())?,
        None => {
            catalog.entries.is_empty()
                || catalog
                    .entries
                    .first()
                    .is_some_and(|entry| entry.manifest.start_slot == 0)
        }
    };
    let catalog_tip_matches_state = match catalog.entries.last() {
        Some(entry) => state
            .get_block_by_slot(entry.manifest.end_slot)?
            .is_some_and(|block| block.hash() == entry.manifest.last_block_hash),
        None => true,
    };
    let every_segment_local = complete_catalog_inventory(&root, &catalog, 2 * 1024 * 1024 * 1024);

    let mut authenticated_sources = 0u32;
    let mut source_catalogs_match = true;
    let mut source_complete_inventories = 0u32;
    let mut unique_sources = BTreeSet::new();
    for source in &source_roots {
        let canonical = fs::canonicalize(source)
            .map_err(|error| format!("failed resolving source {}: {error}", source.display()))?;
        if canonical == archive_path
            || canonical == state_path
            || cache_path.as_ref().is_some_and(|cache| cache == &canonical)
        {
            return Err(format!(
                "Archive V2 source {} is not independent from state, archive, or cache storage",
                source.display()
            ));
        }
        if !unique_sources.insert(canonical) {
            return Err("Archive V2 source roots must be unique".to_string());
        }
        let source_catalog = ArchiveV2Catalog::load(&source.join("catalog.av2"))
            .map_err(|error| format!("source {} catalog failed: {error}", source.display()))?;
        if source_catalog.identity != catalog.identity
            || source_catalog.catalog_root != catalog.catalog_root
        {
            source_catalogs_match = false;
        } else {
            let complete_inventory =
                complete_catalog_inventory(source, &catalog, source_max_object_bytes);
            if complete_inventory {
                authenticated_sources = authenticated_sources.saturating_add(1);
                source_complete_inventories = source_complete_inventories.saturating_add(1);
            } else {
                source_catalogs_match = false;
            }
        }
    }

    let capacity = runtime_role_capacity_decision(
        role,
        &state_dir,
        &root,
        cache_root.as_deref(),
        source_max_object_bytes,
        &catalog.identity.network_id,
    )?;
    let role_config = ArchiveV2RoleConfig {
        version: ARCHIVE_V2_ROLE_CONFIG_VERSION,
        role,
        recent_history_slots,
        verified_cache_quota_bytes: cache_quota_bytes,
        advertise_deep_history: role != ArchiveV2Role::Consensus,
    };
    let policy_config =
        role_preflight_policy_config(&role_config, allow_local_dev_short_history, local_dev_mode)?;
    let requirements = ArchiveV2RoleRequirements {
        independent_consensus_state,
        consensus_wal_and_identity,
        recovery_data_present,
        complete_catalog_verified: complete_catalog_verified
            && catalog_tip_matches_state
            && complete_hot_window
            && source_catalogs_match,
        every_segment_local,
        authenticated_remote_sources: authenticated_sources,
        cache_staging_headroom_bytes: cache_path
            .as_deref()
            .map(cli_filesystem_capacity)
            .transpose()?
            .map(|capacity| capacity.available_bytes)
            .unwrap_or(0),
        network_archive_policy_satisfied: false,
        no_archive_operation_in_progress: true,
    };
    let admission = policy_config
        .admit(&requirements)
        .map_err(|error| error.to_string())?;
    let catalog_range = catalog
        .entries
        .first()
        .zip(catalog.entries.last())
        .map(|(first, last)| (first.manifest.start_slot, last.manifest.end_slot));
    let capability = if admission.admitted {
        Some(
            role_config
                .capability(
                    catalog.identity.clone(),
                    catalog.catalog_root,
                    catalog_range,
                    &admission,
                )
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    Ok(RolePreflightAssessment {
        role,
        role_config,
        admission,
        capability,
        capacity,
        identity: catalog.identity,
        catalog_root: catalog.catalog_root,
        catalog_segments: catalog.entries.len(),
        catalog_end_slot: catalog.entries.last().map(|entry| entry.manifest.end_slot),
        finalized_slot,
        required_archive_end,
        hot_start,
        complete_hot_window,
        complete_catalog_verified,
        catalog_tip_matches_state,
        every_segment_local,
        independent_consensus_state,
        consensus_wal_and_identity,
        recovery_data_present,
        authenticated_sources,
        source_catalogs_match,
        source_complete_inventories,
        genesis_mossstake_slot_only,
    })
}

struct RolePreflightReport<'a> {
    operation: &'a str,
    runtime_admitted: bool,
    bootstrap_authorized: Option<bool>,
    marker_path: Option<&'a Path>,
    marker_created: Option<bool>,
    state_admission_persisted: Option<bool>,
    state_admission_created: Option<bool>,
    dry_run: bool,
}

fn print_role_preflight_assessment(
    assessment: &RolePreflightAssessment,
    report: RolePreflightReport<'_>,
) -> Result<(), String> {
    print_json(&json!({
        "operation": report.operation,
        "role": assessment.role,
        "admitted": report.runtime_admitted,
        "runtime_admitted": report.runtime_admitted,
        "bootstrap_authorized": report.bootstrap_authorized,
        "role_admission": assessment.admission,
        "capacity": assessment.capacity,
        "network_id": assessment.identity.network_id,
        "genesis_hash": assessment.identity.genesis_hash.to_hex(),
        "catalog_root": assessment.catalog_root.to_hex(),
        "catalog_segments": assessment.catalog_segments,
        "catalog_end_slot": assessment.catalog_end_slot,
        "finalized_slot": assessment.finalized_slot,
        "required_archive_end": assessment.required_archive_end,
        "hot_start_slot": assessment.hot_start,
        "complete_hot_window": assessment.complete_hot_window,
        "complete_catalog_verified": assessment.complete_catalog_verified,
        "catalog_tip_matches_state": assessment.catalog_tip_matches_state,
        "every_segment_local": assessment.every_segment_local,
        "independent_consensus_state": assessment.independent_consensus_state,
        "consensus_wal_and_identity": assessment.consensus_wal_and_identity,
        "recovery_data_present": assessment.recovery_data_present,
        "authenticated_source_catalogs": assessment.authenticated_sources,
        "source_catalogs_match": assessment.source_catalogs_match,
        "source_complete_inventories": assessment.source_complete_inventories,
        "genesis_mossstake_slot_only": assessment.genesis_mossstake_slot_only,
        "marker_path": report.marker_path,
        "marker_created": report.marker_created,
        "state_admission_persisted": report.state_admission_persisted,
        "state_admission_created": report.state_admission_created,
        "dry_run": report.dry_run,
    }))
}

/// Read-only, fail-closed admission report for the exact runtime Archive V2
/// role boundary. Deployment automation must run this against the same paths
/// and source roots that it will place in the validator service configuration.
fn run_role_preflight(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(ROLE_PREFLIGHT_VALUES, &[])?;
    let assessment = evaluate_role_preflight(args, false, false)?;
    let admitted = assessment.admission.admitted
        && assessment.capacity.action == ArchiveV2PressureAction::Normal;
    print_role_preflight_assessment(
        &assessment,
        RolePreflightReport {
            operation: "role_preflight",
            runtime_admitted: admitted,
            bootstrap_authorized: None,
            marker_path: None,
            marker_created: None,
            state_admission_persisted: None,
            state_admission_created: None,
            dry_run: false,
        },
    )?;
    if !admitted {
        return Err(
            "Archive V2 role preflight did not reach an admitted Normal-capacity state".to_string(),
        );
    }
    Ok(())
}

/// Creates the exact runtime role marker and state-bound admission fingerprint
/// needed to retire legacy cold history while a validator is stopped. This
/// command permits a non-Normal capacity result only for the circular low-space
/// transition; it never overrides runtime capacity admission and never weakens
/// the network's absolute mutable-storage floor.
fn run_role_bootstrap(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        ROLE_PREFLIGHT_VALUES,
        &[
            "acknowledge-stopped-validator",
            "acknowledge-low-space-legacy-retirement",
            "allow-local-dev-short-history",
            "dry-run",
        ],
    )?;
    if !args.flag("acknowledge-stopped-validator")
        || !args.flag("acknowledge-low-space-legacy-retirement")
    {
        return Err(
            "role bootstrap requires --acknowledge-stopped-validator and --acknowledge-low-space-legacy-retirement"
                .to_string(),
        );
    }
    if args.optional("cold-store")?.is_none() {
        return Err("role bootstrap requires --cold-store to prove canonical slot 0".to_string());
    }

    let allow_local_dev_short_history = args.flag("allow-local-dev-short-history");
    let local_dev_mode = std::env::var("LICHEN_LOCAL_DEV").ok().as_deref() == Some("1");
    let assessment = evaluate_role_preflight(args, allow_local_dev_short_history, local_dev_mode)?;
    if !assessment.admission.admitted {
        print_role_preflight_assessment(
            &assessment,
            RolePreflightReport {
                operation: "role_bootstrap",
                runtime_admitted: false,
                bootstrap_authorized: Some(false),
                marker_path: None,
                marker_created: None,
                state_admission_persisted: None,
                state_admission_created: None,
                dry_run: args.flag("dry-run"),
            },
        )?;
        return Err("Archive V2 role bootstrap semantic admission failed".to_string());
    }
    if assessment.capacity.hot_available_bytes < assessment.capacity.absolute_reserve_bytes {
        print_role_preflight_assessment(
            &assessment,
            RolePreflightReport {
                operation: "role_bootstrap",
                runtime_admitted: false,
                bootstrap_authorized: Some(false),
                marker_path: None,
                marker_created: None,
                state_admission_persisted: None,
                state_admission_created: None,
                dry_run: args.flag("dry-run"),
            },
        )?;
        return Err(format!(
            "Archive V2 role bootstrap refuses to cross the {} byte network storage floor",
            assessment.capacity.absolute_reserve_bytes
        ));
    }

    let capability = assessment
        .capability
        .as_ref()
        .ok_or_else(|| "Archive V2 role bootstrap has no admitted capability".to_string())?;
    let state_admission_fingerprint = archive_v2_state_admission_fingerprint(capability)?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let existing_state_admission = {
        let state = StateStore::open_read_only_with_cache_mb(&state_dir, Some(64))?;
        state.get_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY)?
    };
    let mut state_admission_persisted = match existing_state_admission.as_deref() {
        None => false,
        Some(stored) if stored == state_admission_fingerprint.0.as_slice() => true,
        Some(stored) if stored.len() != 32 => {
            return Err("existing Archive V2 state admission marker is malformed".to_string());
        }
        Some(_) => {
            return Err(
                "existing Archive V2 state admission marker conflicts with the verified bootstrap authorization"
                    .to_string(),
            );
        }
    };

    let marker = ArchiveV2RoleMarker {
        marker_version: 1,
        identity: assessment.identity.clone(),
        role_config: assessment.role_config.clone(),
        genesis_mossstake_slot_only: assessment.genesis_mossstake_slot_only,
    };
    let marker_path = PathBuf::from(args.required("root")?).join(ARCHIVE_V2_ROLE_MARKER_FILENAME);
    let mut marker_created = false;
    let mut state_admission_created = false;
    match fs::symlink_metadata(&marker_path) {
        Ok(_) => {
            let existing = load_archive_v2_role_marker(&marker_path)?;
            if existing != marker {
                return Err(
                    "existing Archive V2 role marker conflicts with the verified bootstrap authorization"
                        .to_string(),
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !args.flag("dry-run") {
                store_archive_v2_role_marker_create_new(&marker_path, &marker)?;
                if load_archive_v2_role_marker(&marker_path)? != marker {
                    return Err(
                        "published Archive V2 role marker failed read-back verification"
                            .to_string(),
                    );
                }
                marker_created = true;
            }
        }
        Err(error) => {
            return Err(format!(
                "failed inspecting Archive V2 role marker {}: {error}",
                marker_path.display()
            ));
        }
    }
    if !args.flag("dry-run") && !state_admission_persisted {
        let state = StateStore::open_with_cache_mb(&state_dir, Some(64))?;
        match state.get_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY)? {
            None => {
                state.put_metadata(
                    ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY,
                    &state_admission_fingerprint.0,
                )?;
                state.sync_hot_wal()?;
                state_admission_created = true;
            }
            Some(stored) if stored.as_slice() == state_admission_fingerprint.0.as_slice() => {}
            Some(stored) if stored.len() != 32 => {
                return Err(
                    "Archive V2 state admission marker became malformed during bootstrap"
                        .to_string(),
                );
            }
            Some(_) => {
                return Err(
                    "Archive V2 state admission marker changed during stopped-validator bootstrap"
                        .to_string(),
                );
            }
        }
        let stored = state
            .get_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY)?
            .ok_or_else(|| {
                "published Archive V2 state admission marker disappeared before read-back"
                    .to_string()
            })?;
        if stored.as_slice() != state_admission_fingerprint.0.as_slice() {
            return Err(
                "published Archive V2 state admission marker failed read-back verification"
                    .to_string(),
            );
        }
        state_admission_persisted = true;
    }
    print_role_preflight_assessment(
        &assessment,
        RolePreflightReport {
            operation: "role_bootstrap",
            runtime_admitted: assessment.capacity.action == ArchiveV2PressureAction::Normal,
            bootstrap_authorized: Some(true),
            marker_path: Some(&marker_path),
            marker_created: Some(marker_created),
            state_admission_persisted: Some(state_admission_persisted),
            state_admission_created: Some(state_admission_created),
            dry_run: args.flag("dry-run"),
        },
    )
}

fn checkpoint_sst_symlink_bytes(root: &Path) -> Result<(u64, u64), String> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    for entry in fs::read_dir(root).map_err(|error| {
        format!(
            "failed reading checkpoint source {}: {error}",
            root.display()
        )
    })? {
        let entry = entry.map_err(|error| format!("failed reading checkpoint entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("sst") {
            continue;
        }
        let link_metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed inspecting {}: {error}", path.display()))?;
        if !link_metadata.file_type().is_symlink() {
            continue;
        }
        let target_metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "checkpoint source SST symlink {} has no readable target: {error}",
                path.display()
            )
        })?;
        if !target_metadata.is_file() || target_metadata.len() == 0 {
            return Err(format!(
                "checkpoint source SST symlink {} does not resolve to a non-empty regular file",
                path.display()
            ));
        }
        count = count.saturating_add(1);
        bytes = bytes
            .checked_add(target_metadata.len())
            .ok_or_else(|| "checkpoint materialization size overflow".to_string())?;
    }
    Ok((count, bytes))
}

fn materialize_checkpoint_sst_symlinks(
    root: &Path,
    maximum_bytes: u64,
) -> Result<(u64, u64), String> {
    let (expected_count, expected_bytes) = checkpoint_sst_symlink_bytes(root)?;
    if expected_bytes > maximum_bytes {
        return Err(format!(
            "checkpoint SST materialization requires {expected_bytes} bytes, exceeding the {maximum_bytes}-byte bound"
        ));
    }
    let mut materialized_count = 0u64;
    let mut materialized_bytes = 0u64;
    let paths = fs::read_dir(root)
        .map_err(|error| format!("failed reading checkpoint {}: {error}", root.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed reading checkpoint entry: {error}"))?;
    for path in paths {
        if path.extension().and_then(|value| value.to_str()) != Some("sst")
            || !fs::symlink_metadata(&path)
                .map_err(|error| format!("failed inspecting {}: {error}", path.display()))?
                .file_type()
                .is_symlink()
        {
            continue;
        }
        let target_metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "checkpoint SST symlink {} became unreadable: {error}",
                path.display()
            )
        })?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "checkpoint SST filename is not UTF-8".to_string())?;
        let temporary = root.join(format!(".{file_name}.materialize.next"));
        if temporary.exists() || fs::symlink_metadata(&temporary).is_ok() {
            return Err(format!(
                "checkpoint materialization target already exists: {}",
                temporary.display()
            ));
        }
        let mut source = fs::File::open(&path)
            .map_err(|error| format!("failed opening {}: {error}", path.display()))?;
        let mut destination = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("failed creating {}: {error}", temporary.display()))?;
        let copied = std::io::copy(&mut source, &mut destination)
            .map_err(|error| format!("failed materializing {}: {error}", path.display()))?;
        if copied != target_metadata.len() {
            return Err(format!(
                "checkpoint SST {} changed size while being materialized",
                path.display()
            ));
        }
        destination
            .set_permissions(target_metadata.permissions())
            .map_err(|error| {
                format!(
                    "failed setting {} permissions: {error}",
                    temporary.display()
                )
            })?;
        destination
            .sync_all()
            .map_err(|error| format!("failed syncing {}: {error}", temporary.display()))?;
        drop(destination);
        fs::rename(&temporary, &path).map_err(|error| {
            format!(
                "failed atomically replacing checkpoint symlink {}: {error}",
                path.display()
            )
        })?;
        materialized_count = materialized_count.saturating_add(1);
        materialized_bytes = materialized_bytes.saturating_add(copied);
    }
    OpenOptions::new()
        .read(true)
        .open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("failed syncing checkpoint directory: {error}"))?;
    let (remaining_count, _) = checkpoint_sst_symlink_bytes(root)?;
    if remaining_count != 0
        || materialized_count != expected_count
        || materialized_bytes != expected_bytes
    {
        return Err(
            "checkpoint SST materialization did not reach the expected inventory".to_string(),
        );
    }
    Ok((materialized_count, materialized_bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HotSnapshotSourcePlan {
    symlink_sst_count: u64,
    symlink_sst_bytes: u64,
    copied_file_count: u64,
    copied_file_bytes: u64,
    hardlinked_sst_count: u64,
}

fn hot_snapshot_source_plan(root: &Path) -> Result<HotSnapshotSourcePlan, String> {
    let mut plan = HotSnapshotSourcePlan {
        symlink_sst_count: 0,
        symlink_sst_bytes: 0,
        copied_file_count: 0,
        copied_file_bytes: 0,
        hardlinked_sst_count: 0,
    };
    for entry in fs::read_dir(root)
        .map_err(|error| format!("failed reading snapshot source {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("failed reading snapshot entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed inspecting {}: {error}", path.display()))?;
        if metadata.is_dir() {
            continue;
        }
        if metadata.file_type().is_symlink() {
            if path.extension().and_then(|value| value.to_str()) != Some("sst") {
                return Err(format!(
                    "snapshot source contains unsupported non-SST symlink {}",
                    path.display()
                ));
            }
            let target = fs::metadata(&path).map_err(|error| {
                format!(
                    "snapshot source SST symlink {} has no readable target: {error}",
                    path.display()
                )
            })?;
            if !target.is_file() || target.len() == 0 {
                return Err(format!(
                    "snapshot source SST symlink {} does not resolve to a non-empty regular file",
                    path.display()
                ));
            }
            plan.symlink_sst_count = plan.symlink_sst_count.saturating_add(1);
            plan.symlink_sst_bytes = plan
                .symlink_sst_bytes
                .checked_add(target.len())
                .ok_or_else(|| "snapshot source symlink size overflow".to_string())?;
        } else if metadata.is_file() {
            if path.extension().and_then(|value| value.to_str()) == Some("sst") {
                if metadata.len() == 0 {
                    return Err(format!("snapshot source SST {} is empty", path.display()));
                }
                plan.hardlinked_sst_count = plan.hardlinked_sst_count.saturating_add(1);
            } else {
                plan.copied_file_count = plan.copied_file_count.saturating_add(1);
                plan.copied_file_bytes = plan
                    .copied_file_bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| "snapshot source copied-file size overflow".to_string())?;
            }
        } else {
            return Err(format!(
                "snapshot source contains unsupported filesystem entry {}",
                path.display()
            ));
        }
    }
    Ok(plan)
}

fn copy_snapshot_file(
    source: &Path,
    destination: &Path,
    expected_bytes: u64,
) -> Result<(), String> {
    let mut source_file = fs::File::open(source)
        .map_err(|error| format!("failed opening {}: {error}", source.display()))?;
    let mut destination_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| format!("failed creating {}: {error}", destination.display()))?;
    let copied = std::io::copy(&mut source_file, &mut destination_file)
        .map_err(|error| format!("failed copying {}: {error}", source.display()))?;
    if copied != expected_bytes {
        return Err(format!(
            "snapshot source {} changed size while being copied",
            source.display()
        ));
    }
    destination_file
        .set_permissions(
            fs::metadata(source)
                .map_err(|error| format!("failed reinspecting {}: {error}", source.display()))?
                .permissions(),
        )
        .map_err(|error| {
            format!(
                "failed setting permissions on {}: {error}",
                destination.display()
            )
        })?;
    destination_file
        .sync_all()
        .map_err(|error| format!("failed syncing {}: {error}", destination.display()))
}

fn stage_hot_snapshot_source(
    source_root: &Path,
    staging_root: &Path,
    expected: HotSnapshotSourcePlan,
) -> Result<HotSnapshotSourcePlan, String> {
    if staging_root.exists() || fs::symlink_metadata(staging_root).is_ok() {
        return Err(format!(
            "refusing to overwrite snapshot staging source {}",
            staging_root.display()
        ));
    }
    fs::create_dir(staging_root).map_err(|error| {
        format!(
            "failed creating snapshot staging source {}: {error}",
            staging_root.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staging_root, fs::Permissions::from_mode(0o700)).map_err(|error| {
            format!(
                "failed protecting snapshot staging source {}: {error}",
                staging_root.display()
            )
        })?;
    }

    let mut entries = fs::read_dir(source_root)
        .map_err(|error| {
            format!(
                "failed reading snapshot source {}: {error}",
                source_root.display()
            )
        })?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed reading snapshot entry: {error}"))?;
    entries.sort();
    for source in entries {
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| format!("failed inspecting {}: {error}", source.display()))?;
        if metadata.is_dir() {
            continue;
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| format!("snapshot source has no filename: {}", source.display()))?;
        let destination = staging_root.join(file_name);
        if metadata.file_type().is_symlink() {
            let target = fs::metadata(&source).map_err(|error| {
                format!(
                    "snapshot source SST symlink {} became unreadable: {error}",
                    source.display()
                )
            })?;
            copy_snapshot_file(&source, &destination, target.len())?;
        } else if metadata.is_file()
            && source.extension().and_then(|value| value.to_str()) == Some("sst")
        {
            fs::hard_link(&source, &destination).map_err(|error| {
                format!(
                    "failed hard-linking immutable SST {} into snapshot staging: {error}",
                    source.display()
                )
            })?;
        } else if metadata.is_file() {
            copy_snapshot_file(&source, &destination, metadata.len())?;
        } else {
            return Err(format!(
                "snapshot source contains unsupported filesystem entry {}",
                source.display()
            ));
        }
    }
    OpenOptions::new()
        .read(true)
        .open(staging_root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("failed syncing snapshot staging source: {error}"))?;
    let actual = hot_snapshot_source_plan(staging_root)?;
    let normalized = HotSnapshotSourcePlan {
        symlink_sst_count: expected.symlink_sst_count,
        symlink_sst_bytes: expected.symlink_sst_bytes,
        copied_file_count: actual.copied_file_count,
        copied_file_bytes: actual.copied_file_bytes,
        hardlinked_sst_count: actual
            .hardlinked_sst_count
            .saturating_sub(expected.symlink_sst_count),
    };
    if normalized != expected {
        return Err("snapshot staging source inventory changed during creation".to_string());
    }
    Ok(expected)
}

/// Create a self-contained hot RocksDB checkpoint for bounded Archive V2
/// building. The caller must stop the validator before invoking this command.
/// The live RocksDB is never opened: immutable regular SSTs are hard-linked
/// into an isolated staging source, SST symlinks and all mutable files are
/// copied, and RocksDB recovery/checkpoint writes can affect only that staging
/// directory. This also prevents a privileged caller from changing live-file
/// ownership while opening the database.
fn run_snapshot_hot(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "output",
            "max-materialized-bytes",
            "minimum-remaining-bytes",
        ],
        &[],
    )?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let output = PathBuf::from(args.required("output")?);
    let maximum_bytes = parse_u64(
        args.required("max-materialized-bytes")?,
        "max-materialized-bytes",
    )?;
    let minimum_remaining_bytes = parse_u64(
        args.required("minimum-remaining-bytes")?,
        "minimum-remaining-bytes",
    )?;
    if maximum_bytes == 0 || minimum_remaining_bytes == 0 {
        return Err(
            "snapshot materialization and remaining-space bounds must be non-zero".to_string(),
        );
    }
    if output.exists() || fs::symlink_metadata(&output).is_ok() {
        return Err(format!(
            "refusing to overwrite checkpoint target {}",
            output.display()
        ));
    }
    let state_path = fs::canonicalize(&state_dir)
        .map_err(|error| format!("failed resolving state directory: {error}"))?;
    let output_parent = output
        .parent()
        .ok_or_else(|| "snapshot output has no parent".to_string())?;
    fs::create_dir_all(output_parent)
        .map_err(|error| format!("failed creating snapshot parent: {error}"))?;
    let output_parent = fs::canonicalize(output_parent)
        .map_err(|error| format!("failed resolving snapshot parent: {error}"))?;
    if output_parent.starts_with(&state_path) || state_path.starts_with(&output_parent) {
        return Err("snapshot output and live state directories must not overlap".to_string());
    }
    let source_plan = hot_snapshot_source_plan(&state_dir)?;
    let staging_copied_bytes = source_plan
        .symlink_sst_bytes
        .checked_add(source_plan.copied_file_bytes)
        .ok_or_else(|| "snapshot staging copy size overflow".to_string())?;
    if staging_copied_bytes > maximum_bytes {
        return Err(format!(
            "snapshot staging needs {staging_copied_bytes} copied bytes, exceeding the {maximum_bytes}-byte bound"
        ));
    }
    let available = cli_filesystem_capacity(&output_parent)?.available_bytes;
    let required = maximum_bytes.saturating_add(minimum_remaining_bytes);
    if available < required {
        return Err(format!(
            "snapshot filesystem has {available} bytes available but needs {required} bytes"
        ));
    }

    let output_name = output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "snapshot output filename is not UTF-8".to_string())?;
    let staging_source = output_parent.join(format!(".{output_name}.source.next"));
    let checkpoint_next = output_parent.join(format!(".{output_name}.checkpoint.next"));
    for temporary in [&staging_source, &checkpoint_next] {
        if temporary.exists() || fs::symlink_metadata(temporary).is_ok() {
            return Err(format!(
                "snapshot temporary path already exists: {}",
                temporary.display()
            ));
        }
    }
    stage_hot_snapshot_source(&state_dir, &staging_source, source_plan)?;
    let state = StateStore::open_with_cache_mb(&staging_source, Some(256))?;
    state.create_hot_raw_checkpoint(
        checkpoint_next
            .to_str()
            .ok_or_else(|| "snapshot output path is not UTF-8".to_string())?,
    )?;
    drop(state);
    let (materialized_count, materialized_bytes) =
        materialize_checkpoint_sst_symlinks(&checkpoint_next, maximum_bytes)?;
    let remaining = cli_filesystem_capacity(&checkpoint_next)?.available_bytes;
    if remaining < minimum_remaining_bytes {
        return Err(format!(
            "snapshot completed with {remaining} bytes, below the {minimum_remaining_bytes}-byte required reserve"
        ));
    }
    fs::remove_dir_all(&staging_source).map_err(|error| {
        format!(
            "failed removing isolated snapshot staging source {}: {error}",
            staging_source.display()
        )
    })?;
    fs::rename(&checkpoint_next, &output).map_err(|error| {
        format!(
            "failed atomically publishing snapshot {}: {error}",
            output.display()
        )
    })?;
    OpenOptions::new()
        .read(true)
        .open(&output_parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("failed syncing snapshot parent: {error}"))?;
    print_json(&json!({
        "operation": "snapshot_hot",
        "state_dir": state_dir,
        "output": output,
        "source_sst_symlink_count": source_plan.symlink_sst_count,
        "source_sst_symlink_bytes": source_plan.symlink_sst_bytes,
        "staging_copied_file_count": source_plan.copied_file_count + source_plan.symlink_sst_count,
        "staging_copied_bytes": staging_copied_bytes,
        "staging_hardlinked_sst_count": source_plan.hardlinked_sst_count,
        "materialized_sst_count": materialized_count,
        "materialized_sst_bytes": materialized_bytes,
        "remaining_bytes": remaining,
    }))
}

fn run_declare_legacy_loss(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(&["root"], &[])?;
    let root = PathBuf::from(args.required("root")?);
    let catalog_path = root.join("catalog.av2");
    let mut catalog = ArchiveV2Catalog::load(&catalog_path).map_err(|error| error.to_string())?;
    let declaration = ArchiveV2LegacyLossDeclaration::lichen_testnet_1();
    if catalog.legacy_loss_declarations.contains(&declaration) {
        return Err(
            "exact lichen-testnet-1 legacy-loss declaration is already cataloged".to_string(),
        );
    }
    catalog
        .declare_legacy_loss(declaration.clone())
        .map_err(|error| error.to_string())?;
    catalog
        .store_atomic(&catalog_path)
        .map_err(|error| error.to_string())?;
    print_json(&json!({
        "operation": "declare-legacy-loss",
        "root": root,
        "catalog_root": catalog.catalog_root.to_hex(),
        "declaration": declaration,
    }))
}

fn run_retirement_authorize(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "cold-store",
            "root",
            "segment-object-hash",
            "start-slot",
            "end-slot",
            "replica-evidence",
            "required-replicas",
            "required-failure-domains",
            "release-tag",
            "release-commit",
            "artifact-sha256",
            "pq-signature-sha256",
            "deployed-validator-count",
            "activated-unix-seconds",
            "authorized-unix-seconds",
            "signer-keypair",
            "output",
        ],
        &[],
    )?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let root = PathBuf::from(args.required("root")?);
    let mut state = StateStore::open_read_only_with_cache_mb(&state_dir, Some(256))?;
    if let Some(cold) = args.optional("cold-store")? {
        state.open_cold_store_read_only(cold)?;
    }
    attach_retirement_reader(&state, &root)?;
    let segment_object_hash = Hash::from_hex(args.required("segment-object-hash")?)?;
    let slot_window = parse_optional_retirement_window(args)?;
    let replica_evidence =
        parse_retirement_replica_evidence(args.repeated("replica-evidence"), segment_object_hash)?;
    let required_replica_count =
        parse_u16(args.required("required-replicas")?, "required-replicas")?;
    let required_failure_domains = parse_u16(
        args.required("required-failure-domains")?,
        "required-failure-domains",
    )?;
    let rollback_anchor = ArchiveV2RollbackAnchor {
        release_tag: args.required("release-tag")?.to_string(),
        release_commit: args.required("release-commit")?.to_string(),
        artifact_sha256: Hash::from_hex(args.required("artifact-sha256")?)?,
        detached_pq_checksum_signature_sha256: Hash::from_hex(
            args.required("pq-signature-sha256")?,
        )?,
        archive_format_version: ARCHIVE_V2_FORMAT_VERSION,
        catalog_format_version: ARCHIVE_V2_CATALOG_VERSION,
        deployed_validator_count: parse_u16(
            args.required("deployed-validator-count")?,
            "deployed-validator-count",
        )?,
        activated_unix_seconds: parse_u64(
            args.required("activated-unix-seconds")?,
            "activated-unix-seconds",
        )?,
    };
    let authorized_unix_seconds = parse_u64(
        args.required("authorized-unix-seconds")?,
        "authorized-unix-seconds",
    )?;
    let request = match slot_window {
        Some((start_slot, end_slot)) => state.prepare_archive_v2_retirement_window_request(
            segment_object_hash,
            start_slot,
            end_slot,
            replica_evidence,
            required_replica_count,
            required_failure_domains,
            rollback_anchor,
            authorized_unix_seconds,
        )?,
        None => state.prepare_archive_v2_retirement_request(
            segment_object_hash,
            replica_evidence,
            required_replica_count,
            required_failure_domains,
            rollback_anchor,
            authorized_unix_seconds,
        )?,
    };
    let password = keypair_password_from_env();
    let signer = KeypairFile::load_with_password_policy(
        Path::new(args.required("signer-keypair")?),
        password.as_deref(),
        plaintext_keypair_allowed_for_local_dev(),
    )?
    .to_keypair()?;
    let retirement =
        ArchiveV2RetirementManifest::sign(request, &signer).map_err(|error| error.to_string())?;
    let encoded = retirement
        .encode_canonical()
        .map_err(|error| error.to_string())?;
    let output = PathBuf::from(args.required("output")?);
    write_bytes_create_new(&output, &encoded)?;
    print_json(&json!({
        "operation": "retirement-authorize",
        "output": output,
        "retirement_manifest_sha256": Hash::hash(&encoded).to_hex(),
        "catalog_root": retirement.catalog_root().to_hex(),
        "segment_object_hash": retirement.segment_object_hash().to_hex(),
        "slot_range": [retirement.slot_range().0, retirement.slot_range().1],
        "signer": retirement.signer().to_string(),
    }))
}

fn run_retirement_pass(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "cold-store",
            "root",
            "retirement-manifest",
            "journal",
            "max-rows",
            "max-bytes",
            "max-wall-time-ms",
            "max-passes-per-open",
        ],
        &[
            "acknowledge-stopped-validator",
            "acknowledge-v2-rollback-only",
        ],
    )?;
    require_retirement_acknowledgements(args)?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let root = PathBuf::from(args.required("root")?);
    let mut state = StateStore::open_with_cache_mb(&state_dir, Some(256))?;
    if let Some(cold) = args.optional("cold-store")? {
        state.open_cold_store(cold)?;
    }
    attach_retirement_reader(&state, &root)?;
    let retirement = read_retirement_manifest(Path::new(args.required("retirement-manifest")?))?;
    let limits = ArchiveV2RetirementLimits {
        max_rows: parse_u64(args.optional("max-rows")?.unwrap_or("2000"), "max-rows")?,
        max_bytes: parse_u64(
            args.optional("max-bytes")?.unwrap_or("67108864"),
            "max-bytes",
        )?,
        max_wall_time: Duration::from_millis(parse_u64(
            args.optional("max-wall-time-ms")?.unwrap_or("2000"),
            "max-wall-time-ms",
        )?),
    };
    let max_passes =
        parse_retirement_passes_per_open(args.optional("max-passes-per-open")?.unwrap_or("1"))?;
    let journal = PathBuf::from(args.required("journal")?);
    let (report, passes_completed) = run_retirement_passes_per_open(max_passes, || {
        state.retire_archive_v2_segment_pass(&retirement, &journal, limits)
    })?;
    print_json(&json!({
        "operation": "retirement-pass",
        "journal": journal,
        "phase": report.phase,
        "category": report.category,
        "scanned_rows": report.scanned_rows,
        "skipped_absent_rebuildable_rows": report.skipped_absent_rebuildable_rows,
        "deleted_hot_rows": report.deleted_hot_rows,
        "deleted_cold_rows": report.deleted_cold_rows,
        "deleted_logical_bytes": report.deleted_logical_bytes,
        "recovered_pending_batch": report.recovered_pending_batch,
        "categories_completed": report.categories_completed,
        "elapsed_millis": report.elapsed_millis,
        "passes_completed": passes_completed,
        "tombstoning_complete": report.phase == ArchiveV2RetirementPhase::ReclaimPending,
    }))
}

fn run_retirement_reclaim(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "cold-store",
            "root",
            "retirement-manifest",
            "journal",
            "max-ranges",
            "max-estimated-input-bytes",
            "reserve-bytes",
        ],
        &[
            "acknowledge-stopped-validator",
            "acknowledge-v2-rollback-only",
        ],
    )?;
    require_retirement_acknowledgements(args)?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let cold_store = args.optional("cold-store")?.map(PathBuf::from);
    let root = PathBuf::from(args.required("root")?);
    let retirement = read_retirement_manifest(Path::new(args.required("retirement-manifest")?))?;
    let default_reserve = if retirement.identity().network_id == "lichen-testnet-1" {
        TESTNET_CAPACITY_FLOOR_BYTES
    } else {
        DEFAULT_CAPACITY_FLOOR_BYTES
    };
    let reserve_bytes = match args.optional("reserve-bytes")? {
        Some(value) => parse_u64(value, "reserve-bytes")?,
        None => default_reserve,
    };
    if reserve_bytes < default_reserve {
        return Err(format!(
            "--reserve-bytes cannot be below the {} byte network floor",
            default_reserve
        ));
    }
    let hot_capacity = cli_filesystem_capacity(&capacity_probe_path(&state_dir)?)?;
    let cold_capacity = match cold_store.as_deref() {
        Some(cold) => cli_filesystem_capacity(&capacity_probe_path(cold)?)?,
        None => hot_capacity,
    };
    let mut state = StateStore::open_with_cache_mb(&state_dir, Some(256))?;
    if let Some(cold) = cold_store.as_deref() {
        state.open_cold_store(cold)?;
    }
    attach_retirement_reader(&state, &root)?;
    let limits = ArchiveV2RetirementReclaimLimits {
        max_ranges: parse_u64(args.optional("max-ranges")?.unwrap_or("1"), "max-ranges")?,
        max_estimated_input_bytes: parse_u64(
            args.optional("max-estimated-input-bytes")?
                .unwrap_or("67108864"),
            "max-estimated-input-bytes",
        )?,
        hot_available_bytes: hot_capacity.available_bytes,
        hot_required_reserve_bytes: reserve_bytes,
        cold_available_bytes: cold_capacity.available_bytes,
        cold_required_reserve_bytes: reserve_bytes,
    };
    let journal = PathBuf::from(args.required("journal")?);
    let report = state.reclaim_archive_v2_retirement_pass(&retirement, &journal, limits)?;
    print_json(&json!({
        "operation": "retirement-reclaim",
        "journal": journal,
        "phase": report.phase,
        "queued_ranges_before": report.queued_ranges_before,
        "queued_ranges_after": report.queued_ranges_after,
        "compacted_ranges": report.compacted_ranges,
        "split_ranges": report.split_ranges,
        "estimated_input_bytes": report.estimated_input_bytes,
        "reclaimed_physical_bytes": report.reclaimed_physical_bytes,
        "total_reclaimed_physical_bytes": report.total_reclaimed_physical_bytes,
        "compaction_duration_millis": report.compaction_duration_millis,
        "paused_reason": report.paused_reason,
        "complete": report.phase == ArchiveV2RetirementPhase::Complete,
    }))
}

fn attach_retirement_reader(state: &StateStore, root: &Path) -> Result<(), String> {
    let catalog =
        ArchiveV2Catalog::load(&root.join("catalog.av2")).map_err(|error| error.to_string())?;
    let reader = ArchiveV2Reader::open(
        catalog.identity.clone(),
        &root.join("catalog.av2"),
        ArchiveV2ReaderConfig {
            role: ArchiveV2Role::FullArchive,
            root: root.to_path_buf(),
            cache_root: None,
            cache_quota_bytes: 0,
            max_decoded_segments: 2,
            allow_remote_fetch: false,
            sources: Vec::new(),
        },
    )
    .map_err(|error| error.to_string())?;
    state.attach_archive_v2_reader(reader);
    Ok(())
}

fn read_retirement_manifest(path: &Path) -> Result<ArchiveV2RetirementManifest, String> {
    let encoded =
        fs::read(path).map_err(|error| format!("failed reading {}: {error}", path.display()))?;
    ArchiveV2RetirementManifest::decode_canonical(&encoded).map_err(|error| error.to_string())
}

fn require_retirement_acknowledgements(args: &CommandArgs) -> Result<(), String> {
    if !args.flag("acknowledge-stopped-validator") || !args.flag("acknowledge-v2-rollback-only") {
        return Err(
            "retirement requires --acknowledge-stopped-validator and --acknowledge-v2-rollback-only"
                .to_string(),
        );
    }
    Ok(())
}

fn parse_retirement_replica_evidence(
    specs: Vec<&str>,
    segment_object_hash: Hash,
) -> Result<Vec<ArchiveV2ReplicaEvidence>, String> {
    if specs.is_empty() {
        return Err("retirement authorization requires --replica-evidence".to_string());
    }
    specs
        .into_iter()
        .map(|spec| {
            let parts = spec.split(',').collect::<Vec<_>>();
            if parts.len() != 3 || parts[0].is_empty() || parts[1].is_empty() {
                return Err(format!(
                    "replica evidence {spec:?} must use destination,failure-domain,verified-unix-seconds"
                ));
            }
            Ok(ArchiveV2ReplicaEvidence {
                destination: parts[0].to_string(),
                failure_domain: parts[1].to_string(),
                segment_object_hash,
                verified_unix_seconds: parse_u64(parts[2], "replica-evidence timestamp")?,
            })
        })
        .collect()
}

fn run_verify(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(&["root", "start-index", "max-objects"], &[])?;
    let root = PathBuf::from(args.required("root")?);
    let start_index = parse_u64(args.optional("start-index")?.unwrap_or("0"), "start-index")?;
    let max_objects = parse_u64(args.optional("max-objects")?.unwrap_or("1"), "max-objects")?;
    if max_objects == 0 || max_objects > MAX_VERIFY_OBJECTS {
        return Err(format!("--max-objects must be in 1..={MAX_VERIFY_OBJECTS}"));
    }
    let catalog =
        ArchiveV2Catalog::load(&root.join("catalog.av2")).map_err(|error| error.to_string())?;
    let manifests = active_manifests(&catalog)?;
    if start_index as usize > manifests.len() {
        return Err("--start-index is beyond the catalog".to_string());
    }
    let end_index = start_index
        .saturating_add(max_objects)
        .min(manifests.len() as u64);
    let mut verified_bytes = 0u64;
    let mut verified_object_hashes = Vec::new();
    for manifest in &manifests[start_index as usize..end_index as usize] {
        let stored_manifest = ArchiveV2Manifest::decode_canonical(
            &fs::read(manifest_path(&root, &manifest.segment_object_hash))
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        if &stored_manifest != *manifest {
            return Err(format!(
                "promoted manifest {} conflicts with catalog",
                manifest.segment_object_hash
            ));
        }
        let object = fs::read(object_path(&root, &manifest.segment_object_hash))
            .map_err(|error| error.to_string())?;
        ArchiveV2SegmentCodec::decode(&object, manifest, &catalog.identity)
            .map_err(|error| error.to_string())?;
        verified_bytes = verified_bytes.saturating_add(object.len() as u64);
        verified_object_hashes.push(manifest.segment_object_hash.to_hex());
    }
    print_json(&json!({
        "operation": "verify",
        "catalog_root": catalog.catalog_root.to_hex(),
        "start_index": start_index,
        "next_index": end_index,
        "verified_objects": end_index.saturating_sub(start_index),
        "verified_object_hashes": verified_object_hashes,
        "verified_bytes": verified_bytes,
        "complete": end_index == manifests.len() as u64,
    }))
}

fn run_repair(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &["root", "network-id", "genesis-hash", "output"],
        &["install"],
    )?;
    let root = PathBuf::from(args.required("root")?);
    let identity = parse_identity(args)?;
    let recovered = ArchiveV2Catalog::recover_from_directory(&root, identity)
        .map_err(|error| error.to_string())?;
    if recovered.entries.is_empty() {
        return Err("repair found no promoted manifest/object pairs".to_string());
    }
    let output = if args.flag("install") {
        if args.optional("output")?.is_some() {
            return Err("--install and --output are mutually exclusive".to_string());
        }
        root.join("catalog.av2")
    } else {
        PathBuf::from(
            args.optional("output")?
                .ok_or_else(|| "repair requires --output or --install".to_string())?,
        )
    };
    if output.exists() {
        return Err(format!(
            "refusing to overwrite existing repair target {}",
            output.display()
        ));
    }
    recovered
        .store_atomic(&output)
        .map_err(|error| error.to_string())?;
    print_json(&json!({
        "operation": "repair",
        "installed": args.flag("install"),
        "output": output,
        "catalog_root": recovered.catalog_root.to_hex(),
        "segments": recovered.entries.len(),
    }))
}

#[derive(Debug, Clone, Copy)]
struct CliFilesystemCapacity {
    available_bytes: u64,
    total_bytes: u64,
}

#[cfg(all(unix, target_os = "linux"))]
fn cli_statvfs_block_count(value: libc::fsblkcnt_t) -> u64 {
    value
}

#[cfg(all(unix, not(target_os = "linux")))]
fn cli_statvfs_block_count(value: libc::fsblkcnt_t) -> u64 {
    value as u64
}

fn capacity_probe_path(path: &Path) -> Result<PathBuf, String> {
    let mut probe = path;
    loop {
        if probe.exists() {
            return Ok(probe.to_path_buf());
        }
        probe = probe.parent().ok_or_else(|| {
            format!(
                "no existing ancestor is available for capacity path {}",
                path.display()
            )
        })?;
    }
}

#[cfg(unix)]
fn cli_filesystem_capacity(path: &Path) -> Result<CliFilesystemCapacity, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path_bytes = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("filesystem path contains a NUL byte: {}", path.display()))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(path_bytes.as_ptr(), stat.as_mut_ptr()) };
    if result != 0 {
        return Err(format!(
            "failed inspecting filesystem capacity for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let stat = unsafe { stat.assume_init() };
    let block_size = if stat.f_frsize > 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    };
    Ok(CliFilesystemCapacity {
        available_bytes: cli_statvfs_block_count(stat.f_bavail).saturating_mul(block_size),
        total_bytes: cli_statvfs_block_count(stat.f_blocks).saturating_mul(block_size),
    })
}

#[cfg(not(unix))]
fn cli_filesystem_capacity(_path: &Path) -> Result<CliFilesystemCapacity, String> {
    Err("filesystem capacity inspection is unsupported on this platform".to_string())
}

fn archive_v2_build_capacity_preflight(
    state_dir: &Path,
    archive_root: &Path,
    identity: &ArchiveV2Identity,
) -> Result<ArchiveV2CapacityDecision, String> {
    let hot = cli_filesystem_capacity(&capacity_probe_path(state_dir)?)?;
    let archive = cli_filesystem_capacity(&capacity_probe_path(archive_root)?)?;
    archive_v2_build_capacity_decision(hot, archive, &identity.network_id)
}

fn archive_v2_build_capacity_decision(
    hot: CliFilesystemCapacity,
    archive: CliFilesystemCapacity,
    network_id: &str,
) -> Result<ArchiveV2CapacityDecision, String> {
    let absolute_reserve = if network_id == "lichen-testnet-1" {
        TESTNET_CAPACITY_FLOOR_BYTES
    } else {
        DEFAULT_CAPACITY_FLOOR_BYTES
    };
    let decision = ArchiveV2CapacityGuard::evaluate_adaptive(
        ArchiveV2CapacityInputs {
            segment_build_enabled: true,
            verified_cache_enabled: false,
            checkpoint_enabled: false,
            hot_available_bytes: hot.available_bytes,
            archive_available_bytes: archive.available_bytes,
            cache_available_bytes: archive.available_bytes,
            mutable_state_write_peak_bytes: 0,
            wal_peak_bytes: 0,
            bounded_compaction_peak_bytes: 0,
            checkpoint_peak_bytes: 0,
            segment_staging_peak_bytes: SEGMENT_OPERATION_PEAK_BYTES,
            verification_copy_bytes: SEGMENT_OPERATION_PEAK_BYTES,
            replication_retry_bytes: SEGMENT_OPERATION_PEAK_BYTES,
            filesystem_reserve_bytes: absolute_reserve,
            cache_fetch_staging_bytes: 0,
            cache_eviction_margin_bytes: 0,
        },
        ArchiveV2CapacityThresholds {
            hot_warning_bytes: absolute_reserve,
            hot_fatal_bytes: absolute_reserve,
            archive_warning_bytes: absolute_reserve,
            cache_warning_bytes: absolute_reserve,
        },
        ArchiveV2CapacityTotals {
            // The builder opens the source stores read-only and stages every
            // byte under archive_root. Preserve the absolute source floor,
            // but do not impose a writable-filesystem percentage reserve on
            // a source that this operation cannot grow. The writable archive
            // destination retains its percentage and operation reserves.
            hot_total_bytes: 0,
            archive_total_bytes: archive.total_bytes,
            cache_total_bytes: archive.total_bytes,
        },
        ArchiveV2AdaptiveReservePolicy {
            reserve_basis_points: CAPACITY_RESERVE_BASIS_POINTS,
            emergency_evidence_reserve_bytes: EVIDENCE_RESERVE_BYTES,
            ..ArchiveV2AdaptiveReservePolicy::default()
        },
    )
    .map_err(|error| error.to_string())?;
    if decision.action != ArchiveV2PressureAction::Normal {
        return Err(format!(
            "Archive V2 build is blocked by adaptive capacity: action={:?} limiting_component={:?} available_bytes={} required_bytes={} reasons={}",
            decision.action,
            decision.limiting_component,
            decision.available_bytes,
            decision.required_bytes,
            decision.reasons.join("; ")
        ));
    }
    Ok(decision)
}

fn run_build(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "cold-store",
            "root",
            "network-id",
            "genesis-hash",
            "start-slot",
            "end-slot",
            "finality-depth-slots",
            "zstd-level",
            "frame-bytes",
            "max-frame-bytes",
            "dictionary",
            "replica-root",
            "required-replicas",
        ],
        &["acknowledge-exact-testnet-missing-watermark"],
    )?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let root = PathBuf::from(args.required("root")?);
    let mut state = StateStore::open_read_only_with_cache_mb(&state_dir, Some(256))?;
    if let Some(cold) = args.optional("cold-store")? {
        state.open_cold_store_read_only(cold)?;
    }
    let identity = parse_identity(args)?;
    let start_slot = parse_u64(args.required("start-slot")?, "start-slot")?;
    let end_slot = parse_u64(args.required("end-slot")?, "end-slot")?;
    let required_finality_depth_slots = parse_u64(
        args.required("finality-depth-slots")?,
        "finality-depth-slots",
    )?;
    if required_finality_depth_slots == 0 {
        return Err("--finality-depth-slots must be non-zero".to_string());
    }
    let zstd_level = args
        .required("zstd-level")?
        .parse::<i32>()
        .map_err(|error| format!("invalid --zstd-level: {error}"))?;
    let dictionary = args
        .optional("dictionary")?
        .map(fs::read)
        .transpose()
        .map_err(|error| format!("failed reading codec dictionary: {error}"))?
        .unwrap_or_default();
    let max_frame_bytes = match args.optional("max-frame-bytes")? {
        Some(value) => parse_u32(value, "max-frame-bytes")?,
        None => 64 * 1024 * 1024,
    };
    let codec = ArchiveV2CodecConfig {
        zstd_level,
        target_frame_bytes: parse_u32(args.required("frame-bytes")?, "frame-bytes")?,
        max_frame_bytes,
        dictionary,
    };
    codec.validate().map_err(|error| error.to_string())?;
    let capacity = archive_v2_build_capacity_preflight(&state_dir, &root, &identity)?;
    let replica_roots = args
        .repeated("replica-root")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let required_replica_count = match args.optional("required-replicas")? {
        Some(value) => parse_usize(value, "required-replicas")?,
        None => replica_roots.len(),
    };
    let options = ArchiveV2BuildOptions {
        root,
        start_slot,
        end_slot,
        required_finality_depth_slots,
        codec,
        replica_roots,
        required_replica_count,
        acknowledge_exact_testnet_missing_watermark: args
            .flag("acknowledge-exact-testnet-missing-watermark"),
    };
    let builder =
        ArchiveV2Builder::new(&state, identity, options).map_err(|error| error.to_string())?;
    let report = builder.build().map_err(|error| error.to_string())?;
    print_json(&json!({
        "operation": "build",
        "start_slot": report.start_slot,
        "end_slot": report.end_slot,
        "segment_object_hash": report.segment_object_hash.map(|hash| hash.to_hex()),
        "segment_content_root": report.segment_content_root.map(|hash| hash.to_hex()),
        "block_count": report.block_count,
        "transaction_count": report.transaction_count,
        "public_index_rows": report.public_index_rows,
        "segment_bytes": report.segment_bytes,
        "replica_acknowledgements": report.replica_acknowledgements,
        "resumed": report.resumed,
        "promoted": report.promoted,
        "catalog_root": report.catalog_root.map(|hash| hash.to_hex()),
        "capacity": capacity,
    }))
}

fn run_mirror(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "root",
            "source",
            "destination",
            "journal",
            "required-replicas",
            "required-failure-domains",
            "max-objects",
            "max-bytes",
        ],
        &[],
    )?;
    let root = PathBuf::from(args.required("root")?);
    let catalog =
        ArchiveV2Catalog::load(&root.join("catalog.av2")).map_err(|error| error.to_string())?;
    let sources = if args.repeated("source").is_empty() {
        vec![directory_replica("local", "local", &root)?]
    } else {
        parse_replicas(args.repeated("source"))?
    };
    let destinations = parse_replicas(args.repeated("destination"))?;
    if destinations.is_empty() {
        return Err("mirror requires at least one --destination name:domain:path".to_string());
    }
    let policy = parse_policy(args, destinations.len())?;
    let journal = args
        .optional("journal")?
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("staging").join("mirror-cli.journal"));
    run_replication(
        args,
        &catalog,
        sources,
        destinations,
        policy,
        &journal,
        "mirror",
    )
}

fn run_restore(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "root",
            "source",
            "network-id",
            "genesis-hash",
            "journal",
            "max-objects",
            "max-bytes",
        ],
        &[],
    )?;
    let root = PathBuf::from(args.required("root")?);
    let identity = parse_identity(args)?;
    let sources = parse_replicas(args.repeated("source"))?;
    if sources.is_empty() {
        return Err("restore requires at least one --source name:domain:path".to_string());
    }
    let discovery =
        discover_archive_v2_catalog(&identity, &sources).map_err(|error| error.to_string())?;
    let catalog = discovery
        .catalog
        .ok_or_else(|| "verified catalog discovery returned no catalog".to_string())?;
    let destination = directory_replica("restore-target", "local-restore", &root)?;
    let journal = args
        .optional("journal")?
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("staging").join("restore-cli.journal"));
    run_replication(
        args,
        &catalog,
        sources,
        vec![destination],
        ArchiveV2ReplicaPolicy {
            required_replicas: 1,
            required_failure_domains: 1,
            require_authenticated: true,
        },
        &journal,
        "restore",
    )
}

fn run_replication(
    args: &CommandArgs,
    catalog: &ArchiveV2Catalog,
    sources: Vec<Arc<dyn ArchiveV2ReplicaTransport>>,
    destinations: Vec<Arc<dyn ArchiveV2ReplicaTransport>>,
    policy: ArchiveV2ReplicaPolicy,
    journal: &Path,
    operation: &str,
) -> Result<(), String> {
    let max_bytes = match args.optional("max-bytes")? {
        Some(value) => parse_u64(value, "max-bytes")?,
        None => DEFAULT_MAX_MIRROR_BYTES,
    };
    let limits = ArchiveV2MirrorLimits {
        max_objects: parse_u64(args.optional("max-objects")?.unwrap_or("1"), "max-objects")?,
        max_bytes,
    };
    let replicator = ArchiveV2Replicator::new(sources, destinations, policy)
        .map_err(|error| error.to_string())?;
    let report = replicator
        .mirror_pass(catalog, journal, limits)
        .map_err(|error| error.to_string())?;
    print_json(&json!({
        "operation": operation,
        "catalog_root": report.catalog_root.to_hex(),
        "mirrored_objects": report.mirrored_objects,
        "mirrored_bytes": report.mirrored_bytes,
        "next_object_index": report.next_object_index,
        "complete": report.complete,
        "source_failures": report.source_failures,
        "destination_failures": report.destination_failures,
        "acknowledgements": report.acknowledgements,
        "journal": journal,
    }))
}

fn run_benchmark(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "cold-store",
            "network-id",
            "genesis-hash",
            "start-slot",
            "end-slot",
            "label",
            "output",
            "candidate",
        ],
        &[],
    )?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let mut state = StateStore::open_read_only_with_cache_mb(&state_dir, Some(256))?;
    if let Some(cold) = args.optional("cold-store")? {
        state.open_cold_store_read_only(cold)?;
    }
    let identity = parse_identity(args)?;
    let start_slot = parse_u64(args.required("start-slot")?, "start-slot")?;
    let end_slot = parse_u64(args.required("end-slot")?, "end-slot")?;
    if end_slot < start_slot {
        return Err("--end-slot precedes --start-slot".to_string());
    }
    let source_genesis_hash = state
        .get_block_by_slot(0)?
        .ok_or_else(|| "benchmark source is missing canonical genesis block 0".to_string())?
        .hash();
    if identity.genesis_hash != source_genesis_hash {
        return Err(format!(
            "--genesis-hash does not match benchmark source genesis {}",
            source_genesis_hash.to_hex()
        ));
    }
    let finalized = state.get_last_finalized_slot()?;
    if end_slot > finalized {
        return Err(format!(
            "benchmark end {end_slot} exceeds finalized slot {finalized}"
        ));
    }
    let first_before = state
        .get_block_by_slot(start_slot)?
        .ok_or_else(|| format!("missing start block {start_slot}"))?
        .hash();
    let last_before = state
        .get_block_by_slot(end_slot)?
        .ok_or_else(|| format!("missing end block {end_slot}"))?
        .hash();
    let previous_block_hash = if start_slot == 0 {
        Hash::default()
    } else {
        state
            .get_block_by_slot(start_slot - 1)?
            .ok_or_else(|| format!("missing predecessor block {}", start_slot - 1))?
            .hash()
    };
    let contents = state.export_archive_v2_segment_contents(start_slot, end_slot)?;
    let finalized_after = state.get_last_finalized_slot()?;
    let first_after = state
        .get_block_by_slot(start_slot)?
        .map(|block| block.hash());
    let last_after = state.get_block_by_slot(end_slot)?.map(|block| block.hash());
    if finalized_after < end_slot
        || first_after != Some(first_before)
        || last_after != Some(last_before)
    {
        return Err("benchmark source changed during fixed-range collection".to_string());
    }
    let plan = benchmark_plan(args)?;
    let report = benchmark_archive_v2_range(
        args.optional("label")?.unwrap_or("operator-range"),
        identity,
        None,
        previous_block_hash,
        &contents,
        &plan,
    )
    .map_err(|error| error.to_string())?;
    let output = PathBuf::from(args.required("output")?);
    write_json_create_new(&output, &report)?;
    print_json(&json!({
        "operation": "benchmark",
        "output": output,
        "start_slot": start_slot,
        "end_slot": end_slot,
        "candidates": report.measurements.len(),
        "successful_candidates": report.measurements.iter().filter(|measurement| measurement.error.is_none()).count(),
    }))
}

fn benchmark_plan(args: &CommandArgs) -> Result<ArchiveV2BenchmarkPlan, String> {
    let candidate_specs = args.repeated("candidate");
    if candidate_specs.is_empty() {
        return Ok(ArchiveV2BenchmarkPlan::required_matrix());
    }
    let candidates = candidate_specs
        .into_iter()
        .map(parse_benchmark_candidate)
        .collect::<Result<Vec<_>, _>>()?;
    let plan = ArchiveV2BenchmarkPlan {
        candidates,
        random_lookup_samples: 64,
    };
    plan.validate().map_err(|error| error.to_string())?;
    Ok(plan)
}

fn parse_benchmark_candidate(spec: &str) -> Result<ArchiveV2BenchmarkCandidate, String> {
    let parts = spec.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err("--candidate must be zstd-level:frame-bytes:dictionary-kind".to_string());
    }
    let zstd_level = parts[0]
        .parse()
        .map_err(|error| format!("invalid --candidate Zstandard level: {error}"))?;
    let frame_bytes = parts[1]
        .parse()
        .map_err(|error| format!("invalid --candidate frame bytes: {error}"))?;
    let dictionary = match parts[2] {
        "none" => ArchiveV2DictionaryKind::None,
        "repeated_public_keys" => ArchiveV2DictionaryKind::RepeatedPublicKeys,
        "trained64_kib" => ArchiveV2DictionaryKind::Trained64Kib,
        "trained128_kib" => ArchiveV2DictionaryKind::Trained128Kib,
        value => return Err(format!("unsupported --candidate dictionary kind {value:?}")),
    };
    Ok(ArchiveV2BenchmarkCandidate {
        zstd_level,
        frame_bytes,
        dictionary,
    })
}

fn run_public_history_manifest(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "root",
            "source-dir",
            "cache-root",
            "cache-quota-bytes",
            "chunk-size",
        ],
        &[],
    )?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let root = PathBuf::from(args.required("root")?);
    let checkpoint_meta_path = state_dir.join("checkpoint_meta.json");
    let checkpoint_meta: CheckpointMeta =
        serde_json::from_slice(&fs::read(&checkpoint_meta_path).map_err(|error| {
            format!(
                "failed reading checkpoint metadata {}: {error}",
                checkpoint_meta_path.display()
            )
        })?)
        .map_err(|error| {
            format!(
                "failed decoding checkpoint metadata {}: {error}",
                checkpoint_meta_path.display()
            )
        })?;
    let CheckpointSnapshotProfile::HotRepairV1 {
        history_start_slot,
        archive_v2_catalog_root: Some(bound_handoff_root),
    } = checkpoint_meta.snapshot_profile
    else {
        return Err(
            "public-history-manifest requires a catalog-bound hot_repair_v1 checkpoint".to_string(),
        );
    };

    let catalog_path = root.join("catalog.av2");
    let catalog = ArchiveV2Catalog::load(&catalog_path).map_err(|error| error.to_string())?;
    let actual_handoff_root = catalog
        .checkpoint_handoff_root(history_start_slot)
        .map_err(|error| error.to_string())?;
    if actual_handoff_root.0 != bound_handoff_root {
        return Err(format!(
            "checkpoint handoff {} differs from catalog handoff {}",
            hex::encode(bound_handoff_root),
            actual_handoff_root
        ));
    }

    let source_dirs = args
        .repeated("source-dir")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let cache_root = args.optional("cache-root")?.map(PathBuf::from);
    let cache_quota_bytes = match args.optional("cache-quota-bytes")? {
        Some(value) => parse_u64(value, "cache-quota-bytes")?,
        None if source_dirs.is_empty() => 0,
        None => 8 * 1024 * 1024 * 1024,
    };
    if source_dirs.is_empty() {
        if cache_root.is_some() || cache_quota_bytes != 0 {
            return Err(
                "--cache-root and --cache-quota-bytes require at least one --source-dir"
                    .to_string(),
            );
        }
    } else if cache_root.is_none() || cache_quota_bytes == 0 {
        return Err(
            "authenticated source verification requires --cache-root and a non-zero --cache-quota-bytes"
                .to_string(),
        );
    }
    for source in &source_dirs {
        let source_catalog = ArchiveV2Catalog::load(&source.join("catalog.av2"))
            .map_err(|error| format!("invalid Archive V2 source {}: {error}", source.display()))?;
        if source_catalog.identity != catalog.identity {
            return Err(format!(
                "Archive V2 source {} has a different network or genesis identity",
                source.display()
            ));
        }
    }
    let sources = source_dirs
        .iter()
        .enumerate()
        .map(|(index, source)| {
            Arc::new(ArchiveV2DirectorySource::new(
                format!("logical-manifest-source-{index}"),
                source,
                true,
            )) as Arc<dyn ArchiveV2ObjectSource>
        })
        .collect::<Vec<_>>();
    let reader = ArchiveV2Reader::open(
        catalog.identity.clone(),
        &catalog_path,
        ArchiveV2ReaderConfig {
            role: if sources.is_empty() {
                ArchiveV2Role::FullArchive
            } else {
                ArchiveV2Role::VerifiedCache
            },
            root: root.clone(),
            cache_root: cache_root.clone(),
            cache_quota_bytes,
            max_decoded_segments: catalog.entries.len().clamp(1, 8),
            allow_remote_fetch: !sources.is_empty(),
            sources,
        },
    )
    .map_err(|error| error.to_string())?;
    let state = StateStore::open_read_only_with_cache_mb(&state_dir, Some(256))?;
    let last_slot = state.get_last_slot()?;
    if last_slot != checkpoint_meta.slot {
        return Err(format!(
            "checkpoint metadata slot {} differs from stored last slot {last_slot}",
            checkpoint_meta.slot
        ));
    }
    let chunk_size = match args.optional("chunk-size")? {
        Some(value) => parse_u64(value, "chunk-size")?.clamp(1, 50_000),
        None => 1_000,
    };
    let manifest = state.compute_archive_v2_checkpoint_public_history_manifest(
        &reader,
        checkpoint_meta.slot,
        checkpoint_meta.snapshot_profile,
        PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
        chunk_size,
    )?;
    print_json(&json!({
        "operation": "public_history_manifest",
        "state_dir": state_dir,
        "archive_v2_root": root,
        "archive_v2_catalog_root": catalog.catalog_root.to_hex(),
        "archive_v2_handoff_root": actual_handoff_root.to_hex(),
        "history_start_slot": history_start_slot,
        "last_slot": last_slot,
        "state_root": state.compute_state_root_read_only().to_hex(),
        "chunk_size": chunk_size,
        "manifest_root": hex::encode(manifest.root),
        "manifest": manifest,
    }))
}

fn run_profile_source(args: &CommandArgs) -> Result<(), String> {
    args.ensure_only(
        &[
            "state-dir",
            "cold-store",
            "start-slot",
            "end-slot",
            "top-blocks",
        ],
        &[],
    )?;
    let state_dir = PathBuf::from(args.required("state-dir")?);
    let mut state = StateStore::open_read_only_with_cache_mb(&state_dir, Some(256))?;
    if let Some(cold) = args.optional("cold-store")? {
        state.open_cold_store_read_only(cold)?;
    }
    let start_slot = parse_u64(args.required("start-slot")?, "start-slot")?;
    let end_slot = parse_u64(args.required("end-slot")?, "end-slot")?;
    if end_slot < start_slot {
        return Err("--end-slot precedes --start-slot".to_string());
    }
    let block_count = end_slot
        .checked_sub(start_slot)
        .and_then(|span| span.checked_add(1))
        .ok_or_else(|| "profile range length overflow".to_string())?;
    if block_count > 1_000_000 {
        return Err("profile-source range exceeds the 1,000,000-block bound".to_string());
    }
    let top_block_limit = parse_usize(args.optional("top-blocks")?.unwrap_or("20"), "top-blocks")?;
    if top_block_limit == 0 || top_block_limit > 1_000 {
        return Err("--top-blocks must be in 1..=1000".to_string());
    }
    let finalized = state.get_last_finalized_slot()?;
    if end_slot > finalized {
        return Err(format!(
            "profile end {end_slot} exceeds finalized slot {finalized}"
        ));
    }
    let genesis_hash = state
        .get_block_by_slot(0)?
        .ok_or_else(|| "profile source is missing canonical genesis block 0".to_string())?
        .hash();
    let mut total_block_bytes = 0u64;
    let mut total_transaction_bytes = 0u64;
    let mut total_transactions = 0u64;
    let mut total_commit_signatures = 0u64;
    let mut nonempty_blocks = 0u64;
    let mut first_timestamp = None;
    let mut last_timestamp = None;
    let mut previous_hash = if start_slot == 0 {
        Hash::default()
    } else {
        state
            .get_block_by_slot(start_slot - 1)?
            .ok_or_else(|| format!("profile source is missing predecessor {}", start_slot - 1))?
            .hash()
    };
    let mut largest_blocks = Vec::<(u64, u64, u64, u64)>::new();
    for slot in start_slot..=end_slot {
        let block = state
            .get_block_by_slot(slot)?
            .ok_or_else(|| format!("profile source is missing canonical block {slot}"))?;
        if block.header.parent_hash != previous_hash {
            return Err(format!(
                "profile source continuity mismatch at canonical block {slot}"
            ));
        }
        previous_hash = block.hash();
        let block_bytes =
            serialized_size_legacy_bincode(&block, "Archive V2 source profile block")?;
        let transaction_bytes =
            block
                .transactions
                .iter()
                .try_fold(0u64, |total, transaction| {
                    serialized_size_legacy_bincode(
                        transaction,
                        "Archive V2 source profile transaction",
                    )
                    .map(|bytes| total.saturating_add(bytes))
                })?;
        let transaction_count = block.transactions.len() as u64;
        total_block_bytes = total_block_bytes.saturating_add(block_bytes);
        total_transaction_bytes = total_transaction_bytes.saturating_add(transaction_bytes);
        total_transactions = total_transactions.saturating_add(transaction_count);
        total_commit_signatures =
            total_commit_signatures.saturating_add(block.commit_signatures.len() as u64);
        if transaction_count > 0 {
            nonempty_blocks = nonempty_blocks.saturating_add(1);
        }
        first_timestamp.get_or_insert(block.header.timestamp);
        last_timestamp = Some(block.header.timestamp);
        largest_blocks.push((block_bytes, slot, transaction_count, transaction_bytes));
    }
    largest_blocks.sort_unstable_by(|left, right| right.cmp(left));
    largest_blocks.truncate(top_block_limit);
    let largest_blocks = largest_blocks
        .into_iter()
        .map(
            |(encoded_bytes, slot, transaction_count, transaction_bytes)| {
                json!({
                    "slot": slot,
                    "encoded_bytes": encoded_bytes,
                    "transaction_count": transaction_count,
                    "transaction_bytes": transaction_bytes,
                })
            },
        )
        .collect::<Vec<_>>();
    print_json(&json!({
        "operation": "profile_source",
        "state_dir": state_dir,
        "start_slot": start_slot,
        "end_slot": end_slot,
        "finalized_slot": finalized,
        "genesis_hash": genesis_hash.to_hex(),
        "block_count": block_count,
        "nonempty_blocks": nonempty_blocks,
        "transaction_count": total_transactions,
        "commit_signature_count": total_commit_signatures,
        "block_bytes": total_block_bytes,
        "transaction_bytes": total_transaction_bytes,
        "first_timestamp": first_timestamp,
        "last_timestamp": last_timestamp,
        "largest_blocks": largest_blocks,
    }))
}

fn parse_identity(args: &CommandArgs) -> Result<ArchiveV2Identity, String> {
    let identity = ArchiveV2Identity {
        network_id: args.required("network-id")?.to_string(),
        genesis_hash: Hash::from_hex(args.required("genesis-hash")?)?,
    };
    identity.validate().map_err(|error| error.to_string())?;
    Ok(identity)
}

fn parse_policy(
    args: &CommandArgs,
    destination_count: usize,
) -> Result<ArchiveV2ReplicaPolicy, String> {
    let required_replicas = match args.optional("required-replicas")? {
        Some(value) => parse_usize(value, "required-replicas")?,
        None => destination_count,
    };
    let required_failure_domains = match args.optional("required-failure-domains")? {
        Some(value) => parse_usize(value, "required-failure-domains")?,
        None => required_replicas,
    };
    ArchiveV2ReplicaPolicy {
        required_replicas,
        required_failure_domains,
        require_authenticated: true,
    }
    .validate(destination_count)
    .map_err(|error| error.to_string())
}

fn parse_replicas(specs: Vec<&str>) -> Result<Vec<Arc<dyn ArchiveV2ReplicaTransport>>, String> {
    specs
        .into_iter()
        .map(|spec| {
            let mut parts = spec.splitn(3, ':');
            let name = parts.next().unwrap_or_default();
            let domain = parts.next().unwrap_or_default();
            let path = parts.next().unwrap_or_default();
            if name.is_empty() || domain.is_empty() || path.is_empty() {
                return Err(format!(
                    "replica {spec:?} must use the form name:failure-domain:path"
                ));
            }
            directory_replica(name, domain, Path::new(path))
        })
        .collect()
}

fn directory_replica(
    name: &str,
    domain: &str,
    root: &Path,
) -> Result<Arc<dyn ArchiveV2ReplicaTransport>, String> {
    Ok(Arc::new(
        ArchiveV2DirectoryReplica::new(name, domain, root, true)
            .map_err(|error| error.to_string())?,
    ))
}

fn active_manifests(catalog: &ArchiveV2Catalog) -> Result<Vec<&ArchiveV2Manifest>, String> {
    catalog
        .entries
        .iter()
        .map(|entry| {
            catalog
                .active_manifest(&entry.manifest.segment_object_hash)
                .map_err(|error| error.to_string())
        })
        .collect()
}

fn object_path(root: &Path, hash: &Hash) -> PathBuf {
    root.join("objects").join(format!("{}.av2s", hash.to_hex()))
}

fn manifest_path(root: &Path, hash: &Hash) -> PathBuf {
    root.join("manifests")
        .join(format!("{}.av2m", hash.to_hex()))
}

fn parse_u64(value: &str, name: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|error| format!("invalid --{name}: {error}"))
}

fn parse_optional_retirement_window(args: &CommandArgs) -> Result<Option<(u64, u64)>, String> {
    match (args.optional("start-slot")?, args.optional("end-slot")?) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) => {
            let start = parse_u64(start, "start-slot")?;
            let end = parse_u64(end, "end-slot")?;
            if end < start {
                return Err("--end-slot precedes --start-slot".to_string());
            }
            Ok(Some((start, end)))
        }
        _ => Err("--start-slot and --end-slot must be provided together".to_string()),
    }
}

fn parse_retirement_passes_per_open(value: &str) -> Result<u64, String> {
    let passes = parse_u64(value, "max-passes-per-open")?;
    if passes == 0 || passes > MAX_RETIREMENT_PASSES_PER_OPEN {
        return Err(format!(
            "--max-passes-per-open must be in 1..={MAX_RETIREMENT_PASSES_PER_OPEN}"
        ));
    }
    Ok(passes)
}

fn run_retirement_passes_per_open(
    max_passes: u64,
    mut run_pass: impl FnMut() -> Result<ArchiveV2RetirementPassReport, String>,
) -> Result<(ArchiveV2RetirementPassReport, u64), String> {
    if max_passes == 0 || max_passes > MAX_RETIREMENT_PASSES_PER_OPEN {
        return Err(format!(
            "retirement pass count must be in 1..={MAX_RETIREMENT_PASSES_PER_OPEN}"
        ));
    }
    let mut report = run_pass()?;
    let mut passes_completed = 1u64;
    for _ in 1..max_passes {
        if matches!(
            report.phase,
            ArchiveV2RetirementPhase::ReclaimPending | ArchiveV2RetirementPhase::Complete
        ) {
            break;
        }
        let next = run_pass()?;
        passes_completed = passes_completed.saturating_add(1);
        report.phase = next.phase;
        report.category = next.category;
        report.scanned_rows = report.scanned_rows.saturating_add(next.scanned_rows);
        report.skipped_absent_rebuildable_rows = report
            .skipped_absent_rebuildable_rows
            .saturating_add(next.skipped_absent_rebuildable_rows);
        report.deleted_hot_rows = report
            .deleted_hot_rows
            .saturating_add(next.deleted_hot_rows);
        report.deleted_cold_rows = report
            .deleted_cold_rows
            .saturating_add(next.deleted_cold_rows);
        report.deleted_logical_bytes = report
            .deleted_logical_bytes
            .saturating_add(next.deleted_logical_bytes);
        report.recovered_pending_batch |= next.recovered_pending_batch;
        report.categories_completed = next.categories_completed;
        report.elapsed_millis = report.elapsed_millis.saturating_add(next.elapsed_millis);
    }
    Ok((report, passes_completed))
}

fn parse_usize(value: &str, name: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|error| format!("invalid --{name}: {error}"))
}

fn parse_u32(value: &str, name: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|error| format!("invalid --{name}: {error}"))
}

fn parse_u16(value: &str, name: &str) -> Result<u16, String> {
    value
        .parse()
        .map_err(|error| format!("invalid --{name}: {error}"))
}

fn push_bounded(values: &mut Vec<String>, value: String) {
    if values.len() < 100 {
        values.push(value);
    }
}

fn print_json(value: &serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn write_json_create_new<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "output path has no parent".to_string())?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let encoded = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("failed creating {}: {error}", path.display()))?;
    file.write_all(&encoded)
        .map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    OpenOptions::new()
        .read(true)
        .open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

fn write_bytes_create_new(path: &Path, encoded: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "output path has no parent".to_string())?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("failed creating {}: {error}", path.display()))?;
    file.write_all(encoded).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    OpenOptions::new()
        .read(true)
        .open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

fn print_usage() {
    println!(
        "lichen-archive-v2 <status|catalog-extension-check|role-preflight|role-bootstrap|snapshot-hot|verify|repair|declare-legacy-loss|build|mirror|restore|retirement-authorize|retirement-pass|retirement-reclaim|public-history-manifest|profile-source|benchmark> [options]\n\
         Run `lichen-archive-v2 <command> --help` is intentionally unsupported; unknown options fail closed.\n\
         Retirement authorization accepts paired --start-slot/--end-slot bounds inside one verified segment; omitting both authorizes the full segment.\n\
         Replica specifications use name:failure-domain:path. Retirement evidence uses destination,failure-domain,verified-unix-seconds. Verify and mirror default to one object per pass."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_role_preflight_policy_is_explicit_local_dev_only() {
        let short = ArchiveV2RoleConfig {
            recent_history_slots: 20,
            ..ArchiveV2RoleConfig::default()
        };
        assert!(role_preflight_policy_config(&short, false, true)
            .unwrap_err()
            .contains("at least 50000"));
        assert!(role_preflight_policy_config(&short, true, false)
            .unwrap_err()
            .contains("at least 50000"));

        let policy = role_preflight_policy_config(&short, true, true).unwrap();
        assert_eq!(
            policy.recent_history_slots,
            ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS
        );
        assert_eq!(short.recent_history_slots, 20);
    }

    #[cfg(unix)]
    #[test]
    fn checkpoint_sst_materialization_replaces_symlinks_with_bounded_regular_files() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.sst");
        let checkpoint = temporary.path().join("checkpoint");
        fs::create_dir(&checkpoint).unwrap();
        fs::write(&source, b"immutable-sst-bytes").unwrap();
        let linked = checkpoint.join("000123.sst");
        symlink(&source, &linked).unwrap();

        assert!(materialize_checkpoint_sst_symlinks(&checkpoint, 18).is_err());
        assert!(linked.is_symlink());
        let (count, bytes) = materialize_checkpoint_sst_symlinks(&checkpoint, 19).unwrap();
        assert_eq!(count, 1);
        assert_eq!(bytes, 19);
        assert!(!linked.is_symlink());
        assert_eq!(fs::read(linked).unwrap(), b"immutable-sst-bytes");
    }

    #[cfg(unix)]
    #[test]
    fn hot_snapshot_staging_never_links_mutable_files_or_preserves_sst_symlinks() {
        use std::os::unix::fs::{symlink, MetadataExt};

        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("state");
        let staging = temporary.path().join("staging");
        let external_sst = temporary.path().join("external.sst");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("checkpoints")).unwrap();
        fs::write(source.join("000001.sst"), b"regular-immutable-sst").unwrap();
        fs::write(&external_sst, b"symlinked-immutable-sst").unwrap();
        symlink(&external_sst, source.join("000002.sst")).unwrap();
        fs::write(source.join("CURRENT"), b"MANIFEST-000003\n").unwrap();
        fs::write(source.join("MANIFEST-000003"), b"mutable-manifest").unwrap();
        fs::write(source.join("validator-keypair.json"), b"secret-sidecar").unwrap();

        let regular_sst_inode = fs::metadata(source.join("000001.sst")).unwrap().ino();
        let current_inode = fs::metadata(source.join("CURRENT")).unwrap().ino();
        let plan = hot_snapshot_source_plan(&source).unwrap();
        assert_eq!(plan.symlink_sst_count, 1);
        assert_eq!(plan.symlink_sst_bytes, 23);
        assert_eq!(plan.hardlinked_sst_count, 1);
        assert_eq!(plan.copied_file_count, 3);

        assert_eq!(
            stage_hot_snapshot_source(&source, &staging, plan).unwrap(),
            plan
        );
        assert_eq!(
            fs::metadata(staging.join("000001.sst")).unwrap().ino(),
            regular_sst_inode
        );
        assert_ne!(
            fs::metadata(staging.join("CURRENT")).unwrap().ino(),
            current_inode
        );
        assert!(fs::symlink_metadata(source.join("000002.sst"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!fs::symlink_metadata(staging.join("000002.sst"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(staging.join("000002.sst")).unwrap(),
            b"symlinked-immutable-sst"
        );
        assert_eq!(
            fs::read(source.join("CURRENT")).unwrap(),
            b"MANIFEST-000003\n"
        );
        assert_eq!(
            fs::read(staging.join("validator-keypair.json")).unwrap(),
            b"secret-sidecar"
        );
        assert!(!staging.join("checkpoints").exists());
    }

    #[test]
    fn snapshot_hot_opens_only_isolated_staging_and_preserves_source_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("state");
        let output_parent = temporary.path().join("recovery");
        let output = output_parent.join("hot-snapshot");
        let state = StateStore::open_with_cache_mb(&source, Some(16)).unwrap();
        drop(state);

        let source_bytes = fs::read_dir(&source)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.is_file().then(|| {
                    (
                        path.file_name().unwrap().to_os_string(),
                        fs::read(&path).unwrap(),
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        let (_, args) = CommandArgs::parse(vec![
            "snapshot-hot".to_string(),
            "--state-dir".to_string(),
            source.display().to_string(),
            "--output".to_string(),
            output.display().to_string(),
            "--max-materialized-bytes".to_string(),
            (16 * 1024 * 1024u64).to_string(),
            "--minimum-remaining-bytes".to_string(),
            "1".to_string(),
        ])
        .unwrap();
        run_snapshot_hot(&args).unwrap();

        let source_bytes_after = fs::read_dir(&source)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.is_file().then(|| {
                    (
                        path.file_name().unwrap().to_os_string(),
                        fs::read(&path).unwrap(),
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(source_bytes_after, source_bytes);
        assert!(StateStore::open_read_only_with_cache_mb(&output, Some(16)).is_ok());
        assert!(!output_parent.join(".hot-snapshot.source.next").exists());
        assert!(!output_parent.join(".hot-snapshot.checkpoint.next").exists());
    }

    #[test]
    fn catalog_inventory_requires_exact_manifest_and_bounded_regular_object() {
        use lichen_core::archive_v2::ArchiveV2SegmentContents;
        use lichen_core::Block;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("archive");
        fs::create_dir_all(root.join("objects")).unwrap();
        fs::create_dir_all(root.join("manifests")).unwrap();
        let identity = ArchiveV2Identity {
            network_id: "inventory-testnet".to_string(),
            genesis_hash: Hash::hash(b"inventory-genesis"),
        };
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"inventory-state"),
            [9; 32],
            Vec::new(),
            1,
        );
        let (object_bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity,
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        let manifest_bytes = manifest.encode_canonical().unwrap();
        let manifest_hash = Hash::hash(&manifest_bytes);
        let object_path = object_path(&root, &manifest.segment_object_hash);
        let manifest_path = manifest_path(&root, &manifest.segment_object_hash);
        fs::write(&object_path, &object_bytes).unwrap();
        fs::write(&manifest_path, &manifest_bytes).unwrap();

        assert!(catalog_inventory_entry_matches(
            &root,
            &manifest,
            manifest_hash,
            object_bytes.len() as u64,
        ));
        assert!(!catalog_inventory_entry_matches(
            &root,
            &manifest,
            manifest_hash,
            object_bytes.len() as u64 - 1,
        ));

        fs::write(&manifest_path, b"wrong-manifest").unwrap();
        assert!(!catalog_inventory_entry_matches(
            &root,
            &manifest,
            manifest_hash,
            object_bytes.len() as u64,
        ));
    }

    #[cfg(unix)]
    #[test]
    fn catalog_inventory_rejects_symlinked_objects() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("archive");
        fs::create_dir_all(root.join("objects")).unwrap();
        fs::create_dir_all(root.join("manifests")).unwrap();
        let identity = ArchiveV2Identity {
            network_id: "inventory-symlink-testnet".to_string(),
            genesis_hash: Hash::hash(b"inventory-symlink-genesis"),
        };
        let block = lichen_core::Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"inventory-symlink-state"),
            [10; 32],
            Vec::new(),
            1,
        );
        let (object_bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity,
            None,
            Hash::default(),
            &lichen_core::archive_v2::ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        let manifest_bytes = manifest.encode_canonical().unwrap();
        let manifest_hash = Hash::hash(&manifest_bytes);
        let external = temporary.path().join("external.av2s");
        fs::write(&external, &object_bytes).unwrap();
        symlink(&external, object_path(&root, &manifest.segment_object_hash)).unwrap();
        fs::write(
            manifest_path(&root, &manifest.segment_object_hash),
            &manifest_bytes,
        )
        .unwrap();

        assert!(!catalog_inventory_entry_matches(
            &root,
            &manifest,
            manifest_hash,
            object_bytes.len() as u64,
        ));
    }

    #[test]
    fn parser_preserves_repeated_replica_options_and_rejects_positionals() {
        let (command, args) = CommandArgs::parse(vec![
            "mirror".to_string(),
            "--destination".to_string(),
            "a:region-a:/one".to_string(),
            "--destination".to_string(),
            "b:region-b:/two".to_string(),
        ])
        .unwrap();
        assert_eq!(command, "mirror");
        assert_eq!(args.repeated("destination").len(), 2);
        assert!(CommandArgs::parse(vec!["status".to_string(), "unexpected".to_string()]).is_err());
    }

    fn role_bootstrap_fixture_args(
        temporary: &tempfile::TempDir,
        dry_run: bool,
    ) -> (CommandArgs, PathBuf) {
        let state_dir = temporary.path().join("state");
        let cold_store = temporary.path().join("cold");
        let archive_root = temporary.path().join("archive-v2");
        fs::create_dir_all(&archive_root).unwrap();

        let mut state = StateStore::open(&state_dir).unwrap();
        let genesis = lichen_core::Block::genesis(Hash::hash(b"bootstrap-state"), 1, Vec::new());
        state.put_block_atomic(&genesis, Some(0), Some(0)).unwrap();
        state.open_cold_store(&cold_store).unwrap();
        drop(state);

        let identity = ArchiveV2Identity {
            network_id: "lichen-testnet-1".to_string(),
            genesis_hash: genesis.hash(),
        };
        ArchiveV2Catalog::empty(identity)
            .unwrap()
            .store_atomic(&archive_root.join("catalog.av2"))
            .unwrap();
        let wal = state_dir.join("consensus.wal");
        let identity_file = state_dir.join("validator-keypair.json");
        let recovery_file = state_dir.join("genesis.json");
        fs::write(&wal, b"wal").unwrap();
        fs::write(&identity_file, b"identity").unwrap();
        fs::write(&recovery_file, b"recovery").unwrap();

        let mut raw = vec![
            "role-bootstrap".to_string(),
            "--state-dir".to_string(),
            state_dir.display().to_string(),
            "--cold-store".to_string(),
            cold_store.display().to_string(),
            "--root".to_string(),
            archive_root.display().to_string(),
            "--role".to_string(),
            "consensus".to_string(),
            "--recent-history-slots".to_string(),
            "200000".to_string(),
            "--wal".to_string(),
            wal.display().to_string(),
            "--identity-file".to_string(),
            identity_file.display().to_string(),
            "--recovery-file".to_string(),
            recovery_file.display().to_string(),
            "--acknowledge-stopped-validator".to_string(),
            "--acknowledge-low-space-legacy-retirement".to_string(),
        ];
        if dry_run {
            raw.push("--dry-run".to_string());
        }
        let (_, args) = CommandArgs::parse(raw).unwrap();
        (args, archive_root.join(ARCHIVE_V2_ROLE_MARKER_FILENAME))
    }

    #[test]
    fn role_bootstrap_dry_run_never_writes_and_publish_is_idempotent() {
        let temporary = tempfile::tempdir().unwrap();
        let (dry_run, marker_path) = role_bootstrap_fixture_args(&temporary, true);
        let state_dir = PathBuf::from(dry_run.required("state-dir").unwrap());
        run_role_bootstrap(&dry_run).unwrap();
        assert!(!marker_path.exists());
        assert_eq!(
            StateStore::open_read_only_with_cache_mb(&state_dir, Some(8))
                .unwrap()
                .get_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY)
                .unwrap(),
            None
        );

        let (_, mut publish) = CommandArgs::parse(vec!["role-bootstrap".to_string()]).unwrap();
        publish.values = dry_run.values;
        publish.flags = dry_run.flags;
        publish.flags.remove("dry-run");
        run_role_bootstrap(&publish).unwrap();
        let first = fs::read(&marker_path).unwrap();
        let first_state_admission = StateStore::open_read_only_with_cache_mb(&state_dir, Some(8))
            .unwrap()
            .get_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY)
            .unwrap()
            .expect("published state admission marker");
        assert_eq!(first_state_admission.len(), 32);
        run_role_bootstrap(&publish).unwrap();
        assert_eq!(fs::read(&marker_path).unwrap(), first);
        assert_eq!(
            StateStore::open_read_only_with_cache_mb(&state_dir, Some(8))
                .unwrap()
                .get_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY)
                .unwrap(),
            Some(first_state_admission.clone())
        );
        assert_eq!(
            load_archive_v2_role_marker(&marker_path)
                .unwrap()
                .role_config
                .recent_history_slots,
            200_000
        );

        publish.values.insert(
            "recent-history-slots".to_string(),
            vec!["210000".to_string()],
        );
        publish.flags.insert("dry-run".to_string());
        assert!(run_role_bootstrap(&publish)
            .unwrap_err()
            .contains("conflicts"));
        assert_eq!(fs::read(&marker_path).unwrap(), first);
        assert_eq!(
            StateStore::open_read_only_with_cache_mb(&state_dir, Some(8))
                .unwrap()
                .get_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY)
                .unwrap(),
            Some(first_state_admission)
        );
    }

    #[test]
    fn role_bootstrap_rejects_conflicting_state_admission_before_role_marker_write() {
        let temporary = tempfile::tempdir().unwrap();
        let (publish, marker_path) = role_bootstrap_fixture_args(&temporary, false);
        let state_dir = PathBuf::from(publish.required("state-dir").unwrap());
        let state = StateStore::open(&state_dir).unwrap();
        state
            .put_metadata(ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY, &[0xA5; 32])
            .unwrap();
        state.sync_hot_wal().unwrap();
        drop(state);

        assert!(run_role_bootstrap(&publish)
            .unwrap_err()
            .contains("state admission marker conflicts"));
        assert!(!marker_path.exists());
    }

    #[test]
    fn role_bootstrap_requires_explicit_acknowledgements_before_state_access() {
        let (_, args) = CommandArgs::parse(vec![
            "role-bootstrap".to_string(),
            "--state-dir".to_string(),
            "/does/not/exist".to_string(),
        ])
        .unwrap();
        let error = run_role_bootstrap(&args).unwrap_err();
        assert!(error.contains("acknowledge-stopped-validator"));
    }

    #[test]
    fn replica_spec_requires_name_domain_and_path() {
        assert!(parse_replicas(vec!["name:domain:/archive"]).is_ok());
        assert!(parse_replicas(vec!["name:/archive"]).is_err());
    }

    #[test]
    fn benchmark_candidate_parser_is_strict() {
        assert_eq!(
            parse_benchmark_candidate("9:4194304:trained64_kib").unwrap(),
            ArchiveV2BenchmarkCandidate {
                zstd_level: 9,
                frame_bytes: 4 * 1024 * 1024,
                dictionary: ArchiveV2DictionaryKind::Trained64Kib,
            }
        );
        assert!(parse_benchmark_candidate("9:4194304:unknown").is_err());
        assert!(parse_benchmark_candidate("9:4194304").is_err());
    }

    #[test]
    fn retirement_evidence_parser_binds_every_replica_to_the_segment() {
        let hash = Hash::hash(b"retirement-cli-segment");
        let evidence = parse_retirement_replica_evidence(
            vec!["r2-primary,enam,42", "r2-replica,apac,43"],
            hash,
        )
        .unwrap();
        assert_eq!(evidence.len(), 2);
        assert!(evidence
            .iter()
            .all(|entry| entry.segment_object_hash == hash));
        assert!(parse_retirement_replica_evidence(vec!["missing-fields"], hash).is_err());
    }

    #[test]
    fn destructive_retirement_commands_require_both_acknowledgements() {
        let (_, incomplete) = CommandArgs::parse(vec![
            "retirement-pass".to_string(),
            "--acknowledge-stopped-validator".to_string(),
        ])
        .unwrap();
        assert!(require_retirement_acknowledgements(&incomplete).is_err());

        let (_, complete) = CommandArgs::parse(vec![
            "retirement-pass".to_string(),
            "--acknowledge-stopped-validator".to_string(),
            "--acknowledge-v2-rollback-only".to_string(),
        ])
        .unwrap();
        assert!(require_retirement_acknowledgements(&complete).is_ok());
    }

    #[test]
    fn retirement_passes_per_open_is_strictly_bounded() {
        assert_eq!(parse_retirement_passes_per_open("1").unwrap(), 1);
        assert_eq!(
            parse_retirement_passes_per_open("16").unwrap(),
            MAX_RETIREMENT_PASSES_PER_OPEN
        );
        assert!(parse_retirement_passes_per_open("0").is_err());
        assert!(parse_retirement_passes_per_open("17").is_err());
        assert!(parse_retirement_passes_per_open("invalid").is_err());
    }

    #[test]
    fn retirement_authorize_window_requires_a_complete_ordered_pair() {
        let (_, full_segment) =
            CommandArgs::parse(vec!["retirement-authorize".to_string()]).unwrap();
        assert_eq!(
            parse_optional_retirement_window(&full_segment).unwrap(),
            None
        );

        let (_, window) = CommandArgs::parse(vec![
            "retirement-authorize".to_string(),
            "--start-slot".to_string(),
            "250000".to_string(),
            "--end-slot".to_string(),
            "254999".to_string(),
        ])
        .unwrap();
        assert_eq!(
            parse_optional_retirement_window(&window).unwrap(),
            Some((250_000, 254_999))
        );

        let (_, incomplete) = CommandArgs::parse(vec![
            "retirement-authorize".to_string(),
            "--start-slot".to_string(),
            "250000".to_string(),
        ])
        .unwrap();
        assert!(parse_optional_retirement_window(&incomplete).is_err());

        let (_, reversed) = CommandArgs::parse(vec![
            "retirement-authorize".to_string(),
            "--start-slot".to_string(),
            "255000".to_string(),
            "--end-slot".to_string(),
            "254999".to_string(),
        ])
        .unwrap();
        assert!(parse_optional_retirement_window(&reversed).is_err());
    }

    #[test]
    fn retirement_passes_per_open_reuses_one_open_and_aggregates_reports() {
        let mut calls = 0u64;
        let (report, passes_completed) = run_retirement_passes_per_open(16, || {
            calls = calls.saturating_add(1);
            Ok(if calls == 1 {
                ArchiveV2RetirementPassReport {
                    phase: ArchiveV2RetirementPhase::Tombstoning,
                    category: Some("blocks".to_string()),
                    scanned_rows: 10,
                    skipped_absent_rebuildable_rows: 2,
                    deleted_hot_rows: 1,
                    deleted_cold_rows: 2,
                    deleted_logical_bytes: 100,
                    recovered_pending_batch: false,
                    categories_completed: 2,
                    elapsed_millis: 60_000,
                }
            } else {
                ArchiveV2RetirementPassReport {
                    phase: ArchiveV2RetirementPhase::ReclaimPending,
                    category: Some("transactions".to_string()),
                    scanned_rows: 20,
                    skipped_absent_rebuildable_rows: 3,
                    deleted_hot_rows: 3,
                    deleted_cold_rows: 4,
                    deleted_logical_bytes: 200,
                    recovered_pending_batch: true,
                    categories_completed: 8,
                    elapsed_millis: 61_000,
                }
            })
        })
        .unwrap();

        assert_eq!(calls, 2);
        assert_eq!(passes_completed, 2);
        assert_eq!(report.phase, ArchiveV2RetirementPhase::ReclaimPending);
        assert_eq!(report.category.as_deref(), Some("transactions"));
        assert_eq!(report.scanned_rows, 30);
        assert_eq!(report.skipped_absent_rebuildable_rows, 5);
        assert_eq!(report.deleted_hot_rows, 4);
        assert_eq!(report.deleted_cold_rows, 6);
        assert_eq!(report.deleted_logical_bytes, 300);
        assert!(report.recovered_pending_batch);
        assert_eq!(report.categories_completed, 8);
        assert_eq!(report.elapsed_millis, 121_000);
    }

    #[test]
    fn read_only_build_source_keeps_absolute_floor_without_writable_percentage_reserve() {
        let gib = 1024 * 1024 * 1024;
        let archive = CliFilesystemCapacity {
            available_bytes: 15 * gib,
            total_bytes: 20 * gib,
        };
        let admitted = archive_v2_build_capacity_decision(
            CliFilesystemCapacity {
                available_bytes: 6 * gib,
                total_bytes: 200 * gib,
            },
            archive,
            "lichen-testnet-1",
        )
        .unwrap();
        assert_eq!(admitted.action, ArchiveV2PressureAction::Normal);
        assert_eq!(admitted.hot_consensus_required_bytes, 5 * gib);

        let below_floor = archive_v2_build_capacity_decision(
            CliFilesystemCapacity {
                available_bytes: 5 * gib - 1,
                total_bytes: 200 * gib,
            },
            archive,
            "lichen-testnet-1",
        )
        .unwrap_err();
        assert!(below_floor.contains("action=StopValidator"));
        assert!(below_floor.contains("required_bytes=5368709120"));
    }
}
