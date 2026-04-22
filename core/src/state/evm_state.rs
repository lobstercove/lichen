use super::*;

#[derive(Serialize, Deserialize)]
struct PersistedEvmReceipt {
    evm_hash: [u8; 32],
    status: bool,
    gas_used: u64,
    block_slot: Option<u64>,
    block_hash: Option<[u8; 32]>,
    contract_address: Option<[u8; 20]>,
    logs: Vec<Vec<u8>>,
    structured_logs: Vec<crate::evm::EvmLog>,
}

#[derive(Deserialize)]
struct LegacyPersistedEvmReceipt {
    evm_hash: [u8; 32],
    status: bool,
    gas_used: u64,
    block_slot: Option<u64>,
    block_hash: Option<[u8; 32]>,
    contract_address: Option<[u8; 20]>,
    logs: Vec<Vec<u8>>,
}

impl From<&EvmReceipt> for PersistedEvmReceipt {
    fn from(receipt: &EvmReceipt) -> Self {
        Self {
            evm_hash: receipt.evm_hash,
            status: receipt.status,
            gas_used: receipt.gas_used,
            block_slot: receipt.block_slot,
            block_hash: receipt.block_hash,
            contract_address: receipt.contract_address,
            logs: receipt.logs.clone(),
            structured_logs: receipt.structured_logs.clone(),
        }
    }
}

impl From<PersistedEvmReceipt> for EvmReceipt {
    fn from(receipt: PersistedEvmReceipt) -> Self {
        Self {
            evm_hash: receipt.evm_hash,
            status: receipt.status,
            gas_used: receipt.gas_used,
            block_slot: receipt.block_slot,
            block_hash: receipt.block_hash,
            contract_address: receipt.contract_address,
            logs: receipt.logs,
            structured_logs: receipt.structured_logs,
        }
    }
}

impl From<LegacyPersistedEvmReceipt> for EvmReceipt {
    fn from(receipt: LegacyPersistedEvmReceipt) -> Self {
        Self {
            evm_hash: receipt.evm_hash,
            status: receipt.status,
            gas_used: receipt.gas_used,
            block_slot: receipt.block_slot,
            block_hash: receipt.block_hash,
            contract_address: receipt.contract_address,
            logs: receipt.logs,
            structured_logs: Vec::new(),
        }
    }
}

pub(super) fn serialize_evm_receipt_for_storage(receipt: &EvmReceipt) -> Result<Vec<u8>, String> {
    bincode::serialize(&PersistedEvmReceipt::from(receipt))
        .map_err(|e| format!("Failed to serialize EVM receipt: {}", e))
}

pub(super) fn deserialize_evm_receipt_from_storage(data: &[u8]) -> Result<EvmReceipt, String> {
    match bincode::deserialize::<PersistedEvmReceipt>(data) {
        Ok(receipt) => Ok(receipt.into()),
        Err(primary_err) => bincode::deserialize::<LegacyPersistedEvmReceipt>(data)
            .map(Into::into)
            .map_err(|_| format!("Failed to deserialize EVM receipt: {}", primary_err)),
    }
}

impl StateStore {
    /// Register EVM address mapping (EVM address -> Native pubkey).
    /// Called on first transaction from an EVM address.
    pub fn register_evm_address(
        &self,
        evm_address: &[u8; 20],
        native_pubkey: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM Map CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, evm_address, native_pubkey.0);

        let mut reverse_key = Vec::with_capacity(52);
        reverse_key.extend_from_slice(b"reverse:");
        reverse_key.extend_from_slice(&native_pubkey.0);
        batch.put_cf(&cf, &reverse_key, evm_address);

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to register EVM address: {}", e))
    }

    pub fn lookup_evm_address(&self, evm_address: &[u8; 20]) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM Map CF not found".to_string())?;

        match self.db.get_cf(&cf, evm_address) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid pubkey data in EVM map".to_string());
                }
                let mut pubkey_bytes = [0u8; 32];
                pubkey_bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(pubkey_bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Reverse lookup: native pubkey -> EVM address.
    pub fn lookup_native_to_evm(&self, native_pubkey: &Pubkey) -> Result<Option<[u8; 20]>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM Map CF not found".to_string())?;

        let mut reverse_key = Vec::with_capacity(40);
        reverse_key.extend_from_slice(b"reverse:");
        reverse_key.extend_from_slice(&native_pubkey.0);

        match self.db.get_cf(&cf, &reverse_key) {
            Ok(Some(data)) => {
                if data.len() != 20 {
                    return Err("Invalid EVM address data in reverse map".to_string());
                }
                let mut evm_bytes = [0u8; 20];
                evm_bytes.copy_from_slice(&data);
                Ok(Some(evm_bytes))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn parse_evm_address(addr_str: &str) -> Result<[u8; 20], String> {
        let addr_str = addr_str.strip_prefix("0x").unwrap_or(addr_str);
        if addr_str.len() != 40 {
            return Err("Invalid EVM address length".to_string());
        }

        let mut bytes = [0u8; 20];
        for i in 0..20 {
            let byte_str = &addr_str[i * 2..i * 2 + 2];
            bytes[i] = u8::from_str_radix(byte_str, 16)
                .map_err(|_| "Invalid hex in EVM address".to_string())?;
        }
        Ok(bytes)
    }

    pub fn get_evm_account(&self, evm_address: &[u8; 20]) -> Result<Option<EvmAccount>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_ACCOUNTS)
            .ok_or_else(|| "EVM Accounts CF not found".to_string())?;

        match self.db.get_cf(&cf, evm_address) {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map(Some)
                .map_err(|e| format!("Failed to deserialize EVM account: {}", e)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn put_evm_account(
        &self,
        evm_address: &[u8; 20],
        account: &EvmAccount,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_ACCOUNTS)
            .ok_or_else(|| "EVM Accounts CF not found".to_string())?;

        let data = bincode::serialize(account)
            .map_err(|e| format!("Failed to serialize EVM account: {}", e))?;

        self.db
            .put_cf(&cf, evm_address, data)
            .map_err(|e| format!("Failed to store EVM account: {}", e))
    }

    pub fn clear_evm_account(&self, evm_address: &[u8; 20]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_ACCOUNTS)
            .ok_or_else(|| "EVM Accounts CF not found".to_string())?;
        self.db
            .delete_cf(&cf, evm_address)
            .map_err(|e| format!("Failed to delete EVM account: {}", e))
    }

    pub fn get_evm_storage(&self, evm_address: &[u8; 20], slot: &[u8; 32]) -> Result<U256, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let mut key = Vec::with_capacity(20 + 32);
        key.extend_from_slice(evm_address);
        key.extend_from_slice(slot);

        match self.db.get_cf(&cf, key) {
            Ok(Some(data)) => {
                let bytes: [u8; 32] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid EVM storage value length".to_string())?;
                Ok(U256::from_be_bytes(bytes))
            }
            Ok(None) => Ok(U256::ZERO),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn put_evm_storage(
        &self,
        evm_address: &[u8; 20],
        slot: &[u8; 32],
        value: U256,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let mut key = Vec::with_capacity(20 + 32);
        key.extend_from_slice(evm_address);
        key.extend_from_slice(slot);

        self.db
            .put_cf(&cf, key, value.to_be_bytes::<32>())
            .map_err(|e| format!("Failed to store EVM storage: {}", e))
    }

    /// Use WriteBatch for atomic bulk delete instead of one-by-one.
    pub fn clear_evm_storage(&self, evm_address: &[u8; 20]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let prefix = evm_address;
        let keys: Vec<Box<[u8]>> = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::From(prefix, Direction::Forward))
            .filter_map(|item| item.ok())
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, _)| key)
            .collect();

        if keys.is_empty() {
            return Ok(());
        }

        let mut batch = rocksdb::WriteBatch::default();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to batch-delete EVM storage: {}", e))
    }

    pub fn clear_evm_storage_slot(
        &self,
        evm_address: &[u8; 20],
        slot: &[u8; 32],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let mut key = Vec::with_capacity(20 + 32);
        key.extend_from_slice(evm_address);
        key.extend_from_slice(slot);

        self.db
            .delete_cf(&cf, key)
            .map_err(|e| format!("Failed to delete EVM storage: {}", e))
    }

    pub fn put_evm_tx(&self, record: &EvmTxRecord) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_TXS)
            .ok_or_else(|| "EVM Txs CF not found".to_string())?;
        let data =
            bincode::serialize(record).map_err(|e| format!("Failed to serialize EVM tx: {}", e))?;
        self.db
            .put_cf(&cf, record.evm_hash, data)
            .map_err(|e| format!("Failed to store EVM tx: {}", e))
    }

    pub fn get_evm_tx(&self, evm_hash: &[u8; 32]) -> Result<Option<EvmTxRecord>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_TXS)
            .ok_or_else(|| "EVM Txs CF not found".to_string())?;
        match self.db.get_cf(&cf, evm_hash) {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map(Some)
                .map_err(|e| format!("Failed to deserialize EVM tx: {}", e)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn mark_evm_tx_included(
        &self,
        evm_hash: &[u8; 32],
        slot: u64,
        block_hash: &Hash,
    ) -> Result<(), String> {
        let mut record = match self.get_evm_tx(evm_hash)? {
            Some(record) => record,
            None => return Ok(()),
        };
        record.block_slot = Some(slot);
        record.block_hash = Some(block_hash.0);
        self.put_evm_tx(&record)
    }

    pub fn put_evm_receipt(&self, receipt: &EvmReceipt) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_RECEIPTS)
            .ok_or_else(|| "EVM Receipts CF not found".to_string())?;
        let data = serialize_evm_receipt_for_storage(receipt)?;
        self.db
            .put_cf(&cf, receipt.evm_hash, data)
            .map_err(|e| format!("Failed to store EVM receipt: {}", e))
    }

    pub fn get_evm_receipt(&self, evm_hash: &[u8; 32]) -> Result<Option<EvmReceipt>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_RECEIPTS)
            .ok_or_else(|| "EVM Receipts CF not found".to_string())?;
        match self.db.get_cf(&cf, evm_hash) {
            Ok(Some(data)) => deserialize_evm_receipt_from_storage(&data).map(Some),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store EVM logs for a slot, appending to existing logs if any.
    pub fn put_evm_logs_for_slot(
        &self,
        slot: u64,
        logs: &[crate::evm::EvmLogEntry],
    ) -> Result<(), String> {
        if logs.is_empty() {
            return Ok(());
        }
        let cf = self
            .db
            .cf_handle(CF_EVM_LOGS_BY_SLOT)
            .ok_or_else(|| "EVM Logs CF not found".to_string())?;
        let key = slot.to_be_bytes();
        let mut existing: Vec<crate::evm::EvmLogEntry> = match self.db.get_cf(&cf, key) {
            Ok(Some(data)) => bincode::deserialize(&data).unwrap_or_default(),
            _ => Vec::new(),
        };
        existing.extend_from_slice(logs);
        let data = bincode::serialize(&existing)
            .map_err(|e| format!("Failed to serialize EVM logs: {}", e))?;
        self.db
            .put_cf(&cf, key, data)
            .map_err(|e| format!("Failed to store EVM logs: {}", e))
    }

    pub fn get_evm_logs_for_slot(&self, slot: u64) -> Result<Vec<crate::evm::EvmLogEntry>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_LOGS_BY_SLOT)
            .ok_or_else(|| "EVM Logs CF not found".to_string())?;
        let key = slot.to_be_bytes();
        match self.db.get_cf(&cf, key) {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map_err(|e| format!("Failed to deserialize EVM logs: {}", e)),
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }
}
