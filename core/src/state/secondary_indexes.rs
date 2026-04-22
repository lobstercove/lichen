use crate::block::Block;

use super::*;

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
        let cf = self
            .db
            .cf_handle(CF_TOKEN_TRANSFERS)
            .ok_or_else(|| "Token transfers CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&token_program.0);

        let mut end_key = prefix.clone();
        if let Some(before_slot) = before_slot {
            end_key.extend_from_slice(&before_slot.to_be_bytes());
        } else {
            end_key.extend_from_slice(&[0xFF; 16]);
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut transfers = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if let Some(before_slot) = before_slot {
                if key.len() >= 40 {
                    let slot_bytes: [u8; 8] = key[32..40].try_into().unwrap_or([0xFF; 8]);
                    let slot = u64::from_be_bytes(slot_bytes);
                    if slot >= before_slot {
                        continue;
                    }
                }
            }
            if let Ok(transfer) = serde_json::from_slice::<TokenTransfer>(&value) {
                transfers.push(transfer);
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
            Ok(_) => Ok(None),
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
        let contract_program_id = crate::processor::CONTRACT_PROGRAM_ID;

        let mut counter_deltas: std::collections::HashMap<Pubkey, u64> =
            std::collections::HashMap::new();

        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let mut accounts = std::collections::HashSet::new();
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
                key.extend_from_slice(&block.header.slot.to_be_bytes());
                key.extend_from_slice(&seq.to_be_bytes());
                key.extend_from_slice(&tx_hash.0);

                batch.put_cf(&cf, &key, []);

                if let Some(ref cf_s) = cf_stats {
                    let delta = counter_deltas.entry(account).or_insert_with(|| {
                        let mut counter_key = Vec::with_capacity(5 + 32);
                        counter_key.extend_from_slice(b"atxc:");
                        counter_key.extend_from_slice(&account.0);
                        match self.db.get_cf(cf_s, &counter_key) {
                            Ok(Some(data)) if data.len() == 8 => {
                                u64::from_le_bytes(data.as_slice().try_into().unwrap())
                            }
                            _ => 0,
                        }
                    });
                    *delta += 1;
                }
            }
        }

        if let Some(ref cf_s) = cf_stats {
            for (account, count) in &counter_deltas {
                let mut counter_key = Vec::with_capacity(5 + 32);
                counter_key.extend_from_slice(b"atxc:");
                counter_key.extend_from_slice(&account.0);
                batch.put_cf(cf_s, &counter_key, count.to_le_bytes());
            }
        }

        Ok(())
    }

    pub fn get_account_tx_signatures(
        &self,
        pubkey: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Hash, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&pubkey.0);

        let mut end_key = prefix.clone();
        end_key.extend_from_slice(&[0xFF; 44]);

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut items = Vec::with_capacity(limit);
        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() < 32 + 8 + 4 + 32 {
                continue;
            }

            let slot_bytes: [u8; 8] = key[32..40]
                .try_into()
                .map_err(|_| "Invalid slot bytes in account tx index".to_string())?;
            let slot = u64::from_be_bytes(slot_bytes);

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&key[44..76]);
            items.push((Hash(hash_bytes), slot));

            if items.len() >= limit {
                break;
            }
        }

        Ok(items)
    }

    /// Get account transaction count via O(1) atomic counter.
    /// Falls back to prefix scan if counter not yet populated.
    pub fn count_account_txs(&self, pubkey: &Pubkey) -> Result<u64, String> {
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
                let cf = self
                    .db
                    .cf_handle(CF_ACCOUNT_TXS)
                    .ok_or_else(|| "Account txs CF not found".to_string())?;

                let mut prefix = Vec::with_capacity(32);
                prefix.extend_from_slice(&pubkey.0);

                let mut count = 0u64;
                let iter = self.db.iterator_cf(
                    &cf,
                    rocksdb::IteratorMode::From(&prefix, Direction::Forward),
                );
                for item in iter {
                    let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
                    if !key.starts_with(&prefix) {
                        break;
                    }
                    count += 1;
                }

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
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let prefix = pubkey.0.to_vec();
        let mut seek_key = Vec::with_capacity(76);
        seek_key.extend_from_slice(&pubkey.0);
        if let Some(slot) = before_slot {
            seek_key.extend_from_slice(&slot.to_be_bytes());
        } else {
            seek_key.extend_from_slice(&u64::MAX.to_be_bytes());
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::new();
        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() < 32 + 8 + 4 + 32 {
                continue;
            }

            let slot_bytes: [u8; 8] = key[32..40]
                .try_into()
                .map_err(|_| "Invalid slot bytes".to_string())?;
            let slot = u64::from_be_bytes(slot_bytes);
            if let Some(bs) = before_slot {
                if slot >= bs {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&key[44..76]);
            results.push((Hash(hash_bytes), slot));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Get recent transactions across all addresses using CF_TX_BY_SLOT reverse scan.
    /// Returns (tx_hash, slot) pairs newest-first. Pass `before_slot` for next page.
    pub fn get_recent_txs(
        &self,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<(Hash, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let seek_key = if let Some(slot) = before_slot {
            slot.to_be_bytes().to_vec()
        } else {
            u64::MAX.to_be_bytes().to_vec()
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

            if let Some(bs) = before_slot {
                if slot >= bs {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&value);
            results.push((Hash(hash_bytes), slot));

            if results.len() >= limit {
                break;
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
