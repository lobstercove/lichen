use super::*;
use crate::restrictions::{
    restriction_mode_blocks_transfer, RestrictionTarget, RestrictionTransferDirection,
    NATIVE_LICN_ASSET_ID,
};

impl TxProcessor {
    /// System program: Durable nonce operations (instruction type 28).
    ///
    /// Sub-opcodes (data[1]):
    ///   0 = Initialize — create a nonce account with stored blockhash
    ///   1 = Advance    — advance stored blockhash to latest (validates durable tx)
    ///   2 = Withdraw   — withdraw spores from nonce account (authority only)
    ///   3 = Authorize  — change nonce authority to a new pubkey
    pub(super) fn system_nonce(&self, ix: &Instruction) -> Result<(), String> {
        if ix.data.len() < 2 {
            return Err("Nonce: missing sub-opcode".to_string());
        }
        let sub = ix.data[1];
        match sub {
            0 => self.nonce_initialize(ix),
            1 => self.nonce_advance(ix),
            2 => self.nonce_withdraw(ix),
            3 => self.nonce_authorize(ix),
            _ => Err(format!("Nonce: unknown sub-opcode {}", sub)),
        }
    }

    /// Initialize a new nonce account.
    /// Data: [28, 0, authority(32)]   Accounts: [funder, nonce_account]
    pub(super) fn nonce_initialize(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("NonceInitialize requires [funder, nonce_account]".to_string());
        }
        if ix.data.len() < 34 {
            return Err("NonceInitialize: missing authority pubkey".to_string());
        }

        let funder = ix.accounts[0];
        let nonce_pk = ix.accounts[1];

        let mut authority_bytes = [0u8; 32];
        authority_bytes.copy_from_slice(&ix.data[2..34]);
        let authority = Pubkey(authority_bytes);

        if self.b_get_account(&nonce_pk)?.is_some() {
            return Err("NonceInitialize: nonce account already exists".to_string());
        }

        let funder_account = self
            .b_get_account(&funder)?
            .ok_or("NonceInitialize: funder account not found")?;
        if funder_account.spendable < NONCE_ACCOUNT_MIN_BALANCE {
            return Err(format!(
                "NonceInitialize: funder needs at least {} spores",
                NONCE_ACCOUNT_MIN_BALANCE
            ));
        }

        let last_slot = self.b_get_last_slot().unwrap_or(0);
        let stored_blockhash = self
            .state
            .get_block_by_slot(last_slot)?
            .map(|b| b.hash())
            .unwrap_or_default();

        let nonce_state = NonceState {
            authority,
            blockhash: stored_blockhash,
            fee_per_signature: BASE_FEE,
        };

        let mut nonce_data =
            bincode::serialize(&nonce_state).map_err(|e| format!("NonceInit serialize: {}", e))?;
        nonce_data.insert(0, NONCE_ACCOUNT_MARKER);

        self.b_transfer(&funder, &nonce_pk, NONCE_ACCOUNT_MIN_BALANCE)?;

        let mut nonce_account = self
            .b_get_account(&nonce_pk)?
            .ok_or("NonceInitialize: nonce account disappeared after transfer")?;
        nonce_account.data = nonce_data;
        nonce_account.owner = SYSTEM_PROGRAM_ID;
        self.b_put_account(&nonce_pk, &nonce_account)?;

        Ok(())
    }

    /// Advance the durable nonce — updates stored blockhash to latest.
    pub(super) fn nonce_advance(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("NonceAdvance requires [authority, nonce_account]".to_string());
        }

        let authority = ix.accounts[0];
        let nonce_pk = ix.accounts[1];
        let nonce_account = self
            .b_get_account(&nonce_pk)?
            .ok_or("NonceAdvance: nonce account not found")?;

        let nonce_state = Self::decode_nonce_state(&nonce_account.data)?;

        if authority != nonce_state.authority {
            return Err("NonceAdvance: signer is not the nonce authority".to_string());
        }

        let last_slot = self.b_get_last_slot().unwrap_or(0);
        let new_blockhash = self
            .state
            .get_block_by_slot(last_slot)?
            .map(|b| b.hash())
            .unwrap_or_default();

        if new_blockhash == nonce_state.blockhash {
            return Err("NonceAdvance: blockhash has not changed since last advance".to_string());
        }

        let updated = NonceState {
            authority: nonce_state.authority,
            blockhash: new_blockhash,
            fee_per_signature: BASE_FEE,
        };

        let mut data =
            bincode::serialize(&updated).map_err(|e| format!("NonceAdvance serialize: {}", e))?;
        data.insert(0, NONCE_ACCOUNT_MARKER);

        let mut acct = nonce_account;
        acct.data = data;
        self.b_put_account(&nonce_pk, &acct)?;

        Ok(())
    }

    /// Withdraw spores from a nonce account (authority only).
    pub(super) fn nonce_withdraw(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 3 {
            return Err("NonceWithdraw requires [authority, nonce_account, recipient]".to_string());
        }
        if ix.data.len() < 10 {
            return Err("NonceWithdraw: missing amount".to_string());
        }

        let authority = ix.accounts[0];
        let nonce_pk = ix.accounts[1];
        let recipient = ix.accounts[2];

        let nonce_account = self
            .b_get_account(&nonce_pk)?
            .ok_or("NonceWithdraw: nonce account not found")?;
        let nonce_state = Self::decode_nonce_state(&nonce_account.data)?;

        if authority != nonce_state.authority {
            return Err("NonceWithdraw: signer is not the nonce authority".to_string());
        }

        let amount = u64::from_le_bytes(
            ix.data[2..10]
                .try_into()
                .map_err(|_| "NonceWithdraw: invalid amount bytes")?,
        );

        if amount == 0 {
            return Err("NonceWithdraw: amount must be > 0".to_string());
        }

        let value_exit_amount = amount.min(nonce_account.spores);
        self.ensure_nonce_authority_value_exit_not_restricted(&authority, value_exit_amount)?;

        if amount >= nonce_account.spores {
            let full_amount = nonce_account.spores;
            self.b_transfer(&nonce_pk, &recipient, full_amount)?;
            let mut acct = self
                .b_get_account(&nonce_pk)?
                .unwrap_or_else(|| Account::new(0, nonce_pk));
            acct.data.clear();
            self.b_put_account(&nonce_pk, &acct)?;
        } else {
            self.b_transfer(&nonce_pk, &recipient, amount)?;
        }

        Ok(())
    }

    fn ensure_nonce_authority_value_exit_not_restricted(
        &self,
        authority: &Pubkey,
        amount: u64,
    ) -> Result<(), String> {
        let slot = self.b_get_last_slot().unwrap_or(0);
        let authority_spendable = self
            .b_get_account(authority)?
            .map(|account| account.spendable)
            .unwrap_or(0);

        let account_target = RestrictionTarget::Account(*authority);
        for record in self.b_get_active_restrictions_for_target(&account_target, slot, 0)? {
            if restriction_mode_blocks_transfer(
                &record.mode,
                RestrictionTransferDirection::Outgoing,
                amount,
                authority_spendable,
            ) {
                return Err(format!(
                    "NonceWithdraw: authority value exit blocked by active account restriction {}",
                    record.id
                ));
            }
        }

        let account_asset_target = RestrictionTarget::AccountAsset {
            account: *authority,
            asset: NATIVE_LICN_ASSET_ID,
        };
        for record in self.b_get_active_restrictions_for_target(&account_asset_target, slot, 0)? {
            if restriction_mode_blocks_transfer(
                &record.mode,
                RestrictionTransferDirection::Outgoing,
                amount,
                authority_spendable,
            ) {
                return Err(format!(
                    "NonceWithdraw: authority value exit blocked by active account-asset restriction {}",
                    record.id
                ));
            }
        }

        Ok(())
    }

    /// Change the nonce authority.
    pub(super) fn nonce_authorize(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("NonceAuthorize requires [authority, nonce_account]".to_string());
        }
        if ix.data.len() < 34 {
            return Err("NonceAuthorize: missing new authority pubkey".to_string());
        }

        let authority = ix.accounts[0];
        let nonce_pk = ix.accounts[1];
        let nonce_account = self
            .b_get_account(&nonce_pk)?
            .ok_or("NonceAuthorize: nonce account not found")?;
        let nonce_state = Self::decode_nonce_state(&nonce_account.data)?;

        if authority != nonce_state.authority {
            return Err("NonceAuthorize: signer is not the nonce authority".to_string());
        }

        let mut new_auth_bytes = [0u8; 32];
        new_auth_bytes.copy_from_slice(&ix.data[2..34]);
        let new_authority = Pubkey(new_auth_bytes);

        if new_authority == Pubkey([0u8; 32]) {
            return Err("NonceAuthorize: new authority cannot be the zero pubkey".to_string());
        }

        let updated = NonceState {
            authority: new_authority,
            blockhash: nonce_state.blockhash,
            fee_per_signature: nonce_state.fee_per_signature,
        };

        let mut data =
            bincode::serialize(&updated).map_err(|e| format!("NonceAuthorize serialize: {}", e))?;
        data.insert(0, NONCE_ACCOUNT_MARKER);

        let mut acct = nonce_account;
        acct.data = data;
        self.b_put_account(&nonce_pk, &acct)?;

        Ok(())
    }

    /// Decode a `NonceState` from the account's data field (skipping the marker byte).
    pub(super) fn decode_nonce_state(data: &[u8]) -> Result<NonceState, String> {
        if data.is_empty() || data[0] != NONCE_ACCOUNT_MARKER {
            return Err("Not a nonce account".to_string());
        }
        bincode::deserialize(&data[1..]).map_err(|e| format!("Invalid nonce state: {}", e))
    }
}
