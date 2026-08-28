use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};
use crate::Hash;

use super::{ArchiveV2Identity, ArchiveV2RoleConfig};

pub const ARCHIVE_V2_ROLE_MARKER_FILENAME: &str = "role-config-v1.bin";
const ARCHIVE_V2_ROLE_MARKER_MAGIC: &[u8] = b"LICHEN-AV2-ROLE\0";
const ARCHIVE_V2_ROLE_MARKER_MAX_BYTES: usize = 64 * 1024;
static ARCHIVE_V2_ROLE_MARKER_NONCE: AtomicU64 = AtomicU64::new(0);

/// Durable authorization for activating one exact Archive V2 identity and
/// runtime role. The genesis compatibility bit is retained because a bounded
/// hot store may no longer contain slot 0 after qualified legacy retirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2RoleMarker {
    pub marker_version: u16,
    pub identity: ArchiveV2Identity,
    pub role_config: ArchiveV2RoleConfig,
    pub genesis_mossstake_slot_only: bool,
}

pub fn load_archive_v2_role_marker(path: &Path) -> Result<ArchiveV2RoleMarker, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed inspecting {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err("Archive V2 role marker is not a regular file".to_string());
    }
    let encoded =
        fs::read(path).map_err(|error| format!("failed reading {}: {error}", path.display()))?;
    let minimum = ARCHIVE_V2_ROLE_MARKER_MAGIC.len() + 4 + 32;
    if encoded.len() < minimum || !encoded.starts_with(ARCHIVE_V2_ROLE_MARKER_MAGIC) {
        return Err("Archive V2 role marker is truncated".to_string());
    }
    let offset = ARCHIVE_V2_ROLE_MARKER_MAGIC.len();
    let payload_len = u32::from_le_bytes(
        encoded[offset..offset + 4]
            .try_into()
            .map_err(|_| "Archive V2 role marker length is truncated".to_string())?,
    ) as usize;
    if payload_len > ARCHIVE_V2_ROLE_MARKER_MAX_BYTES {
        return Err("Archive V2 role marker is too large".to_string());
    }
    let start = offset + 4;
    let end = start
        .checked_add(payload_len)
        .ok_or_else(|| "Archive V2 role marker length overflow".to_string())?;
    if end.checked_add(32) != Some(encoded.len())
        || Hash::hash(&encoded[start..end]).0 != encoded[end..]
    {
        return Err("Archive V2 role marker checksum mismatch".to_string());
    }
    deserialize_legacy_bincode_strict(
        &encoded[start..end],
        ARCHIVE_V2_ROLE_MARKER_MAX_BYTES as u64,
        "Archive V2 role marker",
    )
}

/// Publishes a role marker without replacing an existing authorization.
///
/// The payload is written and synced under a same-directory temporary name,
/// then linked into place with create-new semantics. A concurrent or stale
/// marker therefore fails closed instead of being overwritten.
pub fn store_archive_v2_role_marker_create_new(
    path: &Path,
    marker: &ArchiveV2RoleMarker,
) -> Result<(), String> {
    let payload = serialize_legacy_bincode(marker, "Archive V2 role marker")?;
    if payload.len() > ARCHIVE_V2_ROLE_MARKER_MAX_BYTES {
        return Err("Archive V2 role marker is too large".to_string());
    }
    let mut encoded =
        Vec::with_capacity(ARCHIVE_V2_ROLE_MARKER_MAGIC.len() + 4 + payload.len() + 32);
    encoded.extend_from_slice(ARCHIVE_V2_ROLE_MARKER_MAGIC);
    encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&payload);
    encoded.extend_from_slice(&Hash::hash(&payload).0);
    let parent = path
        .parent()
        .ok_or_else(|| "Archive V2 role marker has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed creating {}: {error}", parent.display()))?;
    let temporary = parent.join(format!(
        ".role-config.{}.{}.tmp",
        std::process::id(),
        ARCHIVE_V2_ROLE_MARKER_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("failed creating {}: {error}", temporary.display()))?;
    let result = (|| {
        file.write_all(&encoded)
            .map_err(|error| format!("failed writing role marker: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed syncing role marker: {error}"))?;
        drop(file);
        fs::hard_link(&temporary, path).map_err(|error| {
            format!(
                "failed publishing create-new role marker {}: {error}",
                path.display()
            )
        })?;
        fs::remove_file(&temporary)
            .map_err(|error| format!("failed removing role marker staging file: {error}"))?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("failed syncing role marker directory: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_v2::{ArchiveV2Role, ARCHIVE_V2_ROLE_CONFIG_VERSION};

    fn marker() -> ArchiveV2RoleMarker {
        ArchiveV2RoleMarker {
            marker_version: 1,
            identity: ArchiveV2Identity {
                network_id: "marker-testnet".to_string(),
                genesis_hash: Hash::hash(b"marker-genesis"),
            },
            role_config: ArchiveV2RoleConfig {
                version: ARCHIVE_V2_ROLE_CONFIG_VERSION,
                role: ArchiveV2Role::Consensus,
                recent_history_slots: 200_000,
                verified_cache_quota_bytes: 0,
                advertise_deep_history: false,
            },
            genesis_mossstake_slot_only: true,
        }
    }

    #[test]
    fn role_marker_is_checksummed_and_roundtrips() {
        let root = tempfile::tempdir().expect("create role marker root");
        let path = root.path().join(ARCHIVE_V2_ROLE_MARKER_FILENAME);
        let marker = marker();
        store_archive_v2_role_marker_create_new(&path, &marker).unwrap();
        assert_eq!(load_archive_v2_role_marker(&path).unwrap(), marker);

        let mut damaged = fs::read(&path).unwrap();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        fs::write(&path, damaged).unwrap();
        assert!(load_archive_v2_role_marker(&path)
            .unwrap_err()
            .contains("checksum"));
    }

    #[test]
    fn role_marker_publish_never_overwrites_existing_authorization() {
        let root = tempfile::tempdir().expect("create role marker root");
        let path = root.path().join(ARCHIVE_V2_ROLE_MARKER_FILENAME);
        let first = marker();
        store_archive_v2_role_marker_create_new(&path, &first).unwrap();

        let mut conflicting = first.clone();
        conflicting.role_config.role = ArchiveV2Role::FullArchive;
        assert!(store_archive_v2_role_marker_create_new(&path, &conflicting).is_err());
        assert_eq!(load_archive_v2_role_marker(&path).unwrap(), first);
    }

    #[cfg(unix)]
    #[test]
    fn role_marker_reader_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create role marker root");
        let target = root.path().join("target.bin");
        let link = root.path().join(ARCHIVE_V2_ROLE_MARKER_FILENAME);
        store_archive_v2_role_marker_create_new(&target, &marker()).unwrap();
        symlink(&target, &link).unwrap();
        assert!(load_archive_v2_role_marker(&link)
            .unwrap_err()
            .contains("regular file"));
    }
}
