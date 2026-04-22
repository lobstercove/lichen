use super::*;

impl TxProcessor {
    pub(super) fn apply_rent(&self, tx: &Transaction) -> Result<(), String> {
        let current_slot = self.b_get_last_slot()?;
        if current_slot == 0 {
            return Ok(());
        }

        let current_epoch = slot_to_epoch(current_slot);

        let mut accounts = HashSet::new();
        for ix in &tx.message.instructions {
            for account in &ix.accounts {
                accounts.insert(*account);
            }
        }

        let (rent_rate, _rent_free_kb) = self.state.get_rent_params()?;

        // Convert monthly rate to per-epoch rate:
        // SLOTS_PER_MONTH = 216_000 * 30 = 6_480_000
        // SLOTS_PER_EPOCH = 432_000
        // epochs_per_month ≈ 15
        let rent_rate_per_epoch = rent_rate.saturating_mul(SLOTS_PER_EPOCH) / SLOTS_PER_MONTH;

        let mut total_rent_collected: u64 = 0;

        for pubkey in accounts {
            let mut account = match self.b_get_account(&pubkey)? {
                Some(acc) => acc,
                None => continue,
            };

            // Initialize rent_epoch on first touch
            if account.rent_epoch == 0 {
                account.rent_epoch = current_slot;
                self.b_put_account(&pubkey, &account)?;
                continue;
            }

            let last_rent_epoch = slot_to_epoch(account.rent_epoch);
            if current_epoch <= last_rent_epoch {
                continue;
            }
            let epochs_elapsed = current_epoch - last_rent_epoch;

            let data_len = account.data.len() as u64;

            // Free tier: accounts with ≤ 2KB data are exempt
            if data_len <= RENT_FREE_BYTES {
                account.rent_epoch = current_slot;
                // Exempt accounts reset missed epochs
                account.missed_rent_epochs = 0;
                self.b_put_account(&pubkey, &account)?;
                continue;
            }

            // Zero-balance accounts with no data: also exempt
            if account.spores == 0 && data_len == 0 {
                account.rent_epoch = current_slot;
                self.b_put_account(&pubkey, &account)?;
                continue;
            }

            // Graduated rent calculation
            let rent_per_epoch = compute_graduated_rent(data_len, rent_rate_per_epoch);
            let rent_due = epochs_elapsed.saturating_mul(rent_per_epoch);

            if rent_due > 0 {
                let actual_rent = rent_due.min(account.spendable);
                if actual_rent > 0 {
                    account
                        .deduct_spendable(actual_rent)
                        .map_err(|e| format!("Rent deduction failed: {}", e))?;
                    total_rent_collected = total_rent_collected.saturating_add(actual_rent);
                }

                if actual_rent < rent_due {
                    // Could not pay full rent — increment missed epochs
                    account.missed_rent_epochs =
                        account.missed_rent_epochs.saturating_add(epochs_elapsed);

                    // Mark dormant after 2+ consecutive missed epochs
                    if account.missed_rent_epochs >= DORMANCY_THRESHOLD_EPOCHS {
                        account.dormant = true;
                    }
                } else {
                    // Paid in full — reset missed counter
                    account.missed_rent_epochs = 0;
                }
            }

            account.rent_epoch = current_slot;
            self.b_put_account(&pubkey, &account)?;
        }

        // Credit collected rent to treasury
        if total_rent_collected > 0 {
            let treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "Treasury pubkey not set for rent credit".to_string())?;
            let mut treasury = self
                .b_get_account(&treasury_pubkey)?
                .unwrap_or_else(|| Account::new(0, treasury_pubkey));
            treasury.add_spendable(total_rent_collected)?;
            self.b_put_account(&treasury_pubkey, &treasury)?;
        }

        Ok(())
    }
}
