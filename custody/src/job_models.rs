use super::*;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SweepJob {
    pub(super) job_id: String,
    pub(super) deposit_id: String,
    pub(super) chain: String,
    pub(super) asset: String,
    pub(super) from_address: String,
    pub(super) to_treasury: String,
    pub(super) tx_hash: String,
    #[serde(default)]
    pub(super) amount: Option<String>,
    #[serde(default)]
    pub(super) credited_amount: Option<String>,
    #[serde(default)]
    pub(super) signatures: Vec<SignerSignature>,
    #[serde(default)]
    pub(super) sweep_tx_hash: Option<String>,
    #[serde(default)]
    pub(super) attempts: u32,
    #[serde(default)]
    pub(super) last_error: Option<String>,
    #[serde(default)]
    pub(super) next_attempt_at: Option<i64>,
    pub(super) status: String,
    pub(super) created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct CreditJob {
    pub(super) job_id: String,
    pub(super) deposit_id: String,
    pub(super) to_address: String,
    pub(super) amount_spores: u64,
    /// Source chain asset identifier ("sol", "eth", "usdt", "usdc")
    /// Determines which wrapped token contract to mint on Lichen.
    #[serde(default)]
    pub(super) source_asset: String,
    /// Source chain ("solana", "ethereum")
    #[serde(default)]
    pub(super) source_chain: String,
    pub(super) status: String,
    pub(super) tx_signature: Option<String>,
    #[serde(default)]
    pub(super) attempts: u32,
    #[serde(default)]
    pub(super) last_error: Option<String>,
    #[serde(default)]
    pub(super) next_attempt_at: Option<i64>,
    pub(super) created_at: i64,
}

/// Treasury reserve ledger entry — tracks actual stablecoin holdings per chain+asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ReserveLedgerEntry {
    pub(super) chain: String,
    pub(super) asset: String,
    pub(super) amount: u64,
    pub(super) last_updated: i64,
}

/// Rebalance job — swap one stablecoin for another on a given chain
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct RebalanceJob {
    pub(super) job_id: String,
    pub(super) chain: String,
    pub(super) from_asset: String,
    pub(super) to_asset: String,
    pub(super) amount: u64,
    pub(super) trigger: String,
    pub(super) linked_withdrawal_job_id: Option<String>,
    pub(super) swap_tx_hash: Option<String>,
    pub(super) status: String,
    #[serde(default)]
    pub(super) attempts: u32,
    #[serde(default)]
    pub(super) last_error: Option<String>,
    #[serde(default)]
    pub(super) next_attempt_at: Option<i64>,
    pub(super) created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WithdrawalJob {
    pub(super) job_id: String,
    pub(super) user_id: String,
    pub(super) asset: String,
    pub(super) amount: u64,
    pub(super) dest_chain: String,
    pub(super) dest_address: String,
    /// For lUSD: which stablecoin the user wants ("usdt" or "usdc")
    #[serde(default = "default_preferred_stablecoin")]
    pub(super) preferred_stablecoin: String,
    /// Lichen burn tx signature (user burned their wrapped tokens)
    pub(super) burn_tx_signature: Option<String>,
    /// Outbound chain tx hash (SOL/ETH/USDT sent to user's dest_address)
    pub(super) outbound_tx_hash: Option<String>,
    /// Pinned Gnosis Safe nonce for threshold EVM withdrawals.
    /// This binds collected signatures to one exact Safe transaction intent.
    #[serde(default)]
    pub(super) safe_nonce: Option<u64>,
    #[serde(default)]
    pub(super) signatures: Vec<SignerSignature>,
    #[serde(default)]
    pub(super) velocity_tier: WithdrawalVelocityTier,
    #[serde(default)]
    pub(super) required_signer_threshold: usize,
    #[serde(default)]
    pub(super) required_operator_confirmations: usize,
    #[serde(default)]
    pub(super) release_after: Option<i64>,
    #[serde(default)]
    pub(super) burn_confirmed_at: Option<i64>,
    #[serde(default)]
    pub(super) operator_confirmations: Vec<WithdrawalOperatorConfirmation>,
    pub(super) status: String,
    #[serde(default)]
    pub(super) attempts: u32,
    #[serde(default)]
    pub(super) last_error: Option<String>,
    #[serde(default)]
    pub(super) next_attempt_at: Option<i64>,
    pub(super) created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum SignerSignatureKind {
    #[default]
    EvmEcdsa,
    PqApproval,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(super) struct SignerSignature {
    #[serde(default)]
    pub(super) kind: SignerSignatureKind,
    pub(super) signer_pubkey: String,
    pub(super) signature: String,
    pub(super) message_hash: String,
    pub(super) received_at: i64,
}

impl SignerSignature {
    pub(super) fn pq_approval(
        signer_address: &Pubkey,
        message_hex: String,
        signature: &PqSignature,
    ) -> Result<Self, String> {
        Ok(Self {
            kind: SignerSignatureKind::PqApproval,
            signer_pubkey: signer_address.to_base58(),
            signature: serde_json::to_string(signature)
                .map_err(|e| format!("encode PQ signature: {}", e))?,
            message_hash: message_hex,
            received_at: chrono::Utc::now().timestamp(),
        })
    }

    pub(super) fn decode_pq_signature(&self) -> Result<PqSignature, String> {
        if self.kind != SignerSignatureKind::PqApproval {
            return Err("signer entry does not contain a PQ approval".to_string());
        }
        serde_json::from_str(&self.signature).map_err(|e| format!("decode PQ signature: {}", e))
    }
}
