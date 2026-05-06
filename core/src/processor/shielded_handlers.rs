use super::*;
use crate::restrictions::{ProtocolModuleId, RestrictionTransferDirection};

impl TxProcessor {
    /// System instruction type 23: Shield deposit (transparent → shielded).
    ///
    /// Debits `amount` from the sender's spendable balance, inserts a new
    /// commitment leaf into the shielded Merkle tree, and increments the
    /// pool's `total_shielded` balance.
    ///
    /// Data layout:
    /// ```text
    ///   [0]       = 23 (type tag)
    ///   [1..9]    = amount (u64 LE, spores)
    ///   [9..41]   = commitment (32 bytes, Poseidon hash of value||blinding)
    ///   [41..]    = Plonky3 STARK proof bytes
    /// ```
    /// Public inputs (derived from data): canonical Goldilocks words for
    /// [amount, commitment]
    /// accounts[0] = sender (debited)
    #[cfg(feature = "zk")]
    pub(super) fn system_shield_deposit(&self, ix: &Instruction) -> Result<(), String> {
        use crate::zk::{ProofType, ShieldAirPublicValues, ZkProof};

        let required_len = 42;

        if ix.data.len() < required_len {
            return Err(format!(
                "Shield: insufficient data length {} (expected >={})",
                ix.data.len(),
                required_len
            ));
        }
        if ix.accounts.is_empty() {
            return Err("Shield: requires [sender] account".to_string());
        }

        let sender = &ix.accounts[0];

        let amount = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Shield: invalid amount encoding".to_string())?,
        );
        if amount == 0 {
            return Err("Shield: amount must be non-zero".to_string());
        }

        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&ix.data[9..41]);

        self.ensure_protocol_module_not_paused(ProtocolModuleId::Shielded, "Shield")?;

        let mut sender_acct = self
            .b_get_account(sender)?
            .ok_or_else(|| "Shield: sender account not found".to_string())?;
        self.ensure_native_account_direction_not_restricted(
            sender,
            RestrictionTransferDirection::Outgoing,
            amount,
            sender_acct.spendable,
            "Shield",
            "sender",
        )?;
        if sender_acct.spendable < amount {
            return Err(format!(
                "Shield: insufficient balance ({} < {})",
                sender_acct.spendable, amount
            ));
        }

        let proof_bytes = ix.data[41..].to_vec();
        let zk_proof = ZkProof::plonky3(
            ProofType::Shield,
            proof_bytes,
            ShieldAirPublicValues::new(amount, commitment)
                .to_stark_public_inputs()
                .into_iter()
                .collect(),
        );

        {
            let verifier = self
                .zk_verifier
                .lock()
                .map_err(|e| format!("Shield: verifier lock poisoned: {}", e))?;
            let valid = verifier
                .verify(&zk_proof)
                .map_err(|e| format!("Shield: proof verification error: {}", e))?;
            if !valid {
                return Err("Shield: ZK proof verification failed".to_string());
            }
        }

        sender_acct.spendable = sender_acct.spendable.saturating_sub(amount);
        sender_acct.spores = sender_acct
            .spendable
            .saturating_add(sender_acct.staked)
            .saturating_add(sender_acct.locked);
        self.b_put_account(sender, &sender_acct)?;

        {
            let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(batch) = guard.as_mut() {
                let mut pool = batch.get_shielded_pool_state()?;
                let index = pool.commitment_count;
                batch.insert_shielded_commitment(index, &commitment)?;
                pool.commitment_count += 1;
                pool.shield_count = pool.shield_count.saturating_add(1);
                pool.total_shielded = pool
                    .total_shielded
                    .checked_add(amount)
                    .ok_or_else(|| "Shield: pool balance overflow".to_string())?;
                let leaves = batch.get_all_shielded_commitments(pool.commitment_count)?;
                let mut tree = crate::zk::MerkleTree::new();
                for leaf in &leaves {
                    tree.insert(*leaf);
                }
                pool.merkle_root = tree.root();
                batch.put_shielded_pool_state(&pool)?;
            } else {
                let mut pool = self.state.get_shielded_pool_state()?;
                let index = pool.commitment_count;
                self.state.insert_shielded_commitment(index, &commitment)?;
                pool.commitment_count += 1;
                pool.shield_count = pool.shield_count.saturating_add(1);
                pool.total_shielded = pool
                    .total_shielded
                    .checked_add(amount)
                    .ok_or_else(|| "Shield: pool balance overflow".to_string())?;
                let leaves = self
                    .state
                    .get_all_shielded_commitments(pool.commitment_count)?;
                let mut tree = crate::zk::MerkleTree::new();
                for leaf in &leaves {
                    tree.insert(*leaf);
                }
                pool.merkle_root = tree.root();
                self.state.put_shielded_pool_state(&pool)?;
            }
        }

        Ok(())
    }

    /// System instruction type 24: Unshield withdraw (shielded → transparent).
    ///
    /// Verifies a ZK proof that the caller owns a shielded note, marks the
    /// note's nullifier as spent, credits the recipient, and decrements the
    /// pool's `total_shielded` balance.
    #[cfg(feature = "zk")]
    pub(super) fn system_unshield_withdraw(&self, ix: &Instruction) -> Result<(), String> {
        use crate::zk::merkle::is_canonical_scalar_bytes;
        use crate::zk::{
            recipient_hash, recipient_preimage_from_bytes, ProofType, UnshieldAirPublicValues,
            ZkProof,
        };

        let required_len = 106;

        if ix.data.len() < required_len {
            return Err(format!(
                "Unshield: insufficient data length {} (expected >={})",
                ix.data.len(),
                required_len
            ));
        }
        if ix.accounts.is_empty() {
            return Err("Unshield: requires [recipient] account".to_string());
        }

        let recipient_pubkey = &ix.accounts[0];

        let amount = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Unshield: invalid amount encoding".to_string())?,
        );
        if amount == 0 {
            return Err("Unshield: amount must be non-zero".to_string());
        }

        let mut nullifier = [0u8; 32];
        nullifier.copy_from_slice(&ix.data[9..41]);

        if !is_canonical_scalar_bytes(&nullifier) {
            return Err(format!(
                "Unshield: non-canonical nullifier encoding: {}",
                hex::encode(nullifier)
            ));
        }

        let mut merkle_root = [0u8; 32];
        merkle_root.copy_from_slice(&ix.data[41..73]);

        let mut recipient_bytes = [0u8; 32];
        recipient_bytes.copy_from_slice(&ix.data[73..105]);

        let proof_bytes = ix.data[105..].to_vec();

        let recipient_preimage = recipient_preimage_from_bytes(recipient_pubkey.0);
        let expected_recipient_bytes = recipient_hash(&recipient_preimage);
        if recipient_bytes != expected_recipient_bytes {
            return Err(
                "Unshield: recipient public input does not match recipient account".to_string(),
            );
        }

        let mut recipient_acct = self
            .b_get_account(recipient_pubkey)?
            .unwrap_or_else(|| crate::Account::new(0, crate::SYSTEM_PROGRAM_ID));
        self.ensure_native_account_direction_not_restricted(
            recipient_pubkey,
            RestrictionTransferDirection::Incoming,
            amount,
            recipient_acct.spendable,
            "Unshield",
            "recipient",
        )?;

        {
            let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            let pool = if let Some(batch) = guard.as_ref() {
                batch.get_shielded_pool_state()?
            } else {
                self.state.get_shielded_pool_state()?
            };
            if pool.merkle_root != merkle_root {
                return Err("Unshield: merkle root does not match current pool state".to_string());
            }
            if amount > pool.total_shielded {
                return Err(format!(
                    "Unshield: insufficient shielded pool balance ({} < {})",
                    pool.total_shielded, amount
                ));
            }
        }

        {
            let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            let spent = if let Some(batch) = guard.as_ref() {
                batch.is_nullifier_spent(&nullifier)?
            } else {
                self.state.is_nullifier_spent(&nullifier)?
            };
            if spent {
                return Err(format!(
                    "Unshield: nullifier already spent: {}",
                    hex::encode(nullifier)
                ));
            }
        }

        let zk_proof = ZkProof::plonky3(
            ProofType::Unshield,
            proof_bytes,
            UnshieldAirPublicValues::new(merkle_root, nullifier, amount, recipient_bytes)
                .to_stark_public_inputs()
                .into_iter()
                .collect(),
        );

        {
            let verifier = self
                .zk_verifier
                .lock()
                .map_err(|e| format!("Unshield: verifier lock poisoned: {}", e))?;
            let valid = verifier
                .verify(&zk_proof)
                .map_err(|e| format!("Unshield: proof verification error: {}", e))?;
            if !valid {
                return Err("Unshield: ZK proof verification failed".to_string());
            }
        }

        recipient_acct.spendable = recipient_acct.spendable.saturating_add(amount);
        recipient_acct.spores = recipient_acct
            .spendable
            .saturating_add(recipient_acct.staked)
            .saturating_add(recipient_acct.locked);
        self.b_put_account(recipient_pubkey, &recipient_acct)?;

        {
            let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(batch) = guard.as_mut() {
                batch.mark_nullifier_spent(&nullifier)?;
                let mut pool = batch.get_shielded_pool_state()?;
                pool.unshield_count = pool.unshield_count.saturating_add(1);
                pool.nullifier_count = pool.nullifier_count.saturating_add(1);
                pool.total_shielded = pool
                    .total_shielded
                    .checked_sub(amount)
                    .ok_or_else(|| "Unshield: shielded pool underflow".to_string())?;
                batch.put_shielded_pool_state(&pool)?;
            } else {
                self.state.mark_nullifier_spent(&nullifier)?;
                let mut pool = self.state.get_shielded_pool_state()?;
                pool.unshield_count = pool.unshield_count.saturating_add(1);
                pool.nullifier_count = pool.nullifier_count.saturating_add(1);
                pool.total_shielded = pool
                    .total_shielded
                    .checked_sub(amount)
                    .ok_or_else(|| "Unshield: shielded pool underflow".to_string())?;
                self.state.put_shielded_pool_state(&pool)?;
            }
        }

        Ok(())
    }

    /// System instruction type 25: Shielded transfer (shielded → shielded).
    ///
    /// 2-in-2-out private transfer. Spends two existing notes and creates two
    /// new commitments with zero-knowledge proof of value conservation.
    #[cfg(feature = "zk")]
    pub(super) fn system_shielded_transfer(&self, ix: &Instruction) -> Result<(), String> {
        use crate::zk::merkle::is_canonical_scalar_bytes;
        use crate::zk::{ProofType, TransferAirPublicValues, ZkProof};

        let required_len = 162;

        if ix.data.len() < required_len {
            return Err(format!(
                "ShieldedTransfer: insufficient data length {} (expected >={})",
                ix.data.len(),
                required_len
            ));
        }

        self.ensure_protocol_module_not_paused(ProtocolModuleId::Shielded, "ShieldedTransfer")?;

        let mut nullifier_a = [0u8; 32];
        nullifier_a.copy_from_slice(&ix.data[1..33]);

        let mut nullifier_b = [0u8; 32];
        nullifier_b.copy_from_slice(&ix.data[33..65]);

        for (label, nul) in [("A", &nullifier_a), ("B", &nullifier_b)] {
            if !is_canonical_scalar_bytes(nul) {
                return Err(format!(
                    "ShieldedTransfer: non-canonical nullifier {} encoding: {}",
                    label,
                    hex::encode(nul)
                ));
            }
        }

        let mut commitment_c = [0u8; 32];
        commitment_c.copy_from_slice(&ix.data[65..97]);

        let mut commitment_d = [0u8; 32];
        commitment_d.copy_from_slice(&ix.data[97..129]);

        let mut merkle_root = [0u8; 32];
        merkle_root.copy_from_slice(&ix.data[129..161]);

        let proof_bytes = ix.data[161..].to_vec();

        {
            let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            let pool = if let Some(batch) = guard.as_ref() {
                batch.get_shielded_pool_state()?
            } else {
                self.state.get_shielded_pool_state()?
            };
            if pool.merkle_root != merkle_root {
                return Err(
                    "ShieldedTransfer: merkle root does not match current pool state".to_string(),
                );
            }
        }

        {
            let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            for (label, nullifier) in [("A", &nullifier_a), ("B", &nullifier_b)] {
                let spent = if let Some(batch) = guard.as_ref() {
                    batch.is_nullifier_spent(nullifier)?
                } else {
                    self.state.is_nullifier_spent(nullifier)?
                };
                if spent {
                    return Err(format!(
                        "ShieldedTransfer: nullifier {} already spent: {}",
                        label,
                        hex::encode(nullifier)
                    ));
                }
            }
            if nullifier_a == nullifier_b {
                return Err("ShieldedTransfer: duplicate nullifiers".to_string());
            }
        }

        let zk_proof = ZkProof::plonky3(
            ProofType::Transfer,
            proof_bytes,
            TransferAirPublicValues::new(
                merkle_root,
                nullifier_a,
                nullifier_b,
                commitment_c,
                commitment_d,
            )
            .to_stark_public_inputs()
            .into_iter()
            .collect(),
        );

        {
            let verifier = self
                .zk_verifier
                .lock()
                .map_err(|e| format!("ShieldedTransfer: verifier lock poisoned: {}", e))?;
            let valid = verifier
                .verify(&zk_proof)
                .map_err(|e| format!("ShieldedTransfer: proof verification error: {}", e))?;
            if !valid {
                return Err("ShieldedTransfer: ZK proof verification failed".to_string());
            }
        }

        {
            let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(batch) = guard.as_mut() {
                batch.mark_nullifier_spent(&nullifier_a)?;
                batch.mark_nullifier_spent(&nullifier_b)?;
                let mut pool = batch.get_shielded_pool_state()?;
                pool.transfer_count = pool.transfer_count.saturating_add(1);
                pool.nullifier_count = pool.nullifier_count.saturating_add(2);
                let idx0 = pool.commitment_count;
                batch.insert_shielded_commitment(idx0, &commitment_c)?;
                batch.insert_shielded_commitment(idx0 + 1, &commitment_d)?;
                pool.commitment_count += 2;
                let leaves = batch.get_all_shielded_commitments(pool.commitment_count)?;
                let mut tree = crate::zk::MerkleTree::new();
                for leaf in &leaves {
                    tree.insert(*leaf);
                }
                pool.merkle_root = tree.root();
                batch.put_shielded_pool_state(&pool)?;
            } else {
                self.state.mark_nullifier_spent(&nullifier_a)?;
                self.state.mark_nullifier_spent(&nullifier_b)?;
                let mut pool = self.state.get_shielded_pool_state()?;
                pool.transfer_count = pool.transfer_count.saturating_add(1);
                pool.nullifier_count = pool.nullifier_count.saturating_add(2);
                let idx0 = pool.commitment_count;
                self.state.insert_shielded_commitment(idx0, &commitment_c)?;
                self.state
                    .insert_shielded_commitment(idx0 + 1, &commitment_d)?;
                pool.commitment_count += 2;
                let leaves = self
                    .state
                    .get_all_shielded_commitments(pool.commitment_count)?;
                let mut tree = crate::zk::MerkleTree::new();
                for leaf in &leaves {
                    tree.insert(*leaf);
                }
                pool.merkle_root = tree.root();
                self.state.put_shielded_pool_state(&pool)?;
            }
        }

        Ok(())
    }
}
