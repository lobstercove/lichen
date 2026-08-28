use crate::merkle::{root_for_file, MerkleAccumulator};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
pub struct ObjectRecord {
    pub hash: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

#[derive(Debug, Clone, Copy)]
pub struct PutResult {
    pub created: bool,
    pub size: u64,
}

pub struct ContentStore {
    root: PathBuf,
    max_object_bytes: u64,
    max_total_bytes: u64,
    stored_bytes: AtomicU64,
    temp_counter: AtomicU64,
}

pub fn decode_hash(hash: &str) -> Result<[u8; 32], String> {
    let decoded = bs58::decode(hash)
        .into_vec()
        .map_err(|_| "content hash is not canonical base58".to_string())?;
    if decoded.len() != 32 || bs58::encode(&decoded).into_string() != hash {
        return Err("content hash must be a canonical 32-byte base58 value".to_string());
    }
    let mut result = [0u8; 32];
    result.copy_from_slice(&decoded);
    Ok(result)
}

fn object_path(root: &Path, hash: &str, decoded: &[u8; 32]) -> PathBuf {
    root.join(format!("{:02x}", decoded[0]))
        .join(format!("{:02x}", decoded[1]))
        .join(hash)
}

impl ContentStore {
    pub async fn open(
        root: PathBuf,
        max_object_bytes: u64,
        max_total_bytes: u64,
    ) -> Result<Self, String> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|error| format!("create Moss data directory: {error}"))?;
        let scan_root = root.clone();
        let records = tokio::task::spawn_blocking(move || scan_records(&scan_root))
            .await
            .map_err(|error| format!("join Moss startup scan: {error}"))??;
        let stored_bytes = records.iter().try_fold(0u64, |total, record| {
            total
                .checked_add(record.size)
                .ok_or_else(|| "Moss stored-byte counter overflow".to_string())
        })?;
        if stored_bytes > max_total_bytes {
            return Err(format!(
                "Moss data directory uses {stored_bytes} bytes, above configured {max_total_bytes}"
            ));
        }
        Ok(Self {
            root,
            max_object_bytes,
            max_total_bytes,
            stored_bytes: AtomicU64::new(stored_bytes),
            temp_counter: AtomicU64::new(1),
        })
    }

    pub fn stored_bytes(&self) -> u64 {
        self.stored_bytes.load(Ordering::Acquire)
    }

    pub fn path_for(&self, hash: &str) -> Result<PathBuf, String> {
        let decoded = decode_hash(hash)?;
        Ok(object_path(&self.root, hash, &decoded))
    }

    fn reserve(&self, size: u64) -> Result<(), String> {
        if size == 0 || size > self.max_object_bytes {
            return Err(format!(
                "object size must be between 1 and {} bytes",
                self.max_object_bytes
            ));
        }
        self.stored_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(size)
                    .filter(|next| *next <= self.max_total_bytes)
            })
            .map(|_| ())
            .map_err(|_| "Moss provider storage quota exceeded".to_string())
    }

    fn release(&self, size: u64) {
        let _ = self
            .stored_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_sub(size)
            });
    }

    pub async fn put_stream<S, E>(
        &self,
        hash: &str,
        declared_size: u64,
        mut stream: S,
    ) -> Result<PutResult, String>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let expected_root = decode_hash(hash)?;
        let target = self.path_for(hash)?;
        if let Ok(metadata) = tokio::fs::metadata(&target).await {
            if !metadata.is_file() || metadata.len() != declared_size {
                return Err("existing Moss object conflicts with upload".to_string());
            }
            let (actual_root, actual_size) = root_for_file(&target).await?;
            if actual_root != expected_root || actual_size != declared_size {
                return Err("existing Moss object failed integrity verification".to_string());
            }
            return Ok(PutResult {
                created: false,
                size: declared_size,
            });
        }

        self.reserve(declared_size)?;
        let parent = target
            .parent()
            .ok_or_else(|| "invalid Moss object path".to_string())?;
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            self.release(declared_size);
            return Err(format!("create Moss object shard: {error}"));
        }
        let temp_id = self.temp_counter.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".{hash}.{}.{}.tmp", std::process::id(), temp_id));
        let result = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.create_new(true).write(true);
            let mut file = options
                .open(&temp)
                .await
                .map_err(|error| format!("create Moss upload: {error}"))?;
            let mut accumulator = MerkleAccumulator::default();
            let mut written = 0u64;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| format!("read upload body: {error}"))?;
                written = written
                    .checked_add(chunk.len() as u64)
                    .ok_or_else(|| "upload size overflow".to_string())?;
                if written > declared_size || written > self.max_object_bytes {
                    return Err("upload exceeds declared or configured size".to_string());
                }
                accumulator.update(&chunk)?;
                file.write_all(&chunk)
                    .await
                    .map_err(|error| format!("write Moss upload: {error}"))?;
            }
            if written != declared_size {
                return Err(format!(
                    "upload size mismatch: declared {declared_size}, received {written}"
                ));
            }
            let (actual_root, actual_size) = accumulator.finish()?;
            if actual_root != expected_root || actual_size != declared_size {
                return Err("upload content does not match its Moss commitment".to_string());
            }
            file.sync_all()
                .await
                .map_err(|error| format!("sync Moss upload: {error}"))?;
            drop(file);

            match tokio::fs::hard_link(&temp, &target).await {
                Ok(()) => {
                    sync_directory(parent.to_path_buf()).await?;
                    Ok(PutResult {
                        created: true,
                        size: declared_size,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = tokio::fs::metadata(&target).await.map_err(|meta_error| {
                        format!("inspect concurrent Moss upload: {meta_error}")
                    })?;
                    if !metadata.is_file() || metadata.len() != declared_size {
                        return Err("concurrent Moss object conflicts with upload".to_string());
                    }
                    Ok(PutResult {
                        created: false,
                        size: declared_size,
                    })
                }
                Err(error) => Err(format!("publish Moss object atomically: {error}")),
            }
        }
        .await;

        let _ = tokio::fs::remove_file(&temp).await;
        match result {
            Ok(put) if put.created => Ok(put),
            Ok(put) => {
                self.release(declared_size);
                Ok(put)
            }
            Err(error) => {
                self.release(declared_size);
                Err(error)
            }
        }
    }

    pub async fn list(&self) -> Result<Vec<ObjectRecord>, String> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || scan_records(&root))
            .await
            .map_err(|error| format!("join Moss object scan: {error}"))?
    }

    pub async fn remove(&self, hash: &str) -> Result<bool, String> {
        let path = self.path_for(hash)?;
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Err("Moss object path is not a regular file".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("inspect Moss object: {error}")),
        };
        tokio::fs::remove_file(&path)
            .await
            .map_err(|error| format!("remove Moss object: {error}"))?;
        for marker in [
            "confirmed",
            "confirm_submitted",
            "proof_submitted",
            "close_submitted",
            "verified",
            "meta",
        ] {
            let _ = tokio::fs::remove_file(path.with_extension(marker)).await;
        }
        self.release(metadata.len());
        if let Some(parent) = path.parent() {
            sync_directory(parent.to_path_buf()).await?;
        }
        Ok(true)
    }

    pub async fn mark(&self, hash: &str, marker: &str, value: &[u8]) -> Result<(), String> {
        if !matches!(
            marker,
            "confirmed" | "confirm_submitted" | "proof_submitted" | "close_submitted" | "verified"
        ) {
            return Err("unsupported Moss marker".to_string());
        }
        let path = self.path_for(hash)?.with_extension(marker);
        tokio::fs::write(&path, value)
            .await
            .map_err(|error| format!("write Moss marker: {error}"))?;
        Ok(())
    }

    pub async fn has_marker(&self, hash: &str, marker: &str) -> bool {
        let Some(path) = self
            .path_for(hash)
            .ok()
            .map(|path| path.with_extension(marker))
        else {
            return false;
        };
        tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
    }

    pub async fn marker_is_recent(
        &self,
        hash: &str,
        marker: &str,
        duration: std::time::Duration,
    ) -> bool {
        let Some(path) = self
            .path_for(hash)
            .ok()
            .map(|path| path.with_extension(marker))
        else {
            return false;
        };
        tokio::fs::metadata(path)
            .await
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < duration)
    }

    pub async fn set_content_type(&self, hash: &str, content_type: &str) -> Result<(), String> {
        let path = self.path_for(hash)?.with_extension("meta");
        let metadata = serde_json::to_vec(&serde_json::json!({ "content_type": content_type }))
            .map_err(|error| format!("encode Moss object metadata: {error}"))?;
        tokio::fs::write(path, metadata)
            .await
            .map_err(|error| format!("write Moss object metadata: {error}"))
    }

    pub async fn content_type(&self, hash: &str) -> String {
        let Some(path) = self
            .path_for(hash)
            .ok()
            .map(|path| path.with_extension("meta"))
        else {
            return "application/octet-stream".to_string();
        };
        tokio::fs::read(path)
            .await
            .ok()
            .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
            .and_then(|value| value.get("content_type")?.as_str().map(str::to_string))
            .unwrap_or_else(|| "application/octet-stream".to_string())
    }
}

async fn sync_directory(path: PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync Moss directory: {error}"))
    })
    .await
    .map_err(|error| format!("join Moss directory sync: {error}"))?
}

fn scan_records(root: &Path) -> Result<Vec<ObjectRecord>, String> {
    let mut records = Vec::new();
    let first_level =
        std::fs::read_dir(root).map_err(|error| format!("scan Moss data directory: {error}"))?;
    for first in first_level {
        let first = first.map_err(|error| format!("scan Moss shard: {error}"))?;
        if !first
            .file_type()
            .map_err(|error| format!("inspect Moss shard: {error}"))?
            .is_dir()
        {
            continue;
        }
        for second in
            std::fs::read_dir(first.path()).map_err(|error| format!("scan Moss shard: {error}"))?
        {
            let second = second.map_err(|error| format!("scan Moss shard: {error}"))?;
            if !second
                .file_type()
                .map_err(|error| format!("inspect Moss shard: {error}"))?
                .is_dir()
            {
                continue;
            }
            for entry in std::fs::read_dir(second.path())
                .map_err(|error| format!("scan Moss objects: {error}"))?
            {
                let entry = entry.map_err(|error| format!("scan Moss object: {error}"))?;
                let metadata = entry
                    .metadata()
                    .map_err(|error| format!("inspect Moss object: {error}"))?;
                if !metadata.is_file() {
                    continue;
                }
                let hash = entry.file_name().to_string_lossy().to_string();
                if hash.starts_with('.') || hash.contains('.') || decode_hash(&hash).is_err() {
                    continue;
                }
                records.push(ObjectRecord {
                    hash,
                    path: entry.path(),
                    size: metadata.len(),
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merkle::MerkleAccumulator;
    use futures_util::stream;

    fn commitment(data: &[u8]) -> String {
        let mut accumulator = MerkleAccumulator::default();
        accumulator.update(data).unwrap();
        bs58::encode(accumulator.finish().unwrap().0).into_string()
    }

    #[tokio::test]
    async fn upload_is_verified_atomic_and_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContentStore::open(directory.path().to_path_buf(), 1_000, 2_000)
            .await
            .unwrap();
        let data = Bytes::from_static(b"moss object");
        let hash = commitment(&data);
        let first = store
            .put_stream(
                &hash,
                data.len() as u64,
                stream::iter(vec![Ok::<_, String>(data.clone())]),
            )
            .await
            .unwrap();
        assert!(first.created);
        assert_eq!(store.stored_bytes(), data.len() as u64);

        let second = store
            .put_stream(
                &hash,
                data.len() as u64,
                stream::iter(vec![Ok::<_, String>(data)]),
            )
            .await
            .unwrap();
        assert!(!second.created);
        assert_eq!(store.stored_bytes(), first.size);
    }

    #[tokio::test]
    async fn upload_rejects_commitment_mismatch_without_leaking_quota() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContentStore::open(directory.path().to_path_buf(), 1_000, 2_000)
            .await
            .unwrap();
        let data = Bytes::from_static(b"wrong object");
        let wrong_hash = commitment(b"different");
        assert!(store
            .put_stream(
                &wrong_hash,
                data.len() as u64,
                stream::iter(vec![Ok::<_, String>(data)]),
            )
            .await
            .is_err());
        assert_eq!(store.stored_bytes(), 0);
        assert!(store.list().await.unwrap().is_empty());
    }
}
