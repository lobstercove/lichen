use crate::block::Block;

use super::*;

#[derive(Debug, Clone, Eq, PartialEq)]
struct AccountTxIndexRow {
    key: Vec<u8>,
    hash: Hash,
    slot: u64,
    seq: u32,
}

fn extract_token_recipient_from_ix(ix: &crate::transaction::Instruction) -> Option<Pubkey> {
    let json_str = std::str::from_utf8(&ix.data).ok()?;
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let call = value.get("Call")?;
    let function = call.get("function")?.as_str()?;
    match function {
        "mint" | "transfer" | "transfer_from" => {
            let args = call.get("args")?.as_array()?;
            if args.len() < 64 {
                return None;
            }
            let mut to_bytes = [0u8; 32];
            for (index, item) in args[32..64].iter().enumerate() {
                to_bytes[index] = item.as_u64()? as u8;
            }
            Some(Pubkey::new(to_bytes))
        }
        _ => None,
    }
}

pub(crate) fn account_tx_index_entries_for_transaction(
    slot: u64,
    tx_index: usize,
    tx: &crate::transaction::Transaction,
) -> Vec<(Pubkey, Vec<u8>)> {
    if tx.is_consensus() {
        return Vec::new();
    }
    let contract_program_id = crate::processor::CONTRACT_PROGRAM_ID;
    let mut entries = Vec::new();

    let mut accounts = std::collections::BTreeSet::new();
    for ix in &tx.message.instructions {
        for account in &ix.accounts {
            accounts.insert(*account);
        }
        if ix.program_id == contract_program_id {
            if let Some(recipient) = extract_token_recipient_from_ix(ix) {
                accounts.insert(recipient);
            }
        }
    }

    let tx_hash = tx.signature();
    let seq = tx_index as u32;

    for account in accounts {
        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
        key.extend_from_slice(&account.0);
        key.extend_from_slice(&slot.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());
        key.extend_from_slice(&tx_hash.0);
        entries.push((account, key));
    }

    entries
}

pub(crate) fn account_tx_index_entries_for_block(block: &Block) -> Vec<(Pubkey, Vec<u8>)> {
    let mut entries = Vec::new();

    for (tx_index, tx) in block.transactions.iter().enumerate() {
        if tx.is_consensus() {
            continue;
        }
        entries.extend(account_tx_index_entries_for_transaction(
            block.header.slot,
            tx_index,
            tx,
        ));
    }

    entries
}

fn parse_account_tx_index_key(key: &[u8]) -> Result<Option<AccountTxIndexRow>, String> {
    if key.len() < 32 + 8 + 4 + 32 {
        return Ok(None);
    }

    let slot_bytes: [u8; 8] = key[32..40]
        .try_into()
        .map_err(|_| "Invalid slot bytes in account tx index".to_string())?;
    let seq_bytes: [u8; 4] = key[40..44]
        .try_into()
        .map_err(|_| "Invalid sequence bytes in account tx index".to_string())?;
    let mut hash_bytes = [0u8; 32];
    hash_bytes.copy_from_slice(&key[44..76]);

    Ok(Some(AccountTxIndexRow {
        key: key.to_vec(),
        hash: Hash(hash_bytes),
        slot: u64::from_be_bytes(slot_bytes),
        seq: u32::from_be_bytes(seq_bytes),
    }))
}

impl StateStore {
    /// Update token balance for a holder. Key: token_program(32) + holder(32).
    pub fn update_token_balance(
        &self,
        token_program: &Pubkey,
        holder: &Pubkey,
        balance: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&holder.0);

        let rev_cf = self
            .db
            .cf_handle(CF_HOLDER_TOKENS)
            .ok_or_else(|| "Holder tokens CF not found".to_string())?;
        let solana_cf = self
            .db
            .cf_handle(CF_SOLANA_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana token accounts CF not found".to_string())?;
        let solana_holder_cf = self
            .db
            .cf_handle(CF_SOLANA_HOLDER_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana holder token accounts CF not found".to_string())?;
        let mut rev_key = Vec::with_capacity(64);
        rev_key.extend_from_slice(&holder.0);
        rev_key.extend_from_slice(&token_program.0);

        let token_account = derive_solana_associated_token_address(holder, token_program)?;
        let binding = solana_token_account_binding_bytes(token_program, holder);
        let holder_key = solana_holder_token_account_key(holder, &token_account);

        let mut batch = WriteBatch::default();
        if balance == 0 {
            batch.delete_cf(&cf, &key);
            batch.delete_cf(&rev_cf, &rev_key);
            batch.put_cf(solana_cf, token_account.0, binding);
            batch.put_cf(solana_holder_cf, holder_key, token_program.0);
        } else {
            batch.put_cf(&cf, &key, balance.to_le_bytes());
            batch.put_cf(&rev_cf, &rev_key, balance.to_le_bytes());
            batch.put_cf(solana_cf, token_account.0, binding);
            batch.put_cf(solana_holder_cf, holder_key, token_program.0);
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to atomically update token balance indexes: {}", e))?;
        Ok(())
    }

    pub fn ensure_solana_token_account_binding(
        &self,
        token_program: &Pubkey,
        holder: &Pubkey,
    ) -> Result<Pubkey, String> {
        let token_account = derive_solana_associated_token_address(holder, token_program)?;
        let cf = self
            .db
            .cf_handle(CF_SOLANA_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana token accounts CF not found".to_string())?;
        let holder_cf = self
            .db
            .cf_handle(CF_SOLANA_HOLDER_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana holder token accounts CF not found".to_string())?;
        let holder_key = solana_holder_token_account_key(holder, &token_account);

        let mut batch = WriteBatch::default();
        batch.put_cf(
            cf,
            token_account.0,
            solana_token_account_binding_bytes(token_program, holder),
        );
        batch.put_cf(holder_cf, holder_key, token_program.0);

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to index Solana token account binding: {}", e))?;

        Ok(token_account)
    }

    pub fn get_solana_token_accounts_by_owner(
        &self,
        holder: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Pubkey, Pubkey)>, String> {
        let cf = self
            .db
            .cf_handle(CF_SOLANA_HOLDER_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana holder token accounts CF not found".to_string())?;

        let prefix = holder.0.to_vec();
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut accounts = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() != 64 || value.len() != 32 {
                continue;
            }

            let mut token_account = [0u8; 32];
            token_account.copy_from_slice(&key[32..64]);
            let mut token_program = [0u8; 32];
            token_program.copy_from_slice(&value[..32]);
            accounts.push((Pubkey(token_account), Pubkey(token_program)));
            if accounts.len() >= limit {
                break;
            }
        }

        Ok(accounts)
    }

    pub fn get_solana_token_account_binding(
        &self,
        token_account: &Pubkey,
    ) -> Result<Option<(Pubkey, Pubkey)>, String> {
        let cf = self
            .db
            .cf_handle(CF_SOLANA_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana token accounts CF not found".to_string())?;

        match self.db.get_cf(cf, token_account.0) {
            Ok(Some(data)) => Ok(parse_solana_token_account_binding(&data)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!(
                "Failed to read Solana token account binding: {}",
                e
            )),
        }
    }

    /// Get token balance for a specific holder.
    pub fn get_token_balance(
        &self,
        token_program: &Pubkey,
        holder: &Pubkey,
    ) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&holder.0);

        match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
            }
            _ => Ok(0),
        }
    }

    /// Get all token holders for a token program with their balances.
    pub fn get_token_holders(
        &self,
        token_program: &Pubkey,
        limit: usize,
        after_holder: Option<&Pubkey>,
    ) -> Result<Vec<(Pubkey, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;

        let prefix = token_program.0.to_vec();
        let start_key = if let Some(after_holder) = after_holder {
            let mut key = prefix.clone();
            key.extend_from_slice(&after_holder.0);
            key.push(0);
            key
        } else {
            prefix.clone()
        };

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&start_key, Direction::Forward),
        );

        let mut holders = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() == 64 && value.len() == 8 {
                let mut holder_bytes = [0u8; 32];
                holder_bytes.copy_from_slice(&key[32..64]);
                let holder = Pubkey(holder_bytes);
                let balance = u64::from_le_bytes((*value).try_into().unwrap());
                holders.push((holder, balance));
                if holders.len() >= limit {
                    break;
                }
            }
        }

        Ok(holders)
    }

    /// Record a token transfer. Key: token_program(32) + slot(BE 8) + seq(BE 8).
    pub fn put_token_transfer(
        &self,
        token_program: &Pubkey,
        transfer: &TokenTransfer,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_TRANSFERS)
            .ok_or_else(|| "Token transfers CF not found".to_string())?;

        let seq = self.next_transfer_seq(token_program, transfer.slot)?;

        let mut key = Vec::with_capacity(48);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&transfer.slot.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());

        let data = serde_json::to_vec(transfer)
            .map_err(|e| format!("Failed to serialize transfer: {}", e))?;

        self.db
            .put_cf(&cf, &key, data)
            .map_err(|e| format!("Failed to store token transfer: {}", e))
    }

    /// Get recent token transfers for a token program.
    pub fn get_token_transfers(
        &self,
        token_program: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<TokenTransfer>, String> {
        self.get_token_transfers_paginated_exact(
            token_program,
            limit,
            before_slot.map(|slot| (slot, 0)),
        )
        .map(|rows| rows.into_iter().map(|(transfer, _)| transfer).collect())
    }

    /// Paginate token transfers with an exclusive `(slot, sequence)` cursor.
    pub fn get_token_transfers_paginated_exact(
        &self,
        token_program: &Pubkey,
        limit: usize,
        before_cursor: Option<(u64, u64)>,
    ) -> Result<Vec<(TokenTransfer, u64)>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let cf = self
            .db
            .cf_handle(CF_TOKEN_TRANSFERS)
            .ok_or_else(|| "Token transfers CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&token_program.0);

        let mut end_key = prefix.clone();
        if let Some((before_slot, before_seq)) = before_cursor {
            end_key.extend_from_slice(&before_slot.to_be_bytes());
            end_key.extend_from_slice(&before_seq.to_be_bytes());
        } else {
            end_key.extend_from_slice(&[0xFF; 16]);
        }

        let mut rows = std::collections::BTreeMap::new();
        if let Some(cold) = self.cold_db.as_ref() {
            if let Some(cold_cf) = cold.cf_handle(COLD_CF_TOKEN_TRANSFERS) {
                let iter = cold.iterator_cf(
                    &cold_cf,
                    rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
                );
                for (key, value) in iter.flatten() {
                    if !key.starts_with(&prefix) {
                        break;
                    }
                    if key.len() < 48 {
                        continue;
                    }
                    let slot = u64::from_be_bytes(key[32..40].try_into().unwrap_or([0xFF; 8]));
                    let seq = u64::from_be_bytes(key[40..48].try_into().unwrap_or([0xFF; 8]));
                    if let Some((before_slot, before_seq)) = before_cursor {
                        if slot > before_slot || (slot == before_slot && seq >= before_seq) {
                            continue;
                        }
                    }
                    rows.insert(key.to_vec(), value.to_vec());
                }
            }
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() < 48 {
                continue;
            }
            let slot = u64::from_be_bytes(key[32..40].try_into().unwrap_or([0xFF; 8]));
            let seq = u64::from_be_bytes(key[40..48].try_into().unwrap_or([0xFF; 8]));
            if let Some((before_slot, before_seq)) = before_cursor {
                if slot > before_slot || (slot == before_slot && seq >= before_seq) {
                    continue;
                }
            }
            rows.insert(key.to_vec(), value.to_vec());
        }

        let mut transfers = Vec::new();
        for (key, value) in rows.iter().rev() {
            if let Ok(transfer) = serde_json::from_slice::<TokenTransfer>(value) {
                let seq = u64::from_be_bytes(key[40..48].try_into().unwrap_or([0; 8]));
                transfers.push((transfer, seq));
                if transfers.len() >= limit {
                    break;
                }
            }
        }

        Ok(transfers)
    }

    /// Atomic transfer sequence counter per token+slot.
    fn next_transfer_seq(&self, token_program: &Pubkey, slot: u64) -> Result<u64, String> {
        let _lock = self
            .transfer_seq_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut key = Vec::with_capacity(4 + 32 + 8);
        key.extend_from_slice(b"tsq:");
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&slot.to_be_bytes());

        let current = match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap())
            }
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, &key, next.to_le_bytes())
            .map_err(|e| format!("Failed to update transfer seq: {}", e))?;
        Ok(current)
    }

    /// Index a transaction by slot. Key: slot(BE 8) + seq(BE 8), Value: tx hash.
    pub fn index_tx_by_slot(&self, slot: u64, tx_hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let seq = self.next_tx_slot_seq(slot)?;

        let mut key = Vec::with_capacity(16);
        key.extend_from_slice(&slot.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());

        self.db
            .put_cf(&cf, &key, tx_hash.0)
            .map_err(|e| format!("Failed to index tx by slot: {}", e))
    }

    /// Get transactions for a slot.
    pub fn get_txs_by_slot(&self, slot: u64, limit: usize) -> Result<Vec<Hash>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let prefix = slot.to_be_bytes().to_vec();
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut hashes = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if value.len() == 32 {
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(&value);
                hashes.push(Hash(hash_bytes));
                if hashes.len() >= limit {
                    break;
                }
            }
        }

        Ok(hashes)
    }

    /// Look up the slot a transaction was included in, by its signature hash.
    pub fn get_tx_slot(&self, sig: &Hash) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "TX to slot CF not found".to_string())?;

        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) if data.len() == 8 => {
                let slot = u64::from_be_bytes(data.as_slice().try_into().unwrap());
                Ok(Some(slot))
            }
            Ok(_) => {
                if let Some(ref cold) = self.cold_db {
                    if let Some(cold_cf) = cold.cf_handle(COLD_CF_TX_TO_SLOT) {
                        return match cold.get_cf(&cold_cf, sig.0) {
                            Ok(Some(data)) if data.len() == 8 => {
                                let slot = u64::from_be_bytes(data.as_slice().try_into().unwrap());
                                Ok(Some(slot))
                            }
                            Ok(_) => Ok(None),
                            Err(e) => Err(format!("Cold database error looking up tx slot: {}", e)),
                        };
                    }
                }
                Ok(None)
            }
            Err(e) => Err(format!("Database error looking up tx slot: {}", e)),
        }
    }

    /// Index a transaction signature -> slot for O(1) reverse lookup.
    pub fn index_tx_to_slot(&self, sig: &Hash, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "TX to slot CF not found".to_string())?;

        self.db
            .put_cf(&cf, sig.0, slot.to_be_bytes())
            .map_err(|e| format!("Failed to index tx to slot: {}", e))
    }

    /// Protected by tx_slot_seq_lock to prevent duplicate sequence numbers.
    fn next_tx_slot_seq(&self, slot: u64) -> Result<u64, String> {
        let _guard = self
            .tx_slot_seq_lock
            .lock()
            .map_err(|e| format!("TX slot seq lock poisoned: {}", e))?;

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut key = Vec::with_capacity(12);
        key.extend_from_slice(b"txs:");
        key.extend_from_slice(&slot.to_be_bytes());

        let current = match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap())
            }
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, &key, next.to_le_bytes())
            .map_err(|e| format!("Failed to update tx slot seq: {}", e))?;
        Ok(current)
    }

    /// AUDIT-FIX M7: Write account-transaction indexes into the provided WriteBatch
    /// so they are committed atomically with the block data.
    pub(crate) fn batch_index_account_transactions(
        &self,
        block: &Block,
        batch: &mut WriteBatch,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let cf_stats = self.db.cf_handle(CF_STATS);
        let mut pending_account_keys: std::collections::HashMap<
            Pubkey,
            std::collections::BTreeSet<Vec<u8>>,
        > = std::collections::HashMap::new();

        for (account, key) in account_tx_index_entries_for_block(block) {
            batch.put_cf(&cf, &key, []);

            if cf_stats.is_some() {
                pending_account_keys.entry(account).or_default().insert(key);
            }
        }

        if let Some(ref cf_s) = cf_stats {
            for (account, keys) in &pending_account_keys {
                let mut counter_key = Vec::with_capacity(5 + 32);
                counter_key.extend_from_slice(b"atxc:");
                counter_key.extend_from_slice(&account.0);
                let base_count = match self.db.get_cf(cf_s, &counter_key) {
                    Ok(Some(data)) if data.len() == 8 => {
                        u64::from_le_bytes(data.as_slice().try_into().unwrap())
                    }
                    Ok(_) => {
                        Self::count_account_tx_entries_in_db(&self.db, CF_ACCOUNT_TXS, account)?
                    }
                    Err(e) => {
                        return Err(format!(
                            "Failed reading account tx counter for {}: {}",
                            account.to_base58(),
                            e
                        ));
                    }
                };
                batch.put_cf(
                    cf_s,
                    &counter_key,
                    base_count.saturating_add(keys.len() as u64).to_le_bytes(),
                );
            }
        }

        Ok(())
    }

    pub fn get_account_tx_signatures(
        &self,
        pubkey: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Hash, u64)>, String> {
        self.get_account_tx_signatures_paginated(pubkey, limit, None)
    }

    fn count_account_tx_entries_in_db(
        db: &DB,
        cf_name: &str,
        pubkey: &Pubkey,
    ) -> Result<u64, String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;

        let prefix = pubkey.0.to_vec();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(
            &cf,
            read_opts,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut count = 0u64;
        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() >= 32 + 8 + 4 + 32 {
                count = count.saturating_add(1);
            }
        }

        Ok(count)
    }

    fn collect_account_tx_keys_in_db(
        db: &DB,
        cf_name: &str,
        pubkey: &Pubkey,
        keys: &mut std::collections::BTreeSet<Vec<u8>>,
    ) -> Result<(), String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;

        let prefix = pubkey.0.to_vec();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(
            &cf,
            read_opts,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() >= 32 + 8 + 4 + 32 {
                keys.insert(key.to_vec());
            }
        }

        Ok(())
    }

    fn scan_account_tx_signatures_in_db(
        db: &DB,
        cf_name: &str,
        pubkey: &Pubkey,
        limit: usize,
        before_cursor: Option<(u64, u32)>,
    ) -> Result<Vec<AccountTxIndexRow>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;

        let prefix = pubkey.0.to_vec();
        let mut seek_key = Vec::with_capacity(76);
        seek_key.extend_from_slice(&pubkey.0);
        if let Some((slot, seq)) = before_cursor {
            if slot == 0 && seq == 0 {
                return Ok(Vec::new());
            }
            seek_key.extend_from_slice(&slot.to_be_bytes());
            seek_key.extend_from_slice(&seq.to_be_bytes());
            seek_key.extend_from_slice(&[0; 32]);
        } else {
            seek_key.extend_from_slice(&u64::MAX.to_be_bytes());
            seek_key.extend_from_slice(&[0xFF; 36]);
        }

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(
            &cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::with_capacity(limit);
        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            let Some(row) = parse_account_tx_index_key(&key)? else {
                continue;
            };
            if let Some((before_slot, before_seq)) = before_cursor {
                if row.slot > before_slot || (row.slot == before_slot && row.seq >= before_seq) {
                    continue;
                }
            }
            results.push(row);

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Get account transaction count. When cold storage is attached, count both
    /// hot and cold account indexes because old rows may have been migrated out
    /// of hot RocksDB.
    pub fn count_account_txs(&self, pubkey: &Pubkey) -> Result<u64, String> {
        if let Some(ref cold) = self.cold_db {
            let mut keys = std::collections::BTreeSet::new();
            Self::collect_account_tx_keys_in_db(&self.db, CF_ACCOUNT_TXS, pubkey, &mut keys)?;
            Self::collect_account_tx_keys_in_db(cold, COLD_CF_ACCOUNT_TXS, pubkey, &mut keys)?;
            return Ok(keys.len() as u64);
        }

        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut counter_key = Vec::with_capacity(5 + 32);
        counter_key.extend_from_slice(b"atxc:");
        counter_key.extend_from_slice(&pubkey.0);

        match self.db.get_cf(&cf_stats, &counter_key) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
            }
            _ => {
                let count = Self::count_account_tx_entries_in_db(&self.db, CF_ACCOUNT_TXS, pubkey)?;

                if let Err(e) = self.db.put_cf(&cf_stats, &counter_key, count.to_le_bytes()) {
                    tracing::warn!("Failed to cache account TX count: {e}");
                }
                Ok(count)
            }
        }
    }

    /// Paginated account transactions using reverse iteration with cursor.
    /// Returns newest-first. Pass `before_slot` to get the next page.
    pub fn get_account_tx_signatures_paginated(
        &self,
        pubkey: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<(Hash, u64)>, String> {
        self.get_account_tx_signatures_paginated_exact(
            pubkey,
            limit,
            before_slot.map(|slot| (slot, 0)),
        )
        .map(|rows| {
            rows.into_iter()
                .map(|(hash, slot, _)| (hash, slot))
                .collect()
        })
    }

    /// Paginate account transactions by their exact canonical index position.
    /// The cursor is exclusive and ordered by `(slot, transaction_index)`.
    pub fn get_account_tx_signatures_paginated_exact(
        &self,
        pubkey: &Pubkey,
        limit: usize,
        before_cursor: Option<(u64, u32)>,
    ) -> Result<Vec<(Hash, u64, u32)>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut rows = Self::scan_account_tx_signatures_in_db(
            &self.db,
            CF_ACCOUNT_TXS,
            pubkey,
            limit,
            before_cursor,
        )?;
        if let Some(ref cold) = self.cold_db {
            rows.extend(Self::scan_account_tx_signatures_in_db(
                cold,
                COLD_CF_ACCOUNT_TXS,
                pubkey,
                limit,
                before_cursor,
            )?);
        }

        rows.sort_by(|a, b| {
            b.slot
                .cmp(&a.slot)
                .then_with(|| b.seq.cmp(&a.seq))
                .then_with(|| b.hash.0.cmp(&a.hash.0))
        });
        rows.dedup_by(|a, b| a.key == b.key);
        rows.truncate(limit);

        Ok(rows
            .into_iter()
            .map(|row| (row.hash, row.slot, row.seq))
            .collect())
    }

    /// Get recent transactions across all addresses using CF_TX_BY_SLOT reverse scan.
    /// Returns (tx_hash, slot) pairs newest-first. Pass `before_slot` for next page.
    pub fn get_recent_txs(
        &self,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<(Hash, u64)>, String> {
        self.get_recent_txs_paginated_exact(limit, before_slot.map(|slot| (slot, 0)))
            .map(|rows| {
                rows.into_iter()
                    .map(|(hash, slot, _)| (hash, slot))
                    .collect()
            })
    }

    /// Paginate the global transaction index using an exclusive canonical
    /// `(slot, transaction_index)` cursor.
    pub fn get_recent_txs_paginated_exact(
        &self,
        limit: usize,
        before_cursor: Option<(u64, u64)>,
    ) -> Result<Vec<(Hash, u64, u64)>, String> {
        self.get_recent_txs_paginated_exact_filtered(limit, before_cursor, false)
    }

    /// Paginate user transactions while retaining consensus envelopes in the
    /// canonical archive indexes used for history verification.
    pub fn get_recent_user_txs_paginated_exact(
        &self,
        limit: usize,
        before_cursor: Option<(u64, u64)>,
    ) -> Result<Vec<(Hash, u64, u64)>, String> {
        self.get_recent_txs_paginated_exact_filtered(limit, before_cursor, true)
    }

    fn get_recent_txs_paginated_exact_filtered(
        &self,
        limit: usize,
        before_cursor: Option<(u64, u64)>,
        user_only: bool,
    ) -> Result<Vec<(Hash, u64, u64)>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let seek_key = if let Some((slot, seq)) = before_cursor {
            let mut key = Vec::with_capacity(16);
            key.extend_from_slice(&slot.to_be_bytes());
            key.extend_from_slice(&seq.to_be_bytes());
            key
        } else {
            vec![0xFF; 16]
        };

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);

        let iter = self.db.iterator_cf_opt(
            &cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::new();
        for item in iter.flatten() {
            let (key, value) = item;
            if key.len() < 16 || value.len() != 32 {
                continue;
            }

            let slot = u64::from_be_bytes(
                key[0..8]
                    .try_into()
                    .map_err(|_| "Corrupt slot key in block hashes".to_string())?,
            );
            let seq = u64::from_be_bytes(
                key[8..16]
                    .try_into()
                    .map_err(|_| "Corrupt sequence key in transaction index".to_string())?,
            );

            if let Some((before_slot, before_seq)) = before_cursor {
                if slot > before_slot || (slot == before_slot && seq >= before_seq) {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&value);
            let hash = Hash(hash_bytes);
            if user_only
                && self
                    .get_transaction(&hash)?
                    .is_some_and(|transaction| transaction.is_consensus())
            {
                continue;
            }
            results.push((hash, slot, seq));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    fn read_recent_shielded_tx_index(
        &self,
        limit: usize,
        before_cursor: Option<(u64, u64)>,
    ) -> Result<Vec<(Hash, u64, u64)>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let cf = self
            .db
            .cf_handle(CF_SHIELDED_TXS)
            .ok_or_else(|| "Shielded txs CF not found".to_string())?;

        let seek_key = if let Some((slot, seq)) = before_cursor {
            let mut key = Vec::with_capacity(48);
            key.extend_from_slice(&slot.to_be_bytes());
            key.extend_from_slice(&seq.to_be_bytes());
            key.extend_from_slice(&[0; 32]);
            key
        } else {
            vec![0xFF; 48]
        };

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);

        let iter = self.db.iterator_cf_opt(
            &cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::with_capacity(limit.min(128));
        for item in iter.flatten() {
            let key = item.0;
            if key.len() < 48 {
                continue;
            }

            let slot = u64::from_be_bytes(
                key[0..8]
                    .try_into()
                    .map_err(|_| "Corrupt slot key in shielded tx index".to_string())?,
            );
            let seq = u64::from_be_bytes(
                key[8..16]
                    .try_into()
                    .map_err(|_| "Corrupt sequence key in shielded tx index".to_string())?,
            );

            if let Some((before_slot, before_seq)) = before_cursor {
                if slot > before_slot || (slot == before_slot && seq >= before_seq) {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&key[16..48]);
            results.push((Hash(hash_bytes), slot, seq));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    fn backfill_recent_shielded_tx_index(
        &self,
        limit: usize,
        before_cursor: Option<(u64, u64)>,
    ) -> Result<Vec<(Hash, u64, u64)>, String> {
        const MAX_BACKFILL_SCAN: usize = 100_000;
        if limit == 0 {
            return Ok(Vec::new());
        }

        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;
        let shielded_txs_cf = self
            .db
            .cf_handle(CF_SHIELDED_TXS)
            .ok_or_else(|| "Shielded txs CF not found".to_string())?;

        let seek_key = if let Some((slot, seq)) = before_cursor {
            let mut key = Vec::with_capacity(16);
            key.extend_from_slice(&slot.to_be_bytes());
            key.extend_from_slice(&seq.to_be_bytes());
            key
        } else {
            vec![0xFF; 16]
        };

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);

        let iter = self.db.iterator_cf_opt(
            &tx_by_slot_cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::with_capacity(limit.min(128));
        let mut batch = WriteBatch::default();
        let mut indexed = 0usize;
        let mut scanned = 0usize;

        for item in iter.flatten() {
            let (key, value) = item;
            if key.len() < 16 || value.len() != 32 {
                continue;
            }

            let slot = u64::from_be_bytes(
                key[0..8]
                    .try_into()
                    .map_err(|_| "Corrupt slot key in tx-by-slot index".to_string())?,
            );
            let seq = u64::from_be_bytes(
                key[8..16]
                    .try_into()
                    .map_err(|_| "Corrupt sequence key in tx-by-slot index".to_string())?,
            );
            if let Some((before_slot, before_seq)) = before_cursor {
                if slot > before_slot || (slot == before_slot && seq >= before_seq) {
                    continue;
                }
            }

            scanned += 1;
            if scanned > MAX_BACKFILL_SCAN {
                break;
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&value);
            let hash = Hash(hash_bytes);
            let tx = match self.get_transaction(&hash)? {
                Some(tx) => tx,
                None => continue,
            };
            if !is_shielded_transaction(&tx) {
                continue;
            }

            let mut shielded_key = Vec::with_capacity(48);
            shielded_key.extend_from_slice(&key[0..16]);
            shielded_key.extend_from_slice(&hash.0);
            batch.put_cf(&shielded_txs_cf, &shielded_key, []);
            indexed += 1;
            results.push((hash, slot, seq));

            if results.len() >= limit {
                break;
            }
        }

        if indexed > 0 {
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to backfill shielded tx index: {}", e))?;
        }

        Ok(results)
    }

    /// Get recent shielded transactions using the shielded transaction index.
    /// Existing databases are lazily backfilled from the generic transaction
    /// index only when the shielded index does not yet cover the requested page.
    pub fn get_recent_shielded_txs(
        &self,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<(Hash, u64)>, String> {
        self.get_recent_shielded_txs_paginated_exact(limit, before_slot.map(|slot| (slot, 0)))
            .map(|rows| {
                rows.into_iter()
                    .map(|(hash, slot, _)| (hash, slot))
                    .collect()
            })
    }

    /// Paginate shielded transactions using the same exclusive canonical
    /// `(slot, transaction_index)` cursor as the global transaction index.
    pub fn get_recent_shielded_txs_paginated_exact(
        &self,
        limit: usize,
        before_cursor: Option<(u64, u64)>,
    ) -> Result<Vec<(Hash, u64, u64)>, String> {
        let mut results = self.read_recent_shielded_tx_index(limit, before_cursor)?;
        if results.len() >= limit {
            return Ok(results);
        }

        let backfill_cursor = results
            .last()
            .map(|(_, slot, seq)| (*slot, *seq))
            .or(before_cursor);
        let remaining = limit.saturating_sub(results.len());
        let mut backfilled = self.backfill_recent_shielded_tx_index(remaining, backfill_cursor)?;
        for item in backfilled.drain(..) {
            if !results
                .iter()
                .any(|(hash, slot, seq)| *hash == item.0 && *slot == item.1 && *seq == item.2)
            {
                results.push(item);
            }
        }
        Ok(results)
    }

    /// Get all token programs a holder has balances in (reverse index scan).
    pub fn get_holder_token_balances(
        &self,
        holder: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Pubkey, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_HOLDER_TOKENS)
            .ok_or_else(|| "Holder tokens CF not found".to_string())?;

        let prefix = holder.0.to_vec();
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut tokens = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() == 64 && value.len() == 8 {
                let mut prog_bytes = [0u8; 32];
                prog_bytes.copy_from_slice(&key[32..64]);
                let program = Pubkey(prog_bytes);
                let balance = u64::from_le_bytes(match (*value).try_into() {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                });
                tokens.push((program, balance));
                if tokens.len() >= limit {
                    break;
                }
            }
        }
        Ok(tokens)
    }
}
