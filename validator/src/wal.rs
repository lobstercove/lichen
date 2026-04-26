// Lichen Consensus WAL (Write-Ahead Log)
//
// Persists consensus state so that after a crash the validator does NOT
// violate Tendermint safety invariants.
//
// What is persisted:
//   - The locked (round, value) pair whenever the validator locks.
//   - Signed prevote/precommit choices before they are broadcast.
//   - The current height to skip replaying completed heights.
//   - Commit decisions so incomplete commits can be retried.
//
// On startup the WAL is replayed: if there is a persisted lock, it is
// restored into the ConsensusEngine before the first round begins.
//
// The WAL is a simple append-only bincode file. After a commit is
// applied, the WAL is truncated (checkpointed) because the committed
// block is the new source of truth.

use lichen_core::{Hash, PqSignature, Precommit, Prevote, Pubkey};
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
    /// Validator signed a local prevote or precommit.
    ///
    /// This is slashing-protection state. It must be fsynced before the vote is
    /// broadcast so a restart cannot sign a conflicting vote for the same
    /// height, round, and vote type.
    SignedVote(SignedVoteRecord),
    /// Commit was applied and persisted — WAL can be truncated.
    Checkpoint { height: u64 },
}

/// BFT vote type persisted for slashing protection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignedVoteType {
    Prevote,
    Precommit,
}

/// Signed vote retained in the WAL until the height is checkpointed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedVoteRecord {
    pub height: u64,
    pub round: u32,
    pub vote_type: SignedVoteType,
    pub block_hash: Option<Hash>,
    pub validator: Pubkey,
    pub signature: PqSignature,
    /// Precommit timestamp is part of the signed message. Prevotes use `None`.
    pub timestamp: Option<u64>,
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
    fn append_result(&mut self, entry: WalEntry) -> Result<(), String> {
        // Serialize entry
        let encoded = bincode::serialize(&entry)
            .map_err(|e| format!("WAL: Failed to serialize entry: {e}"))?;

        // Append length-prefixed entry to file
        let mut file = match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) => {
                return Err(format!(
                    "WAL: Failed to open {}: {}",
                    self.path.display(),
                    e
                ));
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
            return Err(format!("WAL: Failed to write entry: {e}"));
        }

        self.entries.push(entry);
        debug!("📋 WAL: Appended entry (total: {})", self.entries.len());
        Ok(())
    }

    /// Append an entry to the WAL and flush to disk.
    pub fn append(&mut self, entry: WalEntry) {
        if let Err(e) = self.append_result(entry) {
            error!("{}", e);
        }
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

    fn ensure_no_conflicting_signed_vote(&self, record: &SignedVoteRecord) -> Result<(), String> {
        for entry in &self.entries {
            let WalEntry::SignedVote(existing) = entry else {
                continue;
            };
            if existing.height == record.height
                && existing.round == record.round
                && existing.vote_type == record.vote_type
                && existing.validator == record.validator
            {
                if existing.block_hash == record.block_hash
                    && existing.timestamp == record.timestamp
                {
                    return Ok(());
                }
                return Err(format!(
                    "WAL slashing protection: refusing conflicting {:?} for height={} round={} (existing={:?}, new={:?})",
                    record.vote_type,
                    record.height,
                    record.round,
                    existing.block_hash,
                    record.block_hash
                ));
            }
        }
        Ok(())
    }

    fn log_signed_vote(&mut self, record: SignedVoteRecord) -> Result<(), String> {
        self.ensure_no_conflicting_signed_vote(&record)?;
        self.append_result(WalEntry::SignedVote(record))
    }

    /// Persist a signed prevote before it is broadcast.
    pub fn log_signed_prevote(&mut self, prevote: &Prevote) -> Result<(), String> {
        self.log_signed_vote(SignedVoteRecord {
            height: prevote.height,
            round: prevote.round,
            vote_type: SignedVoteType::Prevote,
            block_hash: prevote.block_hash,
            validator: prevote.validator,
            signature: prevote.signature.clone(),
            timestamp: None,
        })
    }

    /// Persist a signed precommit before it is broadcast.
    pub fn log_signed_precommit(&mut self, precommit: &Precommit) -> Result<(), String> {
        self.log_signed_vote(SignedVoteRecord {
            height: precommit.height,
            round: precommit.round,
            vote_type: SignedVoteType::Precommit,
            block_hash: precommit.block_hash,
            validator: precommit.validator,
            signature: precommit.signature.clone(),
            timestamp: Some(precommit.timestamp),
        })
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
        let mut signed_votes: Vec<SignedVoteRecord> = Vec::new();

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
                WalEntry::SignedVote(record) => {
                    if last_checkpoint.is_none_or(|cp| record.height > cp) {
                        signed_votes.push(record.clone());
                    }
                }
                WalEntry::Checkpoint { height } => {
                    last_checkpoint = Some(*height);
                    // Lock is superseded by checkpoint
                    if let Some((lock_h, _, _)) = last_lock {
                        if lock_h <= *height {
                            last_lock = None;
                        }
                    }
                    signed_votes.retain(|record| record.height > *height);
                }
            }
        }

        WalRecovery {
            locked_state: last_lock,
            last_checkpoint,
            last_height_started,
            signed_votes,
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
    /// Signed local votes not superseded by a checkpoint.
    pub signed_votes: Vec<SignedVoteRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lichen_core::Keypair;

    fn temp_data_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lichen-consensus-wal-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn keypair(seed: u8) -> Keypair {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        Keypair::from_seed(&bytes)
    }

    #[test]
    fn test_signed_vote_recovery_and_checkpoint() {
        let dir = temp_data_dir("signed-vote-recovery");
        let kp = keypair(1);
        let hash = Hash::hash(b"block");
        let signable = Prevote::signable_bytes(7, 2, &Some(hash));
        let prevote = Prevote {
            height: 7,
            round: 2,
            block_hash: Some(hash),
            validator: kp.pubkey(),
            signature: kp.sign(&signable),
        };

        let mut wal = ConsensusWal::open(dir.to_str().unwrap());
        wal.log_height_start(7);
        wal.log_signed_prevote(&prevote).unwrap();

        let recovered = ConsensusWal::open(dir.to_str().unwrap()).recover();
        assert_eq!(recovered.signed_votes.len(), 1);
        assert_eq!(recovered.signed_votes[0].vote_type, SignedVoteType::Prevote);
        assert_eq!(recovered.signed_votes[0].block_hash, Some(hash));

        let mut wal = ConsensusWal::open(dir.to_str().unwrap());
        wal.checkpoint(7);
        let recovered = ConsensusWal::open(dir.to_str().unwrap()).recover();
        assert!(recovered.signed_votes.is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_signed_vote_conflict_rejected() {
        let dir = temp_data_dir("signed-vote-conflict");
        let kp = keypair(2);
        let hash_a = Hash::hash(b"a");
        let hash_b = Hash::hash(b"b");
        let vote_a = Prevote {
            height: 9,
            round: 1,
            block_hash: Some(hash_a),
            validator: kp.pubkey(),
            signature: kp.sign(&Prevote::signable_bytes(9, 1, &Some(hash_a))),
        };
        let vote_b = Prevote {
            height: 9,
            round: 1,
            block_hash: Some(hash_b),
            validator: kp.pubkey(),
            signature: kp.sign(&Prevote::signable_bytes(9, 1, &Some(hash_b))),
        };

        let mut wal = ConsensusWal::open(dir.to_str().unwrap());
        wal.log_signed_prevote(&vote_a).unwrap();
        let err = wal.log_signed_prevote(&vote_b).unwrap_err();
        assert!(err.contains("slashing protection"));

        let _ = fs::remove_dir_all(dir);
    }
}
