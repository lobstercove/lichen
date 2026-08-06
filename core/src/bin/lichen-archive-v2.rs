use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lichen_core::archive_v2::{
    benchmark_archive_v2_range, discover_archive_v2_catalog, ArchiveV2AdaptiveReservePolicy,
    ArchiveV2BenchmarkCandidate, ArchiveV2BenchmarkPlan, ArchiveV2BuildOptions, ArchiveV2Builder,
    ArchiveV2CapacityDecision, ArchiveV2CapacityGuard, ArchiveV2CapacityInputs,
    ArchiveV2CapacityThresholds, ArchiveV2CapacityTotals, ArchiveV2Catalog, ArchiveV2CodecConfig,
    ArchiveV2DictionaryKind, ArchiveV2DirectoryReplica, ArchiveV2Identity,
    ArchiveV2LegacyLossDeclaration, ArchiveV2Manifest, ArchiveV2MirrorLimits,
    ArchiveV2PressureAction, ArchiveV2Reader, ArchiveV2ReaderConfig, ArchiveV2ReplicaEvidence,
    ArchiveV2ReplicaPolicy, ArchiveV2ReplicaTransport, ArchiveV2Replicator,
    ArchiveV2RetirementManifest, ArchiveV2Role, ArchiveV2RollbackAnchor, ArchiveV2SegmentCodec,
    ARCHIVE_V2_CATALOG_VERSION, ARCHIVE_V2_FORMAT_VERSION,
};
use lichen_core::codec::serialized_size_legacy_bincode;
use lichen_core::{
    keypair_password_from_env, plaintext_keypair_allowed_for_local_dev, ArchiveV2RetirementLimits,
    ArchiveV2RetirementPhase, ArchiveV2RetirementReclaimLimits, Hash, KeypairFile, StateStore,
};
use serde_json::json;

const DEFAULT_MAX_MIRROR_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_VERIFY_OBJECTS: u64 = 10_000;
const TESTNET_CAPACITY_FLOOR_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const DEFAULT_CAPACITY_FLOOR_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const CAPACITY_RESERVE_BASIS_POINTS: u16 = 500;
const SEGMENT_OPERATION_PEAK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const EVIDENCE_RESERVE_BYTES: u64 = 512 * 1024 * 1024;

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
        "verify" => run_verify(&args),
        "repair" => run_repair(&args),
        "declare-legacy-loss" => run_declare_legacy_loss(&args),
        "retirement-authorize" => run_retirement_authorize(&args),
        "retirement-pass" => run_retirement_pass(&args),
        "retirement-reclaim" => run_retirement_reclaim(&args),
        "build" => run_build(&args),
        "mirror" => run_mirror(&args),
        "restore" => run_restore(&args),
        "profile-source" => run_profile_source(&args),
        "benchmark" => run_benchmark(&args),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        _ => Err(format!(
            "unknown command {command:?}; expected status, verify, repair, declare-legacy-loss, build, mirror, restore, retirement-authorize, retirement-pass, retirement-reclaim, profile-source, or benchmark"
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
    args.ensure_only(&["root"], &[])?;
    let root = PathBuf::from(args.required("root")?);
    let catalog =
        ArchiveV2Catalog::load(&root.join("catalog.av2")).map_err(|error| error.to_string())?;
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
    let request = state.prepare_archive_v2_retirement_request(
        segment_object_hash,
        replica_evidence,
        required_replica_count,
        required_failure_domains,
        rollback_anchor,
        authorized_unix_seconds,
    )?;
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
    let journal = PathBuf::from(args.required("journal")?);
    let report = state.retire_archive_v2_segment_pass(&retirement, &journal, limits)?;
    print_json(&json!({
        "operation": "retirement-pass",
        "journal": journal,
        "phase": report.phase,
        "category": report.category,
        "scanned_rows": report.scanned_rows,
        "deleted_hot_rows": report.deleted_hot_rows,
        "deleted_cold_rows": report.deleted_cold_rows,
        "deleted_logical_bytes": report.deleted_logical_bytes,
        "recovered_pending_batch": report.recovered_pending_batch,
        "categories_completed": report.categories_completed,
        "elapsed_millis": report.elapsed_millis,
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
        "lichen-archive-v2 <status|verify|repair|declare-legacy-loss|build|mirror|restore|retirement-authorize|retirement-pass|retirement-reclaim|profile-source|benchmark> [options]\n\
         Run `lichen-archive-v2 <command> --help` is intentionally unsupported; unknown options fail closed.\n\
         Replica specifications use name:failure-domain:path. Retirement evidence uses destination,failure-domain,verified-unix-seconds. Verify and mirror default to one object per pass."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
