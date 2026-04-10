// Lichen Consensus WAL (Write-Ahead Log)
//
// Persists consensus state so that after a crash the validator does NOT
// violate the Tendermint safety invariant (locked-value rule).
//
// What is persisted:
//   - The locked (round, value) pair whenever the validator locks.
//   - The current height to skip replaying completed heights.
//   - Commit decisions so incomplete commits can be retried.
//
// On startup the WAL is replayed: if there is a persisted lock, it is
// restored into the ConsensusEngine before the first round begins.
//
// The WAL is a simple append-only bincode file. After a commit is
// applied, the WAL is truncated (checkpointed) because the committed
// block is the new source of truth.

use lichen_core::Hash;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

/// A single WAL entry. Entries are appended; only the latest state matters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalEntry {
    /// Consensus started for a new height.
    HeightStarted { height: u64 },
    /// Validator locked on a value (Tendermint safety-critical state).
    Locked {
        height: u64,
        round: u32,
        block_hash: Hash,
    },
    /// Validator decided to commit (2/3+ precommits observed).
    CommitDecision {
        height: u64,
        round: u32,
        block_hash: Hash,
    },
    /// Commit was applied and persisted — WAL can be truncated.
    Checkpoint { height: u64 },
}

/// Consensus WAL backed by a file on disk.
pub struct ConsensusWal {
    path: PathBuf,
    /// In-memory buffer of entries since last checkpoint.
    entries: Vec<WalEntry>,
}

impl ConsensusWal {
    /// Open or create a WAL file at the given path.
    pub fn open(data_dir: &str) -> Self {
        let path = Path::new(data_dir).join("consensus.wal");
        let entries = if path.exists() {
            match fs::read(&path) {
                Ok(data) if !data.is_empty() => Self::decode_entries(&data),
                Ok(_) => Vec::new(),
                Err(e) => {
                    warn!("⚠️ WAL: Failed to read {}: {}", path.display(), e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        if !entries.is_empty() {
            info!(
                "📋 WAL: Loaded {} entries from {}",
                entries.len(),
                path.display()
            );
        }
        Self { path, entries }
    }

    /// AUDIT-FIX MED-02: Compute 4-byte checksum (first 4 bytes of SHA-256).
    fn checksum(data: &[u8]) -> [u8; 4] {
        let hash = Sha256::digest(data);
        [hash[0], hash[1], hash[2], hash[3]]
    }

    /// Decode a sequence of length-prefixed bincode entries with checksum verification.
    /// Format per entry: [len:4 LE][payload:len][checksum:4]
    /// AUDIT-FIX MED-02: Entries without a valid checksum are rejected.
    fn decode_entries(data: &[u8]) -> Vec<WalEntry> {
        let mut entries = Vec::new();
        let mut cursor = 0;
        while cursor + 4 <= data.len() {
            let len = u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]) as usize;
            cursor += 4;
            if cursor + len + 4 > data.len() {
                warn!(
                    "⚠️ WAL: Truncated entry at offset {}, stopping replay",
                    cursor - 4
                );
                break;
            }
            let payload = &data[cursor..cursor + len];
            let stored_checksum = [
                data[cursor + len],
                data[cursor + len + 1],
                data[cursor + len + 2],
                data[cursor + len + 3],
            ];
            let computed = Self::checksum(payload);
            if stored_checksum != computed {
                error!(
                    "🛑 WAL: Checksum mismatch at offset {} (stored {:02x?} != computed {:02x?}) — WAL may be corrupted, stopping replay",
                    cursor - 4, stored_checksum, computed
                );
                break;
            }
            match bincode::deserialize::<WalEntry>(payload) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    warn!("⚠️ WAL: Failed to decode entry at offset {}: {}", cursor, e);
                    break;
                }
            }
            cursor += len + 4;
        }
        entries
    }

    /// Append an entry to the WAL and flush to disk.
    pub fn append(&mut self, entry: WalEntry) {
        // Serialize entry
        let encoded = match bincode::serialize(&entry) {
            Ok(e) => e,
            Err(e) => {
                error!("WAL: Failed to serialize entry: {}", e);
                return;
            }
        };

        // Append length-prefixed entry to file
        let mut file = match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) => {
                error!("WAL: Failed to open {}: {}", self.path.display(), e);
                return;
            }
        };

        let len_bytes = (encoded.len() as u32).to_le_bytes();
        let checksum = Self::checksum(&encoded);
        if let Err(e) = file
            .write_all(&len_bytes)
            .and_then(|_| file.write_all(&encoded))
            .and_then(|_| file.write_all(&checksum))
            .and_then(|_| file.sync_all())
        {
            error!("WAL: Failed to write entry: {}", e);
            return;
        }

        self.entries.push(entry);
        debug!("📋 WAL: Appended entry (total: {})", self.entries.len());
    }

    /// Record that consensus started for a new height.
    pub fn log_height_start(&mut self, height: u64) {
        self.append(WalEntry::HeightStarted { height });
    }

    /// Record that the validator locked on a value.
    pub fn log_lock(&mut self, height: u64, round: u32, block_hash: Hash) {
        self.append(WalEntry::Locked {
            height,
            round,
            block_hash,
        });
    }

    /// Record a commit decision.
    pub fn log_commit_decision(&mut self, height: u64, round: u32, block_hash: Hash) {
        self.append(WalEntry::CommitDecision {
            height,
            round,
            block_hash,
        });
    }

    /// Checkpoint: the commit for `height` was applied. Truncate the WAL
    /// since all prior state is now durably stored in the block DB.
    pub fn checkpoint(&mut self, height: u64) {
        self.entries.clear();
        // Write a single checkpoint entry (effectively truncates the file)
        match fs::File::create(&self.path) {
            Ok(mut f) => {
                let entry = WalEntry::Checkpoint { height };
                if let Ok(encoded) = bincode::serialize(&entry) {
                    let len_bytes = (encoded.len() as u32).to_le_bytes();
                    let checksum = Self::checksum(&encoded);
                    if let Err(e) = f
                        .write_all(&len_bytes)
                        .and_then(|_| f.write_all(&encoded))
                        .and_then(|_| f.write_all(&checksum))
                        .and_then(|_| f.sync_all())
                    {
                        error!(
                            "WAL: Failed to write checkpoint data at height {}: {}",
                            height, e
                        );
                    }
                }
                self.entries.push(entry);
            }
            Err(e) => {
                error!("WAL: Failed to create checkpoint: {}", e);
            }
        }
        debug!("📋 WAL: Checkpoint at height {}", height);
    }

    /// Replay the WAL to recover locked state after a crash.
    ///
    /// Returns:
    /// - The last locked (height, round, block_hash) if any
    /// - The last checkpoint height
    pub fn recover(&self) -> WalRecovery {
        let mut last_lock: Option<(u64, u32, Hash)> = None;
        let mut last_checkpoint: Option<u64> = None;
        let mut last_height_started: Option<u64> = None;

        for entry in &self.entries {
            match entry {
                WalEntry::HeightStarted { height } => {
                    last_height_started = Some(*height);
                }
                WalEntry::Locked {
                    height,
                    round,
                    block_hash,
                } => {
                    // Only keep the lock if it's for the latest height
                    if last_checkpoint.is_none_or(|cp| *height > cp) {
                        last_lock = Some((*height, *round, *block_hash));
                    }
                }
                WalEntry::CommitDecision { .. } => {
                    // Commit was decided but may not have been applied
                }
                WalEntry::Checkpoint { height } => {
                    last_checkpoint = Some(*height);
                    // Lock is superseded by checkpoint
                    if let Some((lock_h, _, _)) = last_lock {
                        if lock_h <= *height {
                            last_lock = None;
                        }
                    }
                }
            }
        }

        WalRecovery {
            locked_state: last_lock,
            last_checkpoint,
            last_height_started,
        }
    }
}

/// Recovery state extracted from the WAL after a restart.
#[derive(Debug)]
pub struct WalRecovery {
    /// If the validator was locked before crashing: (height, round, block_hash).
    pub locked_state: Option<(u64, u32, Hash)>,
    /// Last height that was checkpointed (fully committed).
    pub last_checkpoint: Option<u64>,
    /// Last height that consensus started for (may not have committed).
    pub last_height_started: Option<u64>,
}
