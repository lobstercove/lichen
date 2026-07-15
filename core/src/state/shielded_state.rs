use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShieldedStateRebuildReport {
    pub start_slot: u64,
    pub end_slot: u64,
    pub scanned_blocks: u64,
    pub scanned_transactions: u64,
    pub shielded_transactions: u64,
    pub shield_count: u64,
    pub unshield_count: u64,
    pub transfer_count: u64,
    pub commitment_count: u64,
    pub nullifier_count: u64,
    pub total_shielded: u64,
    pub merkle_root: [u8; 32],
    pub deleted_entries: u64,
    pub dry_run: bool,
}

const SHIELDED_NOTE_PAYLOAD_MAGIC: &[u8; 4] = b"LNP1";
const MAX_SHIELDED_NOTE_PAYLOAD_BYTES: usize = 4096;

type ShieldedNoteEnvelope = (Vec<u8>, Vec<u8>);

fn parse_shielded_note_envelope(
    data: &[u8],
    envelope_offset: usize,
    action: &str,
) -> Result<Option<ShieldedNoteEnvelope>, String> {
    if data.len() < envelope_offset + SHIELDED_NOTE_PAYLOAD_MAGIC.len()
        || &data[envelope_offset..envelope_offset + SHIELDED_NOTE_PAYLOAD_MAGIC.len()]
            != SHIELDED_NOTE_PAYLOAD_MAGIC
    {
        return Ok(None);
    }

    let proof_len_start = envelope_offset + SHIELDED_NOTE_PAYLOAD_MAGIC.len();
    let proof_len_end = proof_len_start
        .checked_add(4)
        .ok_or_else(|| format!("{}: proof length offset overflow", action))?;
    if data.len() < proof_len_end {
        return Err(format!(
            "{}: encrypted note payload header is truncated",
            action
        ));
    }

    let proof_len = u32::from_le_bytes(
        data[proof_len_start..proof_len_end]
            .try_into()
            .map_err(|_| format!("{}: invalid proof length encoding", action))?,
    ) as usize;
    let proof_start = proof_len_end;
    let proof_end = proof_start
        .checked_add(proof_len)
        .ok_or_else(|| format!("{}: proof length overflow", action))?;
    let note_len_start = proof_end;
    let note_len_end = note_len_start
        .checked_add(4)
        .ok_or_else(|| format!("{}: note length offset overflow", action))?;
    if data.len() < note_len_end {
        return Err(format!(
            "{}: encrypted note payload length is missing",
            action
        ));
    }

    let note_len = u32::from_le_bytes(
        data[note_len_start..note_len_end]
            .try_into()
            .map_err(|_| format!("{}: invalid note payload length encoding", action))?,
    ) as usize;
    if note_len == 0 || note_len > MAX_SHIELDED_NOTE_PAYLOAD_BYTES {
        return Err(format!(
            "{}: encrypted note payload length {} is out of bounds",
            action, note_len
        ));
    }

    let note_start = note_len_end;
    let note_end = note_start
        .checked_add(note_len)
        .ok_or_else(|| format!("{}: encrypted note payload length overflow", action))?;
    if data.len() != note_end {
        return Err(format!(
            "{}: encrypted note payload has trailing bytes",
            action
        ));
    }

    Ok(Some((
        data[proof_start..proof_end].to_vec(),
        data[note_start..note_end].to_vec(),
    )))
}

fn shield_deposit_note_payload(
    data: &[u8],
    commitment: &[u8; 32],
) -> Result<Option<Vec<u8>>, String> {
    let Some((_, payload)) = parse_shielded_note_envelope(data, 41, "Shield")? else {
        return Ok(None);
    };
    validate_note_payload("Shield", &payload, commitment)?;
    Ok(Some(payload))
}

fn shielded_transfer_note_payloads(
    data: &[u8],
    commitment_c: &[u8; 32],
    commitment_d: &[u8; 32],
) -> Result<Option<[Vec<u8>; 2]>, String> {
    let Some((_, payload)) = parse_shielded_note_envelope(data, 161, "ShieldedTransfer")? else {
        return Ok(None);
    };
    let json: serde_json::Value = serde_json::from_slice(&payload).map_err(|e| {
        format!(
            "ShieldedTransfer: encrypted output note payload is not valid JSON: {}",
            e
        )
    })?;
    let outputs = json
        .get("outputs")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            "ShieldedTransfer: encrypted output note payload requires outputs array".to_string()
        })?;
    if outputs.len() != 2 {
        return Err(
            "ShieldedTransfer: exactly two encrypted output notes are required".to_string(),
        );
    }

    let encoded_c = serde_json::to_vec(&outputs[0])
        .map_err(|e| format!("ShieldedTransfer: output note 0 encode failed: {}", e))?;
    let encoded_d = serde_json::to_vec(&outputs[1])
        .map_err(|e| format!("ShieldedTransfer: output note 1 encode failed: {}", e))?;
    validate_note_payload("ShieldedTransfer", &encoded_c, commitment_c)?;
    validate_note_payload("ShieldedTransfer", &encoded_d, commitment_d)?;
    Ok(Some([encoded_c, encoded_d]))
}

fn validate_note_payload(
    action: &str,
    payload: &[u8],
    commitment: &[u8; 32],
) -> Result<(), String> {
    let json: serde_json::Value = serde_json::from_slice(payload).map_err(|e| {
        format!(
            "{}: encrypted note payload is not valid JSON: {}",
            action, e
        )
    })?;
    let obj = json
        .as_object()
        .ok_or_else(|| format!("{}: encrypted note payload must be a JSON object", action))?;

    let encrypted_note = obj
        .get("encrypted_note")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{}: encrypted_note is required", action))?;
    if encrypted_note.is_empty() || encrypted_note.len() > MAX_SHIELDED_NOTE_PAYLOAD_BYTES {
        return Err(format!("{}: encrypted_note length is invalid", action));
    }
    if !encrypted_note.starts_with("a1:") {
        return Err(format!("{}: encrypted_note must use the a1 format", action));
    }

    let ephemeral_pk = obj
        .get("ephemeral_pk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{}: ephemeral_pk is required", action))?;
    let ephemeral_bytes = hex::decode(ephemeral_pk)
        .map_err(|e| format!("{}: ephemeral_pk is not hex: {}", action, e))?;
    if ephemeral_bytes.len() != 32 {
        return Err(format!("{}: ephemeral_pk must be 32 bytes", action));
    }

    let payload_commitment = obj
        .get("commitment")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{}: note payload commitment is required", action))?;
    let payload_commitment_bytes = hex::decode(payload_commitment)
        .map_err(|e| format!("{}: note payload commitment is not hex: {}", action, e))?;
    if payload_commitment_bytes.as_slice() != commitment {
        return Err(format!(
            "{}: note payload commitment does not match instruction",
            action
        ));
    }

    Ok(())
}

fn read_u64_le(data: &[u8], range: std::ops::Range<usize>, label: &str) -> Result<u64, String> {
    let bytes = data
        .get(range)
        .ok_or_else(|| format!("{}: missing u64 bytes", label))?;
    Ok(u64::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| format!("{}: invalid u64 encoding", label))?,
    ))
}

fn read_bytes32(data: &[u8], start: usize, label: &str) -> Result<[u8; 32], String> {
    let bytes = data
        .get(start..start + 32)
        .ok_or_else(|| format!("{}: missing 32-byte value", label))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}

impl StateStore {
    /// Insert a note commitment into the shielded commitments column family.
    pub fn insert_shielded_commitment(
        &self,
        index: u64,
        commitment: &[u8; 32],
    ) -> Result<(), String> {
        let _state_commitment_guard = self.lock_state_commitment();
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;

        self.db
            .put_cf(&cf, index.to_be_bytes(), commitment)
            .map_err(|e| format!("Failed to insert shielded commitment: {}", e))?;
        self.clear_composite_state_root_cache();
        Ok(())
    }

    /// Retrieve a commitment leaf by its insertion index.
    pub fn get_shielded_commitment(&self, index: u64) -> Result<Option<[u8; 32]>, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;

        match self.db.get_cf(&cf, index.to_be_bytes()) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err(format!(
                        "Invalid commitment length {} at index {}",
                        data.len(),
                        index
                    ));
                }
                let mut out = [0u8; 32];
                out.copy_from_slice(&data);
                Ok(Some(out))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error reading commitment: {}", e)),
        }
    }

    /// Store the encrypted note payload associated with a commitment index.
    pub fn insert_shielded_note_payload(&self, index: u64, payload: &[u8]) -> Result<(), String> {
        let _state_commitment_guard = self.lock_state_commitment();
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NOTE_PAYLOADS)
            .ok_or_else(|| "Shielded note payloads CF not found".to_string())?;

        self.db
            .put_cf(&cf, index.to_be_bytes(), payload)
            .map_err(|e| format!("Failed to insert shielded note payload: {}", e))?;
        self.clear_composite_state_root_cache();
        Ok(())
    }

    /// Retrieve the encrypted note payload associated with a commitment index.
    pub fn get_shielded_note_payload(&self, index: u64) -> Result<Option<Vec<u8>>, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NOTE_PAYLOADS)
            .ok_or_else(|| "Shielded note payloads CF not found".to_string())?;

        self.db
            .get_cf(&cf, index.to_be_bytes())
            .map_err(|e| format!("Database error reading shielded note payload: {}", e))
    }

    /// Check whether a nullifier has been spent.
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;

        match self.db.get_cf(&cf, nullifier) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error checking nullifier: {}", e)),
        }
    }

    /// Mark a nullifier as spent.
    pub fn mark_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<(), String> {
        let _state_commitment_guard = self.lock_state_commitment();
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;

        self.db
            .put_cf(&cf, nullifier, [0x01])
            .map_err(|e| format!("Failed to mark nullifier spent: {}", e))?;
        self.clear_composite_state_root_cache();
        Ok(())
    }

    /// Load the singleton `ShieldedPoolState` from CF_SHIELDED_POOL.
    #[cfg(feature = "zk")]
    pub fn get_shielded_pool_state(&self) -> Result<crate::zk::ShieldedPoolState, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;

        match self.db.get_cf(&cf, b"state") {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize ShieldedPoolState: {}", e)),
            Ok(None) => Ok(crate::zk::ShieldedPoolState::default()),
            Err(e) => Err(format!("Database error reading shielded pool state: {}", e)),
        }
    }

    /// Persist the singleton `ShieldedPoolState` to CF_SHIELDED_POOL.
    #[cfg(feature = "zk")]
    pub fn put_shielded_pool_state(
        &self,
        state: &crate::zk::ShieldedPoolState,
    ) -> Result<(), String> {
        let _state_commitment_guard = self.lock_state_commitment();
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;

        let data = serde_json::to_vec(state)
            .map_err(|e| format!("Failed to serialize ShieldedPoolState: {}", e))?;

        self.db
            .put_cf(&cf, b"state", &data)
            .map_err(|e| format!("Failed to store ShieldedPoolState: {}", e))?;
        self.clear_composite_state_root_cache();
        Ok(())
    }

    /// Collect all commitment leaves [0..count) from CF_SHIELDED_COMMITMENTS.
    pub fn get_all_shielded_commitments(&self, count: u64) -> Result<Vec<[u8; 32]>, String> {
        let mut leaves = Vec::with_capacity(count as usize);
        for index in 0..count {
            match self.get_shielded_commitment(index)? {
                Some(commitment) => leaves.push(commitment),
                None => {
                    return Err(format!(
                        "Missing shielded commitment at index {} (expected {})",
                        index, count
                    ))
                }
            }
        }
        Ok(leaves)
    }

    #[cfg(feature = "zk")]
    fn queue_clear_shielded_cf(
        &self,
        batch: &mut WriteBatch,
        cf_name: &'static str,
    ) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start);
        let mut keys = Vec::new();
        for item in iter {
            let (key, _) = item.map_err(|e| format!("{} iterator error: {}", cf_name, e))?;
            keys.push(key.to_vec());
        }
        for key in &keys {
            batch.delete_cf(&cf, key);
        }
        Ok(keys.len() as u64)
    }

    #[cfg(feature = "zk")]
    pub fn clear_shielded_state_categories(&self) -> Result<u64, String> {
        let _state_commitment_guard = self.lock_state_commitment();
        let mut batch = WriteBatch::default();
        let mut deleted_entries = 0u64;
        for cf_name in [
            CF_SHIELDED_COMMITMENTS,
            CF_SHIELDED_NOTE_PAYLOADS,
            CF_SHIELDED_NULLIFIERS,
            CF_SHIELDED_POOL,
            CF_SHIELDED_TXS,
        ] {
            deleted_entries =
                deleted_entries.saturating_add(self.queue_clear_shielded_cf(&mut batch, cf_name)?);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to clear shielded state categories: {}", e))?;
        self.clear_composite_state_root_cache();
        Ok(deleted_entries)
    }

    #[cfg(feature = "zk")]
    pub fn rebuild_shielded_state_from_canonical_blocks(
        &self,
        dry_run: bool,
    ) -> Result<ShieldedStateRebuildReport, String> {
        let _state_commitment_guard = self.lock_state_commitment();
        let end_slot = self.get_last_slot().unwrap_or(0);
        let start_slot = 0;

        let commitments_cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;
        let note_payloads_cf = self
            .db
            .cf_handle(CF_SHIELDED_NOTE_PAYLOADS)
            .ok_or_else(|| "Shielded note payloads CF not found".to_string())?;
        let nullifiers_cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;
        let pool_cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;
        let shielded_txs_cf = self
            .db
            .cf_handle(CF_SHIELDED_TXS)
            .ok_or_else(|| "Shielded txs CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        let mut deleted_entries = 0u64;
        for cf_name in [
            CF_SHIELDED_COMMITMENTS,
            CF_SHIELDED_NOTE_PAYLOADS,
            CF_SHIELDED_NULLIFIERS,
            CF_SHIELDED_POOL,
            CF_SHIELDED_TXS,
        ] {
            deleted_entries =
                deleted_entries.saturating_add(self.queue_clear_shielded_cf(&mut batch, cf_name)?);
        }

        let mut pool = crate::zk::ShieldedPoolState::new();
        let mut tree = crate::zk::MerkleTree::new();
        let mut spent_nullifiers = std::collections::HashSet::<[u8; 32]>::new();
        let mut scanned_transactions = 0u64;
        let mut shielded_transactions = 0u64;

        let scanned_blocks =
            self.for_each_canonical_block_in_range(start_slot, end_slot, |slot, block| {
                for (tx_index, tx) in block.transactions.iter().enumerate() {
                    scanned_transactions = scanned_transactions.saturating_add(1);
                    if self
                        .get_tx_meta_full(&tx.signature())?
                        .is_some_and(|meta| !meta.succeeded())
                    {
                        continue;
                    }
                    if is_shielded_transaction(tx) {
                        shielded_transactions = shielded_transactions.saturating_add(1);
                        let sig = tx.signature();
                        let mut shielded_key = Vec::with_capacity(48);
                        shielded_key.extend_from_slice(&slot.to_be_bytes());
                        shielded_key.extend_from_slice(&(tx_index as u64).to_be_bytes());
                        shielded_key.extend_from_slice(&sig.0);
                        batch.put_cf(&shielded_txs_cf, &shielded_key, []);
                    }

                    for ix in &tx.message.instructions {
                        if ix.program_id != crate::SYSTEM_PROGRAM_ID {
                            continue;
                        }
                        let Some(tag) = ix.data.first().copied() else {
                            continue;
                        };
                        match tag {
                            23 => {
                                if ix.data.len() < 41 {
                                    return Err(format!(
                                        "Shield instruction at slot {} tx {} is truncated",
                                        slot, tx_index
                                    ));
                                }
                                let amount = read_u64_le(&ix.data, 1..9, "Shield amount")?;
                                if amount == 0 {
                                    return Err(format!(
                                        "Shield instruction at slot {} tx {} has zero amount",
                                        slot, tx_index
                                    ));
                                }
                                let commitment = read_bytes32(&ix.data, 9, "Shield commitment")?;
                                let note_payload = shield_deposit_note_payload(&ix.data, &commitment)?;
                                let index = pool.commitment_count;
                                batch.put_cf(&commitments_cf, index.to_be_bytes(), commitment);
                                if let Some(payload) = note_payload {
                                    batch.put_cf(&note_payloads_cf, index.to_be_bytes(), payload);
                                }
                                tree.insert(commitment);
                                pool.commitment_count = pool.commitment_count.saturating_add(1);
                                pool.shield_count = pool.shield_count.saturating_add(1);
                                pool.total_shielded =
                                    pool.total_shielded.checked_add(amount).ok_or_else(|| {
                                        format!(
                                            "Shielded pool balance overflow at slot {} tx {}",
                                            slot, tx_index
                                        )
                                    })?;
                                pool.merkle_root = tree.root();
                            }
                            24 => {
                                if ix.data.len() < 105 {
                                    return Err(format!(
                                        "Unshield instruction at slot {} tx {} is truncated",
                                        slot, tx_index
                                    ));
                                }
                                let amount = read_u64_le(&ix.data, 1..9, "Unshield amount")?;
                                if amount == 0 {
                                    return Err(format!(
                                        "Unshield instruction at slot {} tx {} has zero amount",
                                        slot, tx_index
                                    ));
                                }
                                let nullifier = read_bytes32(&ix.data, 9, "Unshield nullifier")?;
                                if !spent_nullifiers.insert(nullifier) {
                                    return Err(format!(
                                        "Duplicate shielded nullifier {} at slot {} tx {}",
                                        hex::encode(nullifier),
                                        slot,
                                        tx_index
                                    ));
                                }
                                batch.put_cf(&nullifiers_cf, nullifier, [0x01]);
                                pool.unshield_count = pool.unshield_count.saturating_add(1);
                                pool.nullifier_count = pool.nullifier_count.saturating_add(1);
                                pool.total_shielded =
                                    pool.total_shielded.checked_sub(amount).ok_or_else(|| {
                                        format!(
                                            "Shielded pool underflow at slot {} tx {}",
                                            slot, tx_index
                                        )
                                    })?;
                            }
                            25 => {
                                if ix.data.len() < 161 {
                                    return Err(format!(
                                        "ShieldedTransfer instruction at slot {} tx {} is truncated",
                                        slot, tx_index
                                    ));
                                }
                                let nullifier_a =
                                    read_bytes32(&ix.data, 1, "ShieldedTransfer nullifier A")?;
                                let nullifier_b =
                                    read_bytes32(&ix.data, 33, "ShieldedTransfer nullifier B")?;
                                if nullifier_a == nullifier_b {
                                    return Err(format!(
                                        "Duplicate in-tx shielded nullifier at slot {} tx {}",
                                        slot, tx_index
                                    ));
                                }
                                for nullifier in [nullifier_a, nullifier_b] {
                                    if !spent_nullifiers.insert(nullifier) {
                                        return Err(format!(
                                            "Duplicate shielded nullifier {} at slot {} tx {}",
                                            hex::encode(nullifier),
                                            slot,
                                            tx_index
                                        ));
                                    }
                                    batch.put_cf(&nullifiers_cf, nullifier, [0x01]);
                                }

                                let commitment_c =
                                    read_bytes32(&ix.data, 65, "ShieldedTransfer commitment C")?;
                                let commitment_d =
                                    read_bytes32(&ix.data, 97, "ShieldedTransfer commitment D")?;
                                let output_payloads = shielded_transfer_note_payloads(
                                    &ix.data,
                                    &commitment_c,
                                    &commitment_d,
                                )?;

                                let idx0 = pool.commitment_count;
                                batch.put_cf(&commitments_cf, idx0.to_be_bytes(), commitment_c);
                                if let Some(payloads) = &output_payloads {
                                    batch.put_cf(
                                        &note_payloads_cf,
                                        idx0.to_be_bytes(),
                                        payloads[0].as_slice(),
                                    );
                                }
                                batch.put_cf(
                                    &commitments_cf,
                                    (idx0 + 1).to_be_bytes(),
                                    commitment_d,
                                );
                                if let Some(payloads) = &output_payloads {
                                    batch.put_cf(
                                        &note_payloads_cf,
                                        (idx0 + 1).to_be_bytes(),
                                        payloads[1].as_slice(),
                                    );
                                }

                                tree.insert(commitment_c);
                                tree.insert(commitment_d);
                                pool.commitment_count = pool.commitment_count.saturating_add(2);
                                pool.transfer_count = pool.transfer_count.saturating_add(1);
                                pool.nullifier_count = pool.nullifier_count.saturating_add(2);
                                pool.merkle_root = tree.root();
                            }
                            _ => {}
                        }
                    }
                }
                Ok(())
            })?;

        let pool_data = serde_json::to_vec(&pool)
            .map_err(|e| format!("Failed to serialize rebuilt ShieldedPoolState: {}", e))?;
        batch.put_cf(&pool_cf, b"state", &pool_data);
        if !dry_run {
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to write rebuilt shielded state: {}", e))?;
            self.clear_composite_state_root_cache();
        }

        Ok(ShieldedStateRebuildReport {
            start_slot,
            end_slot,
            scanned_blocks,
            scanned_transactions,
            shielded_transactions,
            shield_count: pool.shield_count,
            unshield_count: pool.unshield_count,
            transfer_count: pool.transfer_count,
            commitment_count: pool.commitment_count,
            nullifier_count: pool.nullifier_count,
            total_shielded: pool.total_shielded,
            merkle_root: pool.merkle_root,
            deleted_entries,
            dry_run,
        })
    }
}
