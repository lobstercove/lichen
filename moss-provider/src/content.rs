use crate::merkle::{root_for_file, MerkleAccumulator};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssignmentRecord {
    pub storage_id: String,
    pub owner: String,
    pub size: u64,
    #[serde(skip, default = "system_time_epoch")]
    pub modified: SystemTime,
}

fn system_time_epoch() -> SystemTime {
    SystemTime::UNIX_EPOCH
}

#[derive(Debug, Clone)]
pub struct ObjectRecord {
    pub hash: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub assignments: Vec<AssignmentRecord>,
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
    pending_assignment_bytes: AtomicU64,
    temp_counter: AtomicU64,
    mutation_lock: Mutex<()>,
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
        let pending_assignment_bytes = scan_pending_assignment_bytes(&root)?;
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
            pending_assignment_bytes: AtomicU64::new(pending_assignment_bytes),
            temp_counter: AtomicU64::new(1),
            mutation_lock: Mutex::new(()),
        })
    }

    pub fn stored_bytes(&self) -> u64 {
        self.stored_bytes.load(Ordering::Acquire)
    }

    pub fn pending_assignment_bytes(&self) -> u64 {
        self.pending_assignment_bytes.load(Ordering::Acquire)
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

    fn release_pending_assignment(&self, size: u64) -> Result<(), String> {
        self.pending_assignment_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_sub(size)
            })
            .map(|_| ())
            .map_err(|_| "Moss pending assignment counter underflow".to_string())
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

    pub async fn add_assignment(
        &self,
        hash: &str,
        storage_id: &str,
        owner: &str,
        size: u64,
        confirmed_used: u64,
        capacity: u64,
    ) -> Result<bool, String> {
        decode_hash(hash)?;
        decode_hash(storage_id)?;
        if owner.is_empty() || owner.len() > 128 {
            return Err("Moss assignment owner is invalid".to_string());
        }
        let _guard = self.mutation_lock.lock().await;
        let object = self.path_for(hash)?;
        let object_metadata = tokio::fs::metadata(&object)
            .await
            .map_err(|error| format!("inspect Moss object before assignment: {error}"))?;
        if !object_metadata.is_file() || object_metadata.len() != size {
            return Err("Moss assignment size conflicts with its object".to_string());
        }
        let directory = object.with_extension("assignments");
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("create Moss assignment directory: {error}"))?;
        let target = directory.join(storage_id);
        let encoded = serde_json::to_vec(&AssignmentRecord {
            storage_id: storage_id.to_string(),
            owner: owner.to_string(),
            size,
            modified: SystemTime::UNIX_EPOCH,
        })
        .map_err(|error| format!("encode Moss assignment: {error}"))?;
        if let Ok(existing) = tokio::fs::read(&target).await {
            if existing != encoded {
                return Err("existing Moss assignment conflicts with upload".to_string());
            }
            return Ok(false);
        }
        let pending = self.pending_assignment_bytes();
        if confirmed_used
            .checked_add(pending)
            .and_then(|value| value.checked_add(size))
            .is_none_or(|committed| committed > capacity)
        {
            return Err("Moss provider logical assignment capacity exceeded".to_string());
        }
        let temp_id = self.temp_counter.fetch_add(1, Ordering::Relaxed);
        let temp = directory.join(format!(
            ".{storage_id}.{}.{}.tmp",
            std::process::id(),
            temp_id
        ));
        let result = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.create_new(true).write(true);
            let mut file = options
                .open(&temp)
                .await
                .map_err(|error| format!("create Moss assignment: {error}"))?;
            file.write_all(&encoded)
                .await
                .map_err(|error| format!("write Moss assignment: {error}"))?;
            file.sync_all()
                .await
                .map_err(|error| format!("sync Moss assignment: {error}"))?;
            drop(file);
            match tokio::fs::hard_link(&temp, &target).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = tokio::fs::read(&target)
                        .await
                        .map_err(|read_error| format!("read Moss assignment: {read_error}"))?;
                    if existing == encoded {
                        Ok(())
                    } else {
                        Err("concurrent Moss assignment conflicts with upload".to_string())
                    }
                }
                Err(error) => Err(format!("publish Moss assignment atomically: {error}")),
            }
        }
        .await;
        let _ = tokio::fs::remove_file(&temp).await;
        result?;
        self.pending_assignment_bytes
            .fetch_add(size, Ordering::AcqRel);
        sync_directory(directory).await?;
        Ok(true)
    }

    pub async fn remove_assignment(&self, hash: &str, storage_id: &str) -> Result<bool, String> {
        decode_hash(storage_id)?;
        let _guard = self.mutation_lock.lock().await;
        let directory = self.path_for(hash)?.with_extension("assignments");
        let target = directory.join(storage_id);
        let assignment = match tokio::fs::read(&target).await {
            Ok(data) => Some(
                serde_json::from_slice::<AssignmentRecord>(&data)
                    .map_err(|error| format!("decode Moss assignment before removal: {error}"))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("read Moss assignment before removal: {error}")),
        };
        let was_pending = tokio::fs::metadata(directory.join(format!("{storage_id}.confirmed")))
            .await
            .is_err();
        let removed = match tokio::fs::remove_file(&target).await {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(format!("remove Moss assignment: {error}")),
        };
        for marker in [
            "confirmed",
            "confirm_submitted",
            "proof_submitted",
            "close_submitted",
        ] {
            let _ = tokio::fs::remove_file(directory.join(format!("{storage_id}.{marker}"))).await;
        }
        if removed {
            if was_pending {
                if let Some(assignment) = assignment {
                    self.release_pending_assignment(assignment.size)?;
                }
            }
            sync_directory(directory).await?;
        }
        Ok(removed)
    }

    pub async fn remove(&self, hash: &str) -> Result<bool, String> {
        let _guard = self.mutation_lock.lock().await;
        let path = self.path_for(hash)?;
        let assignment_directory = path.with_extension("assignments");
        if directory_has_assignments(&assignment_directory)? {
            return Ok(false);
        }
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Err("Moss object path is not a regular file".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("inspect Moss object: {error}")),
        };
        tokio::fs::remove_file(&path)
            .await
            .map_err(|error| format!("remove Moss object: {error}"))?;
        for marker in ["verified", "meta"] {
            let _ = tokio::fs::remove_file(path.with_extension(marker)).await;
        }
        let _ = tokio::fs::remove_dir_all(&assignment_directory).await;
        self.release(metadata.len());
        if let Some(parent) = path.parent() {
            sync_directory(parent.to_path_buf()).await?;
        }
        Ok(true)
    }

    pub async fn mark(&self, hash: &str, marker: &str, value: &[u8]) -> Result<(), String> {
        if marker != "verified" {
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

    pub async fn mark_assignment(
        &self,
        hash: &str,
        storage_id: &str,
        marker: &str,
        value: &[u8],
    ) -> Result<(), String> {
        if !matches!(
            marker,
            "confirmed" | "confirm_submitted" | "proof_submitted" | "close_submitted"
        ) {
            return Err("unsupported Moss assignment marker".to_string());
        }
        decode_hash(storage_id)?;
        let directory = self.path_for(hash)?.with_extension("assignments");
        let path = directory.join(format!("{storage_id}.{marker}"));
        if marker == "confirmed" {
            let _guard = self.mutation_lock.lock().await;
            let assignment = tokio::fs::read(directory.join(storage_id))
                .await
                .map_err(|error| format!("read confirmed Moss assignment: {error}"))?;
            let assignment: AssignmentRecord = serde_json::from_slice(&assignment)
                .map_err(|error| format!("decode confirmed Moss assignment: {error}"))?;
            let mut options = tokio::fs::OpenOptions::new();
            options.create_new(true).write(true);
            match options.open(&path).await {
                Ok(mut file) => {
                    file.write_all(value)
                        .await
                        .map_err(|error| format!("write Moss assignment marker: {error}"))?;
                    file.sync_all()
                        .await
                        .map_err(|error| format!("sync Moss assignment marker: {error}"))?;
                    self.release_pending_assignment(assignment.size)?;
                    sync_directory(directory).await?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
                Err(error) => return Err(format!("create Moss assignment marker: {error}")),
            }
        } else {
            tokio::fs::write(&path, value)
                .await
                .map_err(|error| format!("write Moss assignment marker: {error}"))?;
        }
        Ok(())
    }

    pub async fn has_assignment_marker(&self, hash: &str, storage_id: &str, marker: &str) -> bool {
        let Ok(path) = self.path_for(hash) else {
            return false;
        };
        tokio::fs::metadata(
            path.with_extension("assignments")
                .join(format!("{storage_id}.{marker}")),
        )
        .await
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
    }

    pub async fn assignment_marker_is_recent(
        &self,
        hash: &str,
        storage_id: &str,
        marker: &str,
        duration: std::time::Duration,
    ) -> bool {
        let Ok(path) = self.path_for(hash) else {
            return false;
        };
        tokio::fs::metadata(
            path.with_extension("assignments")
                .join(format!("{storage_id}.{marker}")),
        )
        .await
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < duration)
    }

    pub async fn set_content_type(&self, hash: &str, content_type: &str) -> Result<(), String> {
        let _guard = self.mutation_lock.lock().await;
        let path = self.path_for(hash)?.with_extension("meta");
        let metadata = serde_json::to_vec(&serde_json::json!({ "content_type": content_type }))
            .map_err(|error| format!("encode Moss object metadata: {error}"))?;
        if let Ok(existing) = tokio::fs::read(&path).await {
            return if existing == metadata {
                Ok(())
            } else {
                Err("existing Moss content type conflicts with upload".to_string())
            };
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        match options.open(&path).await {
            Ok(mut file) => {
                file.write_all(&metadata)
                    .await
                    .map_err(|error| format!("write Moss object metadata: {error}"))?;
                file.sync_all()
                    .await
                    .map_err(|error| format!("sync Moss object metadata: {error}"))?;
                if let Some(parent) = path.parent() {
                    sync_directory(parent.to_path_buf()).await?;
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = tokio::fs::read(&path)
                    .await
                    .map_err(|read_error| format!("read Moss object metadata: {read_error}"))?;
                if existing == metadata {
                    Ok(())
                } else {
                    Err("concurrent Moss content type conflicts with upload".to_string())
                }
            }
            Err(error) => Err(format!("create Moss object metadata: {error}")),
        }
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
                let assignments = scan_assignments(&entry.path())?;
                if assignments
                    .iter()
                    .any(|assignment| assignment.size != metadata.len())
                {
                    return Err("Moss assignment size conflicts with its object".to_string());
                }
                records.push(ObjectRecord {
                    hash,
                    path: entry.path(),
                    size: metadata.len(),
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    assignments,
                });
            }
        }
    }
    Ok(records)
}

fn scan_assignments(object_path: &Path) -> Result<Vec<AssignmentRecord>, String> {
    let directory = object_path.with_extension("assignments");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("scan Moss assignment directory: {error}")),
    };
    let mut assignments = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("scan Moss assignment: {error}"))?;
        let metadata = entry
            .metadata()
            .map_err(|error| format!("inspect Moss assignment: {error}"))?;
        let storage_id = entry.file_name().to_string_lossy().to_string();
        if !metadata.is_file() || storage_id.contains('.') || decode_hash(&storage_id).is_err() {
            continue;
        }
        let data = std::fs::read(entry.path())
            .map_err(|error| format!("read Moss assignment: {error}"))?;
        let mut assignment: AssignmentRecord = serde_json::from_slice(&data)
            .map_err(|error| format!("decode Moss assignment: {error}"))?;
        if assignment.storage_id != storage_id || assignment.owner.is_empty() {
            return Err("Moss assignment metadata is inconsistent".to_string());
        }
        assignment.modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        assignments.push(assignment);
    }
    Ok(assignments)
}

fn scan_pending_assignment_bytes(root: &Path) -> Result<u64, String> {
    let mut total = 0u64;
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
                .map_err(|error| format!("scan Moss assignment directories: {error}"))?
            {
                let entry =
                    entry.map_err(|error| format!("scan Moss assignment directory: {error}"))?;
                let name = entry.file_name().to_string_lossy().to_string();
                if !entry
                    .file_type()
                    .map_err(|error| format!("inspect Moss assignment directory: {error}"))?
                    .is_dir()
                    || !name.ends_with(".assignments")
                {
                    continue;
                }
                for assignment in std::fs::read_dir(entry.path())
                    .map_err(|error| format!("scan Moss assignments: {error}"))?
                {
                    let assignment =
                        assignment.map_err(|error| format!("scan Moss assignment: {error}"))?;
                    let storage_id = assignment.file_name().to_string_lossy().to_string();
                    if storage_id.contains('.') || decode_hash(&storage_id).is_err() {
                        continue;
                    }
                    let data = std::fs::read(assignment.path())
                        .map_err(|error| format!("read Moss assignment: {error}"))?;
                    let record: AssignmentRecord = serde_json::from_slice(&data)
                        .map_err(|error| format!("decode Moss assignment: {error}"))?;
                    if record.storage_id != storage_id || record.size == 0 {
                        return Err("Moss assignment metadata is inconsistent".to_string());
                    }
                    if !entry
                        .path()
                        .join(format!("{storage_id}.confirmed"))
                        .is_file()
                    {
                        total = total
                            .checked_add(record.size)
                            .ok_or_else(|| "Moss pending assignment bytes overflow".to_string())?;
                    }
                }
            }
        }
    }
    Ok(total)
}

fn directory_has_assignments(directory: &Path) -> Result<bool, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("scan Moss assignment directory: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("scan Moss assignment: {error}"))?;
        let storage_id = entry.file_name().to_string_lossy().to_string();
        if !storage_id.contains('.') && decode_hash(&storage_id).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
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
                stream::iter(vec![Ok::<_, String>(data.clone())]),
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

    #[tokio::test]
    async fn shared_content_keeps_distinct_owner_scoped_assignments() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContentStore::open(directory.path().to_path_buf(), 1_000, 2_000)
            .await
            .unwrap();
        let data = Bytes::from_static(b"shared moss object");
        let hash = commitment(&data);
        let storage_a = bs58::encode([1u8; 32]).into_string();
        let storage_b = bs58::encode([2u8; 32]).into_string();
        let storage_c = bs58::encode([3u8; 32]).into_string();
        let logical_capacity = data.len() as u64 * 2;

        store
            .put_stream(
                &hash,
                data.len() as u64,
                stream::iter(vec![Ok::<_, String>(data.clone())]),
            )
            .await
            .unwrap();
        assert!(store
            .add_assignment(
                &hash,
                &storage_a,
                "owner-a",
                data.len() as u64,
                0,
                logical_capacity,
            )
            .await
            .unwrap());
        store.set_content_type(&hash, "image/png").await.unwrap();
        store.set_content_type(&hash, "image/png").await.unwrap();
        assert!(store.set_content_type(&hash, "text/html").await.is_err());
        assert!(store
            .add_assignment(
                &hash,
                &storage_b,
                "owner-b",
                data.len() as u64,
                0,
                logical_capacity,
            )
            .await
            .unwrap());
        assert!(!store
            .add_assignment(
                &hash,
                &storage_a,
                "owner-a",
                data.len() as u64,
                0,
                logical_capacity,
            )
            .await
            .unwrap());
        assert!(store
            .add_assignment(
                &hash,
                &storage_c,
                "owner-c",
                data.len() as u64,
                0,
                logical_capacity,
            )
            .await
            .is_err());
        store
            .mark_assignment(&hash, &storage_a, "confirmed", b"slot")
            .await
            .unwrap();
        assert_eq!(store.pending_assignment_bytes(), data.len() as u64);
        assert!(store.remove_assignment(&hash, &storage_a).await.unwrap());
        assert!(store
            .add_assignment(
                &hash,
                &storage_c,
                "owner-c",
                data.len() as u64,
                0,
                logical_capacity,
            )
            .await
            .unwrap());

        let records = store.list().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].assignments.len(), 2);
        assert_eq!(store.pending_assignment_bytes(), data.len() as u64 * 2);
        let reopened = ContentStore::open(directory.path().to_path_buf(), 1_000, 2_000)
            .await
            .unwrap();
        assert_eq!(reopened.pending_assignment_bytes(), data.len() as u64 * 2);
        drop(reopened);
        assert!(!store.remove(&hash).await.unwrap());
        assert!(store.remove_assignment(&hash, &storage_b).await.unwrap());
        assert!(!store.remove(&hash).await.unwrap());
        assert!(store.remove_assignment(&hash, &storage_c).await.unwrap());
        assert!(store.remove(&hash).await.unwrap());
        assert_eq!(store.stored_bytes(), 0);
    }
}
