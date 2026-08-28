use crate::client::ReadonlyContractResult;
use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 5] = [
    "BOUNTY",
    "bounty",
    "BountyBoard",
    "BOUNTYBOARD",
    "bountyboard",
];
const BOUNTY_DATA_SIZE: usize = 91;
const PLATFORM_STATS_SIZE: usize = 32;
const SUBMISSION_DATA_SIZE: usize = 72;
const BOUNTY_TERMS_SIZE: usize = 64;
const ACCOUNTING_MIGRATION_STATUS_SIZE: usize = 40;
const ACCOUNTING_HEALTH_SIZE: usize = 56;
const ADMIN_TRANSITION_SIZE: usize = 64;

/// Bounty status: open for submissions.
pub const BOUNTY_STATUS_OPEN: u8 = 0;
/// Bounty status: completed (a submission was approved).
pub const BOUNTY_STATUS_COMPLETED: u8 = 1;
/// Bounty status: cancelled (refund issued).
pub const BOUNTY_STATUS_CANCELLED: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardBountyInfo {
    pub creator: Pubkey,
    pub title_hash: [u8; 32],
    pub reward_amount: u64,
    pub deadline_slot: u64,
    pub status: u8,
    pub submission_count: u8,
    pub created_slot: u64,
    pub approved_idx: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardPlatformStats {
    pub bounty_count: u64,
    pub completed_count: u64,
    pub reward_volume: u64,
    pub cancel_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardSubmission {
    pub worker: Pubkey,
    pub proof_hash: [u8; 32],
    pub submitted_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardTerms {
    pub reward_token: Pubkey,
    pub platform_fee_bps: u64,
    pub gross_reward: u64,
    pub worker_net: u64,
    pub platform_fee: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardAccountingMigrationStatus {
    pub expected_bounty_count: u64,
    pub cursor: u64,
    pub reconstructed_escrow: u64,
    pub accounting_version: u64,
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardAccountingHealth {
    pub accounting_version: u64,
    pub migration_locked: bool,
    pub escrow_liability: u64,
    pub platform_fees: u64,
    pub total_liability: u64,
    pub custody_balance: u64,
    pub solvent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardAdminTransition {
    pub current_admin: Pubkey,
    pub pending_admin: Option<Pubkey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardStats {
    pub bounty_count: u64,
    pub completed_count: u64,
    #[serde(rename = "reward_volume", alias = "total_reward_volume")]
    pub total_reward_volume: u64,
    pub cancel_count: u64,
    pub paused: bool,
}

pub struct CreateBountyParams {
    pub title_hash: [u8; 32],
    pub reward_amount: u64,
    pub deadline_slot: u64,
    pub payment_value: Option<u64>,
}

pub struct SubmitWorkParams {
    pub bounty_id: u64,
    pub proof_hash: [u8; 32],
}

pub struct ApproveWorkParams {
    pub bounty_id: u64,
    pub submission_idx: u8,
}

#[derive(Debug, Clone)]
pub struct BountyBoardClient {
    client: Client,
    program_id: Arc<Mutex<Option<Pubkey>>>,
}

fn build_layout_args(layout: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        1 + layout.len() + chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
    );
    out.push(0xAB);
    out.extend_from_slice(layout);
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

fn encode_create_bounty_args(
    creator: &Pubkey,
    title_hash: &[u8; 32],
    reward_amount: u64,
    deadline_slot: u64,
) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20, 0x08, 0x08],
        &[
            creator.as_ref().to_vec(),
            title_hash.to_vec(),
            reward_amount.to_le_bytes().to_vec(),
            deadline_slot.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_submit_work_args(bounty_id: u64, worker: &Pubkey, proof_hash: &[u8; 32]) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x20, 0x20],
        &[
            bounty_id.to_le_bytes().to_vec(),
            worker.as_ref().to_vec(),
            proof_hash.to_vec(),
        ],
    )
}

fn encode_approve_work_args(caller: &Pubkey, bounty_id: u64, submission_idx: u8) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08, 0x01],
        &[
            caller.as_ref().to_vec(),
            bounty_id.to_le_bytes().to_vec(),
            vec![submission_idx],
        ],
    )
}

fn encode_cancel_bounty_args(caller: &Pubkey, bounty_id: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08],
        &[caller.as_ref().to_vec(), bounty_id.to_le_bytes().to_vec()],
    )
}

fn encode_bounty_id_args(bounty_id: u64) -> Vec<u8> {
    build_layout_args(&[0x08], &[bounty_id.to_le_bytes().to_vec()])
}

fn encode_submission_args(bounty_id: u64, submission_idx: u8) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x01],
        &[bounty_id.to_le_bytes().to_vec(), vec![submission_idx]],
    )
}

fn encode_update_work_args(
    bounty_id: u64,
    submission_idx: u8,
    worker: &Pubkey,
    proof_hash: &[u8; 32],
) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x01, 0x20, 0x20],
        &[
            bounty_id.to_le_bytes().to_vec(),
            vec![submission_idx],
            worker.as_ref().to_vec(),
            proof_hash.to_vec(),
        ],
    )
}

fn encode_address_args(address: &Pubkey) -> Vec<u8> {
    build_layout_args(&[0x20], &[address.as_ref().to_vec()])
}

fn encode_caller_address_args(caller: &Pubkey, address: &Pubkey) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20],
        &[caller.as_ref().to_vec(), address.as_ref().to_vec()],
    )
}

fn encode_caller_address_amount_args(caller: &Pubkey, address: &Pubkey, amount: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20, 0x08],
        &[
            caller.as_ref().to_vec(),
            address.as_ref().to_vec(),
            amount.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_caller_u64_args(caller: &Pubkey, value: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08],
        &[caller.as_ref().to_vec(), value.to_le_bytes().to_vec()],
    )
}

fn encode_migration_completion_args(
    caller: &Pubkey,
    expected_escrow: u64,
    expected_platform_fees: u64,
    expected_total_liability: u64,
) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08, 0x08, 0x08],
        &[
            caller.as_ref().to_vec(),
            expected_escrow.to_le_bytes().to_vec(),
            expected_platform_fees.to_le_bytes().to_vec(),
            expected_total_liability.to_le_bytes().to_vec(),
        ],
    )
}

fn ensure_readonly_success(
    result: &ReadonlyContractResult,
    function_name: &str,
    allowed_codes: &[i64],
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if !allowed_codes.contains(&code) {
        return Err(Error::RpcError(result.error.clone().unwrap_or_else(|| {
            format!("BountyBoard {} returned code {}", function_name, code)
        })));
    }
    if !result.success {
        return Err(Error::RpcError(result.error.clone().unwrap_or_else(|| {
            format!("BountyBoard {} failed", function_name)
        })));
    }
    Ok(())
}

fn decode_return_data(result: &ReadonlyContractResult, function_name: &str) -> Result<Vec<u8>> {
    let Some(return_data) = &result.return_data else {
        return Err(Error::ParseError(format!(
            "BountyBoard {} did not return payload data",
            function_name,
        )));
    };

    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, return_data)
        .map_err(|err| Error::ParseError(err.to_string()))
}

fn decode_u64(bytes: &[u8], start: usize, function_name: &str) -> Result<u64> {
    let end = start + 8;
    if bytes.len() < end {
        return Err(Error::ParseError(format!(
            "BountyBoard {} payload was shorter than expected",
            function_name,
        )));
    }
    let slice: [u8; 8] = bytes[start..end].try_into().map_err(|_| {
        Error::ParseError(format!(
            "BountyBoard {} payload was malformed",
            function_name
        ))
    })?;
    Ok(u64::from_le_bytes(slice))
}

fn decode_flag(bytes: &[u8], start: usize, field_name: &str) -> Result<bool> {
    match decode_u64(bytes, start, field_name)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::ParseError(format!(
            "BountyBoard {} must be encoded as 0 or 1",
            field_name
        ))),
    }
}

fn decode_bounty_info(result: &ReadonlyContractResult) -> Result<BountyBoardBountyInfo> {
    ensure_readonly_success(result, "get_bounty", &[0])?;
    let bytes = decode_return_data(result, "get_bounty")?;
    if bytes.len() != BOUNTY_DATA_SIZE {
        return Err(Error::ParseError(
            "BountyBoard get_bounty payload must be exactly 91 bytes".into(),
        ));
    }

    let mut creator_bytes = [0u8; 32];
    creator_bytes.copy_from_slice(&bytes[0..32]);

    let mut title_hash = [0u8; 32];
    title_hash.copy_from_slice(&bytes[32..64]);

    Ok(BountyBoardBountyInfo {
        creator: Pubkey(creator_bytes),
        title_hash,
        reward_amount: decode_u64(&bytes, 64, "get_bounty")?,
        deadline_slot: decode_u64(&bytes, 72, "get_bounty")?,
        status: bytes[80],
        submission_count: bytes[81],
        created_slot: decode_u64(&bytes, 82, "get_bounty")?,
        approved_idx: bytes[90],
    })
}

fn decode_platform_stats(result: &ReadonlyContractResult) -> Result<BountyBoardPlatformStats> {
    ensure_readonly_success(result, "get_platform_stats", &[0])?;
    let bytes = decode_return_data(result, "get_platform_stats")?;
    if bytes.len() != PLATFORM_STATS_SIZE {
        return Err(Error::ParseError(
            "BountyBoard get_platform_stats payload must be exactly 32 bytes".into(),
        ));
    }

    Ok(BountyBoardPlatformStats {
        bounty_count: decode_u64(&bytes, 0, "get_platform_stats")?,
        completed_count: decode_u64(&bytes, 8, "get_platform_stats")?,
        reward_volume: decode_u64(&bytes, 16, "get_platform_stats")?,
        cancel_count: decode_u64(&bytes, 24, "get_platform_stats")?,
    })
}

fn decode_submission(result: &ReadonlyContractResult) -> Result<BountyBoardSubmission> {
    ensure_readonly_success(result, "get_submission", &[0])?;
    let bytes = decode_return_data(result, "get_submission")?;
    if bytes.len() != SUBMISSION_DATA_SIZE {
        return Err(Error::ParseError(
            "BountyBoard get_submission payload must be exactly 72 bytes".into(),
        ));
    }
    let mut worker = [0u8; 32];
    worker.copy_from_slice(&bytes[0..32]);
    let mut proof_hash = [0u8; 32];
    proof_hash.copy_from_slice(&bytes[32..64]);
    Ok(BountyBoardSubmission {
        worker: Pubkey(worker),
        proof_hash,
        submitted_slot: decode_u64(&bytes, 64, "get_submission")?,
    })
}

fn decode_bounty_terms(result: &ReadonlyContractResult) -> Result<BountyBoardTerms> {
    ensure_readonly_success(result, "get_bounty_terms", &[0])?;
    let bytes = decode_return_data(result, "get_bounty_terms")?;
    if bytes.len() != BOUNTY_TERMS_SIZE {
        return Err(Error::ParseError(
            "BountyBoard get_bounty_terms payload must be exactly 64 bytes".into(),
        ));
    }
    let mut reward_token = [0u8; 32];
    reward_token.copy_from_slice(&bytes[0..32]);
    Ok(BountyBoardTerms {
        reward_token: Pubkey(reward_token),
        platform_fee_bps: decode_u64(&bytes, 32, "get_bounty_terms")?,
        gross_reward: decode_u64(&bytes, 40, "get_bounty_terms")?,
        worker_net: decode_u64(&bytes, 48, "get_bounty_terms")?,
        platform_fee: decode_u64(&bytes, 56, "get_bounty_terms")?,
    })
}

fn decode_accounting_migration_status(
    result: &ReadonlyContractResult,
) -> Result<BountyBoardAccountingMigrationStatus> {
    ensure_readonly_success(result, "get_accounting_migration_status", &[0])?;
    let bytes = decode_return_data(result, "get_accounting_migration_status")?;
    if bytes.len() != ACCOUNTING_MIGRATION_STATUS_SIZE {
        return Err(Error::ParseError(
            "BountyBoard accounting migration status must be exactly 40 bytes".into(),
        ));
    }
    Ok(BountyBoardAccountingMigrationStatus {
        expected_bounty_count: decode_u64(&bytes, 0, "get_accounting_migration_status")?,
        cursor: decode_u64(&bytes, 8, "get_accounting_migration_status")?,
        reconstructed_escrow: decode_u64(&bytes, 16, "get_accounting_migration_status")?,
        accounting_version: decode_u64(&bytes, 24, "get_accounting_migration_status")?,
        locked: decode_flag(&bytes, 32, "migration lock")?,
    })
}

fn decode_accounting_health(
    result: &ReadonlyContractResult,
) -> Result<BountyBoardAccountingHealth> {
    ensure_readonly_success(result, "get_accounting_health", &[0])?;
    let bytes = decode_return_data(result, "get_accounting_health")?;
    if bytes.len() != ACCOUNTING_HEALTH_SIZE {
        return Err(Error::ParseError(
            "BountyBoard accounting health must be exactly 56 bytes".into(),
        ));
    }
    Ok(BountyBoardAccountingHealth {
        accounting_version: decode_u64(&bytes, 0, "get_accounting_health")?,
        migration_locked: decode_flag(&bytes, 8, "migration lock")?,
        escrow_liability: decode_u64(&bytes, 16, "get_accounting_health")?,
        platform_fees: decode_u64(&bytes, 24, "get_accounting_health")?,
        total_liability: decode_u64(&bytes, 32, "get_accounting_health")?,
        custody_balance: decode_u64(&bytes, 40, "get_accounting_health")?,
        solvent: decode_flag(&bytes, 48, "solvent flag")?,
    })
}

fn decode_admin_transition(result: &ReadonlyContractResult) -> Result<BountyBoardAdminTransition> {
    ensure_readonly_success(result, "get_admin_transition", &[0])?;
    let bytes = decode_return_data(result, "get_admin_transition")?;
    if bytes.len() != ADMIN_TRANSITION_SIZE {
        return Err(Error::ParseError(
            "BountyBoard admin transition must be exactly 64 bytes".into(),
        ));
    }
    let mut current = [0u8; 32];
    current.copy_from_slice(&bytes[..32]);
    let mut pending = [0u8; 32];
    pending.copy_from_slice(&bytes[32..64]);
    Ok(BountyBoardAdminTransition {
        current_admin: Pubkey(current),
        pending_admin: pending
            .iter()
            .any(|byte| *byte != 0)
            .then_some(Pubkey(pending)),
    })
}

impl BountyBoardClient {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            program_id: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_program_id(client: Client, program_id: Pubkey) -> Self {
        Self {
            client,
            program_id: Arc::new(Mutex::new(Some(program_id))),
        }
    }

    pub async fn get_program_id(&self) -> Result<Pubkey> {
        if let Some(program_id) = *self.program_id.lock().map_err(|_| {
            Error::ConfigError("BountyBoardClient program cache lock poisoned".into())
        })? {
            return Ok(program_id);
        }

        for symbol in PROGRAM_SYMBOL_CANDIDATES {
            let entry = match self.client.get_symbol_registry(symbol).await {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let Some(program) = entry.get("program").and_then(|value| value.as_str()) else {
                continue;
            };
            let program_id = Pubkey::from_base58(program).map_err(Error::ParseError)?;
            *self.program_id.lock().map_err(|_| {
                Error::ConfigError("BountyBoardClient program cache lock poisoned".into())
            })? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the BountyBoard program via getSymbolRegistry(\"BOUNTY\")".into(),
        ))
    }

    // --- Read methods ---

    pub async fn get_bounty(&self, bounty_id: u64) -> Result<Option<BountyBoardBountyInfo>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_bounty",
                encode_bounty_id_args(bounty_id),
                None,
            )
            .await?;

        if result.return_code == Some(1) {
            return Ok(None);
        }

        decode_bounty_info(&result).map(Some)
    }

    pub async fn get_bounty_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_bounty_count_exact",
                Vec::new(),
                None,
            )
            .await?;
        ensure_readonly_success(&result, "get_bounty_count_exact", &[0])?;

        let bytes = decode_return_data(&result, "get_bounty_count_exact")?;
        if bytes.len() != 8 {
            return Err(Error::ParseError(
                "BountyBoard get_bounty_count_exact payload must be exactly 8 bytes".into(),
            ));
        }
        decode_u64(&bytes, 0, "get_bounty_count_exact")
    }

    pub async fn get_platform_stats(&self) -> Result<BountyBoardPlatformStats> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_platform_stats",
                Vec::new(),
                None,
            )
            .await?;
        decode_platform_stats(&result)
    }

    pub async fn get_submission(
        &self,
        bounty_id: u64,
        submission_idx: u8,
    ) -> Result<Option<BountyBoardSubmission>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_submission",
                encode_submission_args(bounty_id, submission_idx),
                None,
            )
            .await?;
        if result.return_code == Some(1) {
            return Ok(None);
        }
        decode_submission(&result).map(Some)
    }

    pub async fn get_bounty_terms(&self, bounty_id: u64) -> Result<Option<BountyBoardTerms>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_bounty_terms",
                encode_bounty_id_args(bounty_id),
                None,
            )
            .await?;
        if result.return_code == Some(1) {
            return Ok(None);
        }
        decode_bounty_terms(&result).map(Some)
    }

    pub async fn get_platform_fees(&self, token: &Pubkey) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_platform_fees",
                encode_address_args(token),
                None,
            )
            .await?;
        ensure_readonly_success(&result, "get_platform_fees", &[0])?;
        let bytes = decode_return_data(&result, "get_platform_fees")?;
        if bytes.len() != 8 {
            return Err(Error::ParseError(
                "BountyBoard get_platform_fees payload must be exactly 8 bytes".into(),
            ));
        }
        decode_u64(&bytes, 0, "get_platform_fees")
    }

    pub async fn get_accounting_migration_status(
        &self,
    ) -> Result<BountyBoardAccountingMigrationStatus> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_accounting_migration_status",
                Vec::new(),
                None,
            )
            .await?;
        decode_accounting_migration_status(&result)
    }

    pub async fn get_accounting_health(&self) -> Result<BountyBoardAccountingHealth> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_accounting_health",
                Vec::new(),
                None,
            )
            .await?;
        decode_accounting_health(&result)
    }

    pub async fn get_admin_transition(&self) -> Result<BountyBoardAdminTransition> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_admin_transition",
                Vec::new(),
                None,
            )
            .await?;
        decode_admin_transition(&result)
    }

    pub async fn get_stats(&self) -> Result<BountyBoardStats> {
        let value = self.client.get_bountyboard_stats().await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    // --- Write methods ---

    pub async fn create_bounty(
        &self,
        creator: &Keypair,
        params: CreateBountyParams,
    ) -> Result<String> {
        if params.title_hash.iter().all(|byte| *byte == 0) {
            return Err(Error::ConfigError(
                "BountyBoard title_hash must not be the zero hash".into(),
            ));
        }
        let program_id = self.get_program_id().await?;
        let payment_value = params.payment_value.unwrap_or(params.reward_amount);
        self.client
            .call_contract(
                creator,
                &program_id,
                "create_bounty",
                encode_create_bounty_args(
                    &creator.pubkey(),
                    &params.title_hash,
                    params.reward_amount,
                    params.deadline_slot,
                ),
                payment_value,
            )
            .await
    }

    pub async fn submit_work(&self, worker: &Keypair, params: SubmitWorkParams) -> Result<String> {
        if params.proof_hash.iter().all(|byte| *byte == 0) {
            return Err(Error::ConfigError(
                "BountyBoard proof_hash must not be the zero hash".into(),
            ));
        }
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                worker,
                &program_id,
                "submit_work",
                encode_submit_work_args(params.bounty_id, &worker.pubkey(), &params.proof_hash),
                0,
            )
            .await
    }

    pub async fn approve_work(
        &self,
        creator: &Keypair,
        params: ApproveWorkParams,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                creator,
                &program_id,
                "approve_work",
                encode_approve_work_args(
                    &creator.pubkey(),
                    params.bounty_id,
                    params.submission_idx,
                ),
                0,
            )
            .await
    }

    pub async fn cancel_bounty(&self, creator: &Keypair, bounty_id: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                creator,
                &program_id,
                "cancel_bounty",
                encode_cancel_bounty_args(&creator.pubkey(), bounty_id),
                0,
            )
            .await
    }

    pub async fn update_work(
        &self,
        worker: &Keypair,
        bounty_id: u64,
        submission_idx: u8,
        proof_hash: [u8; 32],
    ) -> Result<String> {
        if proof_hash.iter().all(|byte| *byte == 0) {
            return Err(Error::ConfigError(
                "BountyBoard proof_hash must not be the zero hash".into(),
            ));
        }
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                worker,
                &program_id,
                "update_work",
                encode_update_work_args(bounty_id, submission_idx, &worker.pubkey(), &proof_hash),
                0,
            )
            .await
    }

    pub async fn initialize(&self, admin: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "initialize",
                encode_address_args(&admin.pubkey()),
                0,
            )
            .await
    }

    pub async fn set_identity_admin(&self, admin: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "set_identity_admin",
                encode_address_args(&admin.pubkey()),
                0,
            )
            .await
    }

    pub async fn propose_admin(&self, admin: &Keypair, new_admin: &Pubkey) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "propose_admin",
                encode_caller_address_args(&admin.pubkey(), new_admin),
                0,
            )
            .await
    }

    pub async fn accept_admin(&self, pending_admin: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                pending_admin,
                &program_id,
                "accept_admin",
                encode_address_args(&pending_admin.pubkey()),
                0,
            )
            .await
    }

    pub async fn cancel_admin_proposal(&self, admin: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "cancel_admin_proposal",
                encode_address_args(&admin.pubkey()),
                0,
            )
            .await
    }

    pub async fn set_lichenid_address(&self, admin: &Keypair, lichenid: &Pubkey) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "set_lichenid_address",
                encode_caller_address_args(&admin.pubkey(), lichenid),
                0,
            )
            .await
    }

    pub async fn set_identity_gate(&self, admin: &Keypair, min_reputation: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "set_identity_gate",
                encode_caller_u64_args(&admin.pubkey(), min_reputation),
                0,
            )
            .await
    }

    pub async fn set_token_address(&self, admin: &Keypair, token: &Pubkey) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "set_token_address",
                encode_caller_address_args(&admin.pubkey(), token),
                0,
            )
            .await
    }

    pub async fn set_platform_fee(&self, admin: &Keypair, fee_bps: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "set_platform_fee",
                encode_caller_u64_args(&admin.pubkey(), fee_bps),
                0,
            )
            .await
    }

    pub async fn pause(&self, admin: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "bb_pause",
                encode_address_args(&admin.pubkey()),
                0,
            )
            .await
    }

    pub async fn unpause(&self, admin: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "bb_unpause",
                encode_address_args(&admin.pubkey()),
                0,
            )
            .await
    }

    pub async fn set_fee_treasury(&self, admin: &Keypair, treasury: &Pubkey) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "set_fee_treasury",
                encode_caller_address_args(&admin.pubkey(), treasury),
                0,
            )
            .await
    }

    pub async fn withdraw_platform_fees(
        &self,
        admin: &Keypair,
        token: &Pubkey,
        amount: u64,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "withdraw_platform_fees",
                encode_caller_address_amount_args(&admin.pubkey(), token, amount),
                0,
            )
            .await
    }

    pub async fn migrate_bounty_token(
        &self,
        admin: &Keypair,
        bounty_id: u64,
        token: &Pubkey,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = build_layout_args(
            &[0x20, 0x08, 0x20],
            &[
                admin.pubkey().as_ref().to_vec(),
                bounty_id.to_le_bytes().to_vec(),
                token.as_ref().to_vec(),
            ],
        );
        self.client
            .call_contract(admin, &program_id, "migrate_bounty_token", args, 0)
            .await
    }

    pub async fn begin_accounting_v2_migration(
        &self,
        admin: &Keypair,
        expected_bounty_count: u64,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "begin_accounting_v2_migration",
                encode_caller_u64_args(&admin.pubkey(), expected_bounty_count),
                0,
            )
            .await
    }

    pub async fn migrate_accounting_v2_bounty(
        &self,
        caller: &Keypair,
        bounty_id: u64,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                caller,
                &program_id,
                "migrate_accounting_v2_bounty",
                encode_bounty_id_args(bounty_id),
                0,
            )
            .await
    }

    pub async fn complete_accounting_v2_migration(
        &self,
        admin: &Keypair,
        expected_escrow: u64,
        expected_platform_fees: u64,
        expected_total_liability: u64,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                admin,
                &program_id,
                "complete_accounting_v2_migration",
                encode_migration_completion_args(
                    &admin.pubkey(),
                    expected_escrow,
                    expected_platform_fees,
                    expected_total_liability,
                ),
                0,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readonly_result(return_code: i64, bytes: Vec<u8>) -> ReadonlyContractResult {
        ReadonlyContractResult {
            success: true,
            return_data: Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                bytes,
            )),
            return_code: Some(return_code),
            logs: Vec::new(),
            error: None,
            compute_used: None,
        }
    }

    #[test]
    fn create_bounty_encoding_matches_named_export_layout() {
        let creator = Pubkey([7u8; 32]);
        let title_hash = [0xAAu8; 32];
        let encoded = encode_create_bounty_args(&creator, &title_hash, 1_000, 2_000);

        assert_eq!(&encoded[..5], &[0xAB, 0x20, 0x20, 0x08, 0x08]);
        assert_eq!(&encoded[5..37], &[7u8; 32]);
        assert_eq!(&encoded[37..69], &[0xAAu8; 32]);
        assert_eq!(
            u64::from_le_bytes(encoded[69..77].try_into().unwrap()),
            1_000
        );
        assert_eq!(
            u64::from_le_bytes(encoded[77..85].try_into().unwrap()),
            2_000
        );
    }

    #[test]
    fn submit_work_encoding_matches_named_export_layout() {
        let worker = Pubkey([8u8; 32]);
        let proof_hash = [0xBBu8; 32];
        let encoded = encode_submit_work_args(42, &worker, &proof_hash);

        assert_eq!(&encoded[..4], &[0xAB, 0x08, 0x20, 0x20]);
        assert_eq!(u64::from_le_bytes(encoded[4..12].try_into().unwrap()), 42);
        assert_eq!(&encoded[12..44], &[8u8; 32]);
        assert_eq!(&encoded[44..76], &[0xBBu8; 32]);
    }

    #[test]
    fn approve_work_encoding_matches_named_export_layout() {
        let caller = Pubkey([9u8; 32]);
        let encoded = encode_approve_work_args(&caller, 5, 2);

        assert_eq!(&encoded[..4], &[0xAB, 0x20, 0x08, 0x01]);
        assert_eq!(&encoded[4..36], &[9u8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[36..44].try_into().unwrap()), 5);
        assert_eq!(encoded[44], 2);
    }

    #[test]
    fn cancel_bounty_encoding_matches_named_export_layout() {
        let caller = Pubkey([10u8; 32]);
        let encoded = encode_cancel_bounty_args(&caller, 3);

        assert_eq!(&encoded[..3], &[0xAB, 0x20, 0x08]);
        assert_eq!(&encoded[3..35], &[10u8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[35..43].try_into().unwrap()), 3);
    }

    #[test]
    fn bounty_id_encoding_matches_named_export_layout() {
        let encoded = encode_bounty_id_args(7);

        assert_eq!(&encoded[..2], &[0xAB, 0x08]);
        assert_eq!(u64::from_le_bytes(encoded[2..10].try_into().unwrap()), 7);
    }

    #[test]
    fn bounty_info_decoding_matches_contract_layout() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[1u8; 32]); // creator
        payload.extend_from_slice(&[0xAAu8; 32]); // title_hash
        payload.extend_from_slice(&5_000_000_000u64.to_le_bytes()); // reward_amount
        payload.extend_from_slice(&1000u64.to_le_bytes()); // deadline_slot
        payload.push(BOUNTY_STATUS_COMPLETED); // status
        payload.push(3); // submission_count
        payload.extend_from_slice(&500u64.to_le_bytes()); // created_slot
        payload.push(1); // approved_idx

        let result = readonly_result(0, payload);
        let bounty = decode_bounty_info(&result).unwrap();

        assert_eq!(
            bounty,
            BountyBoardBountyInfo {
                creator: Pubkey([1u8; 32]),
                title_hash: [0xAAu8; 32],
                reward_amount: 5_000_000_000,
                deadline_slot: 1000,
                status: BOUNTY_STATUS_COMPLETED,
                submission_count: 3,
                created_slot: 500,
                approved_idx: 1,
            }
        );
    }

    #[test]
    fn platform_stats_decoding_matches_contract_layout() {
        let result = readonly_result(
            0,
            [
                10u64.to_le_bytes().as_slice(),
                5u64.to_le_bytes().as_slice(),
                50_000_000_000u64.to_le_bytes().as_slice(),
                2u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );

        let stats = decode_platform_stats(&result).unwrap();

        assert_eq!(
            stats,
            BountyBoardPlatformStats {
                bounty_count: 10,
                completed_count: 5,
                reward_volume: 50_000_000_000,
                cancel_count: 2,
            }
        );
    }

    #[test]
    fn accounting_decoders_require_exact_layouts_and_canonical_flags() {
        let migration = readonly_result(
            0,
            [
                2u64.to_le_bytes().as_slice(),
                1u64.to_le_bytes().as_slice(),
                100u64.to_le_bytes().as_slice(),
                0u64.to_le_bytes().as_slice(),
                1u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );
        let health = readonly_result(
            0,
            [
                2u64.to_le_bytes().as_slice(),
                0u64.to_le_bytes().as_slice(),
                100u64.to_le_bytes().as_slice(),
                10u64.to_le_bytes().as_slice(),
                110u64.to_le_bytes().as_slice(),
                110u64.to_le_bytes().as_slice(),
                1u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );

        let migration = decode_accounting_migration_status(&migration).unwrap();
        let health = decode_accounting_health(&health).unwrap();
        assert_eq!(migration.cursor, 1);
        assert!(migration.locked);
        assert_eq!(health.total_liability, 110);
        assert!(health.solvent);

        let malformed_flag = readonly_result(
            0,
            [
                2u64.to_le_bytes().as_slice(),
                1u64.to_le_bytes().as_slice(),
                100u64.to_le_bytes().as_slice(),
                0u64.to_le_bytes().as_slice(),
                2u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );
        assert!(decode_accounting_migration_status(&malformed_flag).is_err());

        let mut trailing = vec![0u8; ACCOUNTING_HEALTH_SIZE];
        trailing.push(0);
        assert!(decode_accounting_health(&readonly_result(0, trailing)).is_err());
    }

    #[test]
    fn admin_transition_decoder_is_exact_and_supports_no_pending_admin() {
        let mut payload = Vec::with_capacity(ADMIN_TRANSITION_SIZE);
        payload.extend_from_slice(&[3u8; 32]);
        payload.extend_from_slice(&[4u8; 32]);
        let transition = decode_admin_transition(&readonly_result(0, payload)).unwrap();
        assert_eq!(transition.current_admin, Pubkey([3u8; 32]));
        assert_eq!(transition.pending_admin, Some(Pubkey([4u8; 32])));

        let mut no_pending = Vec::with_capacity(ADMIN_TRANSITION_SIZE);
        no_pending.extend_from_slice(&[3u8; 32]);
        no_pending.extend_from_slice(&[0u8; 32]);
        assert_eq!(
            decode_admin_transition(&readonly_result(0, no_pending))
                .unwrap()
                .pending_admin,
            None
        );
        assert!(decode_admin_transition(&readonly_result(0, vec![0u8; 65])).is_err());
    }

    #[test]
    fn admin_rotation_encoding_matches_named_export_layout() {
        let current = Pubkey([3u8; 32]);
        let pending = Pubkey([4u8; 32]);
        let propose = encode_caller_address_args(&current, &pending);
        let accept = encode_address_args(&pending);
        let cancel = encode_address_args(&current);
        assert_eq!(&propose[..3], &[0xAB, 0x20, 0x20]);
        assert_eq!(&propose[3..35], &current.0);
        assert_eq!(&propose[35..67], &pending.0);
        assert_eq!(&accept[..2], &[0xAB, 0x20]);
        assert_eq!(&accept[2..34], &pending.0);
        assert_eq!(&cancel[..2], &[0xAB, 0x20]);
        assert_eq!(&cancel[2..34], &current.0);
    }

    #[test]
    fn accounting_migration_encoding_matches_named_export_layout() {
        let caller = Pubkey([11u8; 32]);
        let begin = encode_caller_u64_args(&caller, 2);
        let migrate = encode_bounty_id_args(1);
        let complete = encode_migration_completion_args(&caller, 100, 10, 110);

        assert_eq!(&begin[..3], &[0xAB, 0x20, 0x08]);
        assert_eq!(&migrate[..2], &[0xAB, 0x08]);
        assert_eq!(&complete[..5], &[0xAB, 0x20, 0x08, 0x08, 0x08]);
    }

    #[test]
    fn rpc_stats_accept_reward_volume_field() {
        let stats: BountyBoardStats = serde_json::from_value(serde_json::json!({
            "bounty_count": 2,
            "completed_count": 1,
            "reward_volume": 90,
            "cancel_count": 0,
            "paused": false
        }))
        .unwrap();
        assert_eq!(stats.total_reward_volume, 90);
    }

    #[test]
    fn not_found_bounty_returns_none_via_return_code_1() {
        let result = ReadonlyContractResult {
            success: true,
            return_data: None,
            return_code: Some(1),
            logs: Vec::new(),
            error: None,
            compute_used: None,
        };
        // Simulate the client logic: only code 1 means not found.
        assert_eq!(result.return_code, Some(1));

        let malformed = ReadonlyContractResult {
            success: false,
            return_data: None,
            return_code: Some(2),
            logs: Vec::new(),
            error: None,
            compute_used: None,
        };
        assert!(decode_bounty_info(&malformed).is_err());
        assert!(decode_submission(&malformed).is_err());
        assert!(decode_bounty_terms(&malformed).is_err());
    }
}
