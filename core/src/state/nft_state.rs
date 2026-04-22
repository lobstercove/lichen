use super::*;

impl StateStore {
    pub fn index_nft_mint(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        owner: &Pubkey,
    ) -> Result<(), String> {
        self.add_nft_owner_index(owner, token)?;
        self.add_nft_collection_index(collection, token)?;
        Ok(())
    }

    pub fn index_nft_transfer(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        from: &Pubkey,
        to: &Pubkey,
    ) -> Result<(), String> {
        self.remove_nft_owner_index(from, token)?;
        self.add_nft_owner_index(to, token)?;
        self.add_nft_collection_index(collection, token)?;
        Ok(())
    }

    pub fn get_nft_tokens_by_owner(
        &self,
        owner: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Pubkey>, String> {
        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&owner.0);
        self.scan_nft_index(CF_NFT_BY_OWNER, &prefix, limit)
    }

    pub fn get_nft_tokens_by_collection(
        &self,
        collection: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Pubkey>, String> {
        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&collection.0);
        self.scan_nft_index(CF_NFT_BY_COLLECTION, &prefix, limit)
    }

    pub fn record_nft_activity(
        &self,
        activity: &crate::nft::NftActivity,
        sequence: u32,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_ACTIVITY)
            .ok_or_else(|| "NFT activity CF not found".to_string())?;

        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
        key.extend_from_slice(&activity.collection.0);
        key.extend_from_slice(&activity.slot.to_be_bytes());
        key.extend_from_slice(&sequence.to_be_bytes());
        key.extend_from_slice(&activity.token.0);

        let value = crate::nft::encode_nft_activity(activity)?;
        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store NFT activity: {}", e))
    }

    pub fn get_nft_activity_by_collection(
        &self,
        collection: &Pubkey,
        limit: usize,
    ) -> Result<Vec<crate::nft::NftActivity>, String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_ACTIVITY)
            .ok_or_else(|| "NFT activity CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&collection.0);

        let mut end_key = prefix.clone();
        end_key.extend_from_slice(&[0xFF; 48]);

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut items = Vec::with_capacity(limit);
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }

            let activity = crate::nft::decode_nft_activity(&value)?;
            items.push(activity);
            if items.len() >= limit {
                break;
            }
        }

        Ok(items)
    }

    fn add_nft_owner_index(&self, owner: &Pubkey, token: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&owner.0);
        key.extend_from_slice(&token.0);

        self.db
            .put_cf(&cf, key, [])
            .map_err(|e| format!("Failed to store NFT owner index: {}", e))
    }

    fn remove_nft_owner_index(&self, owner: &Pubkey, token: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&owner.0);
        key.extend_from_slice(&token.0);

        self.db
            .delete_cf(&cf, key)
            .map_err(|e| format!("Failed to delete NFT owner index: {}", e))
    }

    fn add_nft_collection_index(&self, collection: &Pubkey, token: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token.0);

        self.db
            .put_cf(&cf, key, [])
            .map_err(|e| format!("Failed to store NFT collection index: {}", e))
    }

    pub fn index_nft_token_id(
        &self,
        collection: &Pubkey,
        token_id: u64,
        token_account: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(44);
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        self.db
            .put_cf(&cf, &key, token_account.0)
            .map_err(|e| format!("Failed to index NFT token_id: {}", e))
    }

    pub fn nft_token_id_exists(&self, collection: &Pubkey, token_id: u64) -> Result<bool, String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(44);
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        match self.db.get_cf(&cf, &key) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    fn scan_nft_index(
        &self,
        cf_name: &str,
        prefix: &[u8],
        limit: usize,
    ) -> Result<Vec<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;

        let mut results = Vec::new();
        let iter = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::From(prefix, Direction::Forward));

        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(prefix) {
                break;
            }

            if key.len() < prefix.len() + 32 {
                continue;
            }

            let start = prefix.len();
            let end = start + 32;
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&key[start..end]);
            results.push(Pubkey(bytes));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }
}
