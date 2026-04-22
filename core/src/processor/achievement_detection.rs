use super::*;

impl TxProcessor {
    // ═══════════════════════════════════════════════════════════════════════════
    // ACHIEVEMENT AUTO-DETECTION (post-execution hook)
    // ═══════════════════════════════════════════════════════════════════════════

    /// Detect and auto-award achievements after a successful transaction.
    /// Writes directly to LichenID's CF_CONTRACT_STORAGE. Best-effort only.
    pub(super) fn detect_and_award_achievements(&self, tx: &Transaction) -> Result<(), String> {
        // Resolve LichenID contract address from symbol registry
        let lichenid_addr = match self.state.get_symbol_registry("YID") {
            Ok(Some(entry)) => entry.program,
            _ => return Ok(()),
        };

        let first_ix = tx.message.instructions.first();
        let ix = match first_ix {
            Some(ix) => ix,
            None => return Ok(()),
        };
        let caller = match ix.accounts.first() {
            Some(acc) => *acc,
            None => return Ok(()),
        };

        let hex = Self::pubkey_to_hex(&caller);
        let identity_key = format!("id:{}", hex);
        if self
            .state
            .get_contract_storage(&lichenid_addr, identity_key.as_bytes())
            .ok()
            .flatten()
            .is_none()
        {
            return Ok(());
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let timestamp = current_slot;

        self.increment_contribution(&lichenid_addr, &hex, 0)?;

        if ix.program_id == SYSTEM_PROGRAM_ID {
            let op = ix.data.first().copied().unwrap_or(255);
            match op {
                0 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    let amount = if ix.data.len() >= 9 {
                        u64::from_le_bytes(ix.data[1..9].try_into().unwrap_or([0; 8]))
                    } else {
                        0
                    };
                    if amount >= 100 * 1_000_000_000 {
                        self.award_ach(&lichenid_addr, &caller, &hex, 106, timestamp)?;
                    }
                    if amount >= 1_000 * 1_000_000_000 {
                        self.award_ach(&lichenid_addr, &caller, &hex, 107, timestamp)?;
                    }
                }
                6 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 63, timestamp)?;
                }
                7 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 64, timestamp)?;
                }
                8 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 65, timestamp)?;
                }
                9 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 41, timestamp)?;
                }
                10 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 42, timestamp)?;
                }
                11 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                }
                12 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 108, timestamp)?;
                }
                13 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 43, timestamp)?;
                    let amount = if ix.data.len() >= 9 {
                        u64::from_le_bytes(ix.data[1..9].try_into().unwrap_or([0; 8]))
                    } else {
                        0
                    };
                    let tier = ix.data.get(9).copied().unwrap_or(0);
                    if tier >= 1 {
                        self.award_ach(&lichenid_addr, &caller, &hex, 44, timestamp)?;
                    }
                    if tier >= 3 {
                        self.award_ach(&lichenid_addr, &caller, &hex, 45, timestamp)?;
                    }
                    if amount >= 10_000 * 1_000_000_000 {
                        self.award_ach(&lichenid_addr, &caller, &hex, 46, timestamp)?;
                    }
                }
                14 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                }
                15 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 47, timestamp)?;
                }
                16 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 48, timestamp)?;
                }
                23 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 57, timestamp)?;
                }
                24 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 58, timestamp)?;
                }
                25 => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                    self.award_ach(&lichenid_addr, &caller, &hex, 59, timestamp)?;
                }
                _ => {
                    self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
                }
            }
        } else if ix.program_id == CONTRACT_PROGRAM_ID {
            self.award_ach(&lichenid_addr, &caller, &hex, 1, timestamp)?;
            if let Ok(json_str) = std::str::from_utf8(&ix.data) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if val.get("Deploy").is_some() {
                        self.award_ach(&lichenid_addr, &caller, &hex, 3, timestamp)?;
                        self.increment_contribution(&lichenid_addr, &hex, 2)?;
                    }
                    if let Some(call) = val.get("Call") {
                        let func = call.get("function").and_then(|f| f.as_str()).unwrap_or("");
                        let contract_addr = ix.accounts.get(1).copied();
                        let contract_symbol = contract_addr.and_then(|addr| {
                            self.state
                                .get_symbol_registry_by_program(&addr)
                                .ok()
                                .flatten()
                                .map(|e| e.symbol)
                        });
                        let sym = contract_symbol.as_deref().unwrap_or("");

                        if sym == "YID" {
                            match func {
                                "register_identity" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 109, timestamp)?;
                                }
                                "register_name" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 9, timestamp)?;
                                    self.award_ach(&lichenid_addr, &caller, &hex, 12, timestamp)?;
                                }
                                "update_profile" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 110, timestamp)?;
                                }
                                "vouch" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 111, timestamp)?;
                                    self.increment_contribution(&lichenid_addr, &hex, 4)?;
                                }
                                "create_agent" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 112, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "DEX" || sym == "DEX_CORE" || sym == "LICHENSWAP" {
                            match func {
                                "swap" | "swap_exact_input" | "swap_exact_output"
                                | "execute_swap" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 13, timestamp)?;
                                }
                                "add_liquidity" | "provide_liquidity" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 14, timestamp)?;
                                }
                                "remove_liquidity" | "withdraw_liquidity" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 15, timestamp)?;
                                }
                                _ => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 16, timestamp)?;
                                }
                            }
                        }

                        if sym == "DEXROUTER" {
                            self.award_ach(&lichenid_addr, &caller, &hex, 17, timestamp)?;
                        }

                        if sym == "DEXMARGIN" {
                            match func {
                                "open_position" | "open_long" | "open_short" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 18, timestamp)?;
                                }
                                "close_position" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 19, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "DEXGOV" || sym == "DAO" {
                            match func {
                                "create_proposal" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 71, timestamp)?;
                                }
                                "vote" | "cast_vote" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 2, timestamp)?;
                                    self.award_ach(&lichenid_addr, &caller, &hex, 72, timestamp)?;
                                    self.increment_contribution(&lichenid_addr, &hex, 1)?;
                                }
                                "delegate" | "delegate_votes" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 73, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "DEXREWARDS" {
                            match func {
                                "claim" | "claim_rewards" | "harvest" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 20, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "ANALYTICS" {
                            self.award_ach(&lichenid_addr, &caller, &hex, 21, timestamp)?;
                        }

                        if sym == "LEND" {
                            match func {
                                "deposit" | "supply" | "lend" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 31, timestamp)?;
                                }
                                "borrow" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 32, timestamp)?;
                                }
                                "repay" | "repay_loan" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 33, timestamp)?;
                                }
                                "liquidate" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 34, timestamp)?;
                                }
                                "withdraw" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 35, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "BRIDGE" {
                            match func {
                                "deposit" | "bridge_in" | "lock" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 51, timestamp)?;
                                }
                                "withdraw" | "bridge_out" | "unlock" | "claim" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 52, timestamp)?;
                                }
                                _ => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 53, timestamp)?;
                                }
                            }
                        }

                        if sym == "WETH" || sym == "WBNB" || sym == "WSOL" {
                            match func {
                                "wrap" | "deposit" | "mint" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 54, timestamp)?;
                                }
                                "unwrap" | "withdraw" | "burn" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 55, timestamp)?;
                                }
                                "transfer" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 56, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "LUSD" {
                            match func {
                                "mint" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 36, timestamp)?;
                                }
                                "redeem" | "burn" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 37, timestamp)?;
                                }
                                "transfer" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 38, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "SHIELDED" {
                            self.award_ach(&lichenid_addr, &caller, &hex, 60, timestamp)?;
                        }

                        if sym == "MARKET" {
                            match func {
                                "list" | "create_listing" | "list_nft" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 66, timestamp)?;
                                }
                                "buy" | "purchase" | "buy_nft" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 67, timestamp)?;
                                }
                                "make_offer" | "bid" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 68, timestamp)?;
                                }
                                "accept_offer" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 69, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "PUNKS" {
                            self.award_ach(&lichenid_addr, &caller, &hex, 70, timestamp)?;
                        }

                        if sym == "AUCTION" {
                            match func {
                                "create_auction" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 91, timestamp)?;
                                }
                                "place_bid" | "bid" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 92, timestamp)?;
                                }
                                "claim" | "settle" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 93, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "ORACLE" {
                            match func {
                                "submit_price" | "update_price" | "report" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 81, timestamp)?;
                                }
                                _ => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 82, timestamp)?;
                                }
                            }
                        }

                        if sym == "MOSS" {
                            match func {
                                "upload" | "store" | "put" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 86, timestamp)?;
                                }
                                "download" | "get" | "retrieve" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 87, timestamp)?;
                                }
                                _ => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 88, timestamp)?;
                                }
                            }
                        }

                        if sym == "BOUNTY" {
                            match func {
                                "create_bounty" | "post_bounty" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 96, timestamp)?;
                                }
                                "submit_work" | "claim_bounty" | "complete" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 97, timestamp)?;
                                }
                                "approve" | "accept_submission" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 98, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "PREDICT" {
                            match func {
                                "create_market" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 101, timestamp)?;
                                }
                                "predict" | "place_bet" | "buy_shares" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 102, timestamp)?;
                                }
                                "resolve" | "settle" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 103, timestamp)?;
                                }
                                "claim" | "redeem" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 104, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "COMPUTE" {
                            match func {
                                "register_provider" | "offer_compute" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 113, timestamp)?;
                                }
                                "request_compute" | "submit_job" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 114, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "SPOREPAY" {
                            match func {
                                "create_invoice" | "create_payment" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 115, timestamp)?;
                                }
                                "pay" | "send_payment" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 116, timestamp)?;
                                }
                                "create_subscription" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 117, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "SPOREPUMP" {
                            match func {
                                "create_token" | "launch" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 118, timestamp)?;
                                }
                                "buy" | "purchase" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 119, timestamp)?;
                                }
                                "sell" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 120, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "SPOREVAULT" {
                            match func {
                                "deposit" | "lock" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 121, timestamp)?;
                                }
                                "withdraw" | "unlock" => {
                                    self.award_ach(&lichenid_addr, &caller, &hex, 122, timestamp)?;
                                }
                                _ => {}
                            }
                        }

                        if sym == "LICN" {
                            self.award_ach(&lichenid_addr, &caller, &hex, 123, timestamp)?;
                        }

                        self.award_ach(&lichenid_addr, &caller, &hex, 124, timestamp)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn pubkey_to_hex(pubkey: &Pubkey) -> String {
        hex::encode(pubkey.0)
    }

    fn award_ach(
        &self,
        lichenid_addr: &Pubkey,
        _caller: &Pubkey,
        hex: &str,
        achievement_id: u8,
        timestamp: u64,
    ) -> Result<(), String> {
        let key = format!("ach:{}:{:02}", hex, achievement_id);
        let key_bytes = key.as_bytes();

        if let Ok(Some(_)) = self.state.get_contract_storage(lichenid_addr, key_bytes) {
            return Ok(());
        }

        let mut ach_data = Vec::with_capacity(9);
        ach_data.push(achievement_id);
        ach_data.extend_from_slice(&timestamp.to_le_bytes());
        self.b_put_contract_storage(lichenid_addr, key_bytes, &ach_data)?;

        let count_key = format!("ach_count:{}", hex);
        let count_bytes = count_key.as_bytes();
        let prev = self
            .state
            .get_contract_storage(lichenid_addr, count_bytes)
            .ok()
            .flatten()
            .and_then(|d| {
                if d.len() >= 8 {
                    Some(u64::from_le_bytes(d[..8].try_into().unwrap_or([0; 8])))
                } else {
                    None
                }
            })
            .unwrap_or(0);
        self.b_put_contract_storage(lichenid_addr, count_bytes, &(prev + 1).to_le_bytes())?;

        Ok(())
    }

    fn increment_contribution(
        &self,
        lichenid_addr: &Pubkey,
        hex: &str,
        contribution_type: u8,
    ) -> Result<(), String> {
        let key = format!("cont:{}:{}", hex, contribution_type);
        let key_bytes = key.as_bytes();
        let prev = self
            .state
            .get_contract_storage(lichenid_addr, key_bytes)
            .ok()
            .flatten()
            .and_then(|d| {
                if d.len() >= 8 {
                    Some(u64::from_le_bytes(d[..8].try_into().unwrap_or([0; 8])))
                } else {
                    None
                }
            })
            .unwrap_or(0);
        self.b_put_contract_storage(lichenid_addr, key_bytes, &(prev + 1).to_le_bytes())?;
        Ok(())
    }
}
