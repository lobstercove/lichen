//! Archive V2: immutable, content-addressed, chronological public history.
//!
//! This representation is deliberately outside consensus and wire encoding.
//! It reconstructs the existing [`crate::Block`] and [`crate::Transaction`]
//! objects exactly while allowing their archival storage layout to evolve.

mod benchmark;
mod builder;
mod capacity;
mod catalog;
mod codec;
mod format;
mod join;
mod reader;
mod replication;
mod retirement;
mod role_marker;
mod roles;

/// Canonically key-sorted public-history rows reconstructed from Archive V2.
pub type ArchiveV2Rows = Vec<(Vec<u8>, Vec<u8>)>;

pub use benchmark::{
    benchmark_archive_v2_range, ArchiveV2BenchmarkCandidate, ArchiveV2BenchmarkMeasurement,
    ArchiveV2BenchmarkPlan, ArchiveV2BenchmarkReport, ArchiveV2DictionaryKind,
};
pub use builder::{
    ArchiveV2BuildJournal, ArchiveV2BuildOptions, ArchiveV2BuildReport, ArchiveV2Builder,
    ArchiveV2FaultPoint,
};
pub use capacity::{
    ArchiveV2AdaptiveReservePolicy, ArchiveV2CapacityComponent, ArchiveV2CapacityDecision,
    ArchiveV2CapacityForecast, ArchiveV2CapacityGuard, ArchiveV2CapacityInputs,
    ArchiveV2CapacitySample, ArchiveV2CapacityThresholds, ArchiveV2CapacityTotals,
    ArchiveV2PressureAction,
};
pub use catalog::{
    ArchiveV2Catalog, ArchiveV2CatalogEntry, ArchiveV2CatalogSupersession,
    ArchiveV2LegacyLossDeclaration, ARCHIVE_V2_CATALOG_VERSION,
};
pub use codec::{
    ArchiveV2DecodedSegment, ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
    ArchiveV2TransactionLocation,
};
pub use format::{
    ArchiveV2CategoryCommitment, ArchiveV2CodecConfig, ArchiveV2Error, ArchiveV2FrameDescriptor,
    ArchiveV2FrameKind, ArchiveV2Identity, ArchiveV2Manifest, ArchiveV2PublicIndexes,
    ArchiveV2PublicRow, ArchiveV2TransactionFilter, ARCHIVE_V2_FORMAT_VERSION,
};
pub use join::{
    discover_archive_v2_catalog, ArchiveV2CatalogDiscoveryReport, ArchiveV2JoinArchiveAction,
    ArchiveV2JoinPlan, ArchiveV2MutableStateJoinMethod, ARCHIVE_V2_JOIN_PLAN_VERSION,
};
pub use reader::{
    ArchiveV2DirectorySource, ArchiveV2ObjectSource, ArchiveV2Reader, ArchiveV2ReaderConfig,
    ArchiveV2ReaderStatus,
};
pub use replication::{
    inspect_archive_v2_replica_inventory, ArchiveV2DirectoryReplica, ArchiveV2MirrorLimits,
    ArchiveV2MirrorReport, ArchiveV2ReplicaInventory, ArchiveV2ReplicaObjectInventory,
    ArchiveV2ReplicaPolicy, ArchiveV2ReplicaTransport, ArchiveV2Replicator,
};
pub use retirement::{
    ArchiveV2CategoryProof, ArchiveV2ReplicaEvidence, ArchiveV2RetirementManifest,
    ArchiveV2RetirementRequest, ArchiveV2RollbackAnchor,
};
pub use role_marker::{
    load_archive_v2_role_marker, store_archive_v2_role_marker_create_new, ArchiveV2RoleMarker,
    ARCHIVE_V2_ROLE_MARKER_FILENAME,
};
pub use roles::{
    archive_v2_state_admission_fingerprint, ArchiveV2CapabilityAdvertisement, ArchiveV2Role,
    ArchiveV2RoleAdmission, ArchiveV2RoleConfig, ArchiveV2RoleRequirements,
    ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS, ARCHIVE_V2_ROLE_CONFIG_VERSION,
    ARCHIVE_V2_STATE_ADMISSION_METADATA_KEY,
};
