use crate::client::ReadonlyContractResult;
use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 5] = [
    "COMPUTE",
    "compute",
    "ComputeMarket",
    "COMPUTEMARKET",
    "compute_market",
];

pub const COMPUTE_JOB_PENDING: u8 = 0;
pub const COMPUTE_JOB_CLAIMED: u8 = 1;
pub const COMPUTE_JOB_COMPLETED: u8 = 2;
pub const COMPUTE_JOB_DISPUTED: u8 = 3;
pub const COMPUTE_JOB_CANCELLED: u8 = 4;
pub const COMPUTE_JOB_RESOLVED: u8 = 5;
pub const COMPUTE_JOB_RELEASED: u8 = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketProviderInfo {
    pub address: Pubkey,
    pub total_capacity: u64,
    pub price_per_unit: u64,
    pub jobs_completed: u64,
    pub active: bool,
    pub registered_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketProviderCapacity {
    pub total: u64,
    pub reserved: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketJobInfo {
    pub requester: Pubkey,
    pub compute_units: u64,
    pub max_price: u64,
    pub code_hash: [u8; 32],
    pub status: u8,
    pub provider: Pubkey,
    pub result_hash: [u8; 32],
    pub created_slot: u64,
    pub completed_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketJobTiming {
    pub created_slot: u64,
    pub claim_deadline: u64,
    pub claimed_slot: u64,
    pub completion_deadline: u64,
    pub completed_slot: u64,
    pub challenge_deadline: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketPlatformStats {
    pub job_count: u64,
    pub completed_count: u64,
    pub payment_volume: u64,
    pub dispute_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketAgentControls {
    pub enabled: bool,
    pub route_paused: bool,
    pub max_daily_cap: u64,
    pub max_per_task_cap: u64,
    pub policy_count: u64,
    pub payment_count: u64,
    pub payment_volume: u64,
    pub blocked_payment_count: u64,
    pub blocked_payment_count_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketAccountingMigrationStatus {
    pub expected_job_count: u64,
    pub cursor: u64,
    pub reconstructed_escrow: u64,
    pub reconstructed_unpaid: u64,
    pub accounting_version: u64,
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketAccountingHealth {
    pub accounting_version: u64,
    pub migration_locked: bool,
    pub escrow_liability: u64,
    pub unpaid_liability: u64,
    pub platform_fees: u64,
    pub total_liability: u64,
    pub custody_balance: u64,
    pub solvent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputeMarketAgentPolicy {
    pub policy_version: u64,
    pub daily_cap: u64,
    pub per_task_cap: u64,
    pub policy_hash: [u8; 32],
    pub created_slot: u64,
    pub updated_slot: u64,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitComputeJobParams {
    pub compute_units: u64,
    pub max_price: u64,
    pub code_hash: [u8; 32],
    /// Native LICN attached to escrow. Set to zero for allowance-based token payment.
    pub payment_value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitAgentComputeJobParams {
    pub compute_units: u64,
    pub max_price: u64,
    pub code_hash: [u8; 32],
    pub action_hash: [u8; 32],
    /// Native LICN attached to escrow. Set to zero for allowance-based token payment.
    pub payment_value: u64,
}

#[derive(Debug, Clone)]
pub struct ComputeMarketClient {
    client: Client,
    program_id: Arc<Mutex<Option<Pubkey>>>,
}

fn layout(types: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut output =
        Vec::with_capacity(1 + types.len() + chunks.iter().map(Vec::len).sum::<usize>());
    output.push(0xAB);
    output.extend_from_slice(types);
    for chunk in chunks {
        output.extend_from_slice(chunk);
    }
    output
}

fn address_args(addresses: &[&Pubkey]) -> Vec<u8> {
    let types = vec![0x20; addresses.len()];
    let chunks: Vec<Vec<u8>> = addresses
        .iter()
        .map(|address| address.as_ref().to_vec())
        .collect();
    layout(&types, &chunks)
}

fn id_args(job_id: u64) -> Vec<u8> {
    layout(&[0x08], &[job_id.to_le_bytes().to_vec()])
}

fn return_bytes(result: &ReadonlyContractResult, function: &str) -> Result<Vec<u8>> {
    let code = result.return_code.unwrap_or(0);
    if code != 0 || !result.success {
        return Err(Error::RpcError(result.error.clone().unwrap_or_else(|| {
            format!("Compute Market {function} returned code {code}")
        })));
    }
    let encoded = result.return_data.as_ref().ok_or_else(|| {
        Error::ParseError(format!(
            "Compute Market {function} did not return payload data"
        ))
    })?;
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .map_err(|error| Error::ParseError(error.to_string()))
}

fn require_length<'a>(data: &'a [u8], expected: usize, function: &str) -> Result<&'a [u8]> {
    if data.len() != expected {
        return Err(Error::ParseError(format!(
            "Compute Market {function} payload must be exactly {expected} bytes"
        )));
    }
    Ok(data)
}

fn require_nonzero_hash(hash: &[u8; 32], field: &str) -> Result<()> {
    if hash.iter().all(|byte| *byte == 0) {
        return Err(Error::ConfigError(format!(
            "Compute Market {field} must not be the zero hash"
        )));
    }
    Ok(())
}

fn read_u64(data: &[u8], offset: usize, function: &str) -> Result<u64> {
    let end = offset.saturating_add(8);
    let bytes: [u8; 8] = data
        .get(offset..end)
        .ok_or_else(|| {
            Error::ParseError(format!(
                "Compute Market {function} payload was shorter than expected"
            ))
        })?
        .try_into()
        .map_err(|_| Error::ParseError(format!("Compute Market {function} payload malformed")))?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_pubkey(data: &[u8], offset: usize, function: &str) -> Result<Pubkey> {
    let bytes: [u8; 32] = data
        .get(offset..offset.saturating_add(32))
        .ok_or_else(|| {
            Error::ParseError(format!(
                "Compute Market {function} payload was shorter than expected"
            ))
        })?
        .try_into()
        .map_err(|_| Error::ParseError(format!("Compute Market {function} payload malformed")))?;
    Ok(Pubkey(bytes))
}

fn read_hash(data: &[u8], offset: usize, function: &str) -> Result<[u8; 32]> {
    data.get(offset..offset.saturating_add(32))
        .ok_or_else(|| {
            Error::ParseError(format!(
                "Compute Market {function} payload was shorter than expected"
            ))
        })?
        .try_into()
        .map_err(|_| Error::ParseError(format!("Compute Market {function} payload malformed")))
}

fn decode_provider(data: &[u8]) -> Result<ComputeMarketProviderInfo> {
    require_length(data, 65, "get_provider_info")?;
    Ok(ComputeMarketProviderInfo {
        address: read_pubkey(data, 0, "get_provider_info")?,
        total_capacity: read_u64(data, 32, "get_provider_info")?,
        price_per_unit: read_u64(data, 40, "get_provider_info")?,
        jobs_completed: read_u64(data, 48, "get_provider_info")?,
        active: data[56] == 1,
        registered_slot: read_u64(data, 57, "get_provider_info")?,
    })
}

fn decode_job(data: &[u8]) -> Result<ComputeMarketJobInfo> {
    require_length(data, 161, "get_job")?;
    Ok(ComputeMarketJobInfo {
        requester: read_pubkey(data, 0, "get_job")?,
        compute_units: read_u64(data, 32, "get_job")?,
        max_price: read_u64(data, 40, "get_job")?,
        code_hash: read_hash(data, 48, "get_job")?,
        status: data[80],
        provider: read_pubkey(data, 81, "get_job")?,
        result_hash: read_hash(data, 113, "get_job")?,
        created_slot: read_u64(data, 145, "get_job")?,
        completed_slot: read_u64(data, 153, "get_job")?,
    })
}

impl ComputeMarketClient {
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
            Error::ConfigError("ComputeMarketClient program cache lock poisoned".into())
        })? {
            return Ok(program_id);
        }
        for symbol in PROGRAM_SYMBOL_CANDIDATES {
            let Ok(entry) = self.client.get_symbol_registry(symbol).await else {
                continue;
            };
            let Some(program) = entry.get("program").and_then(|value| value.as_str()) else {
                continue;
            };
            let program_id = Pubkey::from_base58(program).map_err(Error::ParseError)?;
            *self.program_id.lock().map_err(|_| {
                Error::ConfigError("ComputeMarketClient program cache lock poisoned".into())
            })? = Some(program_id);
            return Ok(program_id);
        }
        Err(Error::ConfigError(
            "Unable to resolve the Compute Market program via getSymbolRegistry(\"COMPUTE\")"
                .into(),
        ))
    }

    async fn read(&self, function: &str, args: Vec<u8>) -> Result<ReadonlyContractResult> {
        self.client
            .call_readonly_contract(&self.get_program_id().await?, function, args, None)
            .await
    }

    async fn write(
        &self,
        caller: &Keypair,
        function: &str,
        args: Vec<u8>,
        value: u64,
    ) -> Result<String> {
        self.client
            .call_contract(caller, &self.get_program_id().await?, function, args, value)
            .await
    }

    pub async fn get_job(&self, job_id: u64) -> Result<Option<ComputeMarketJobInfo>> {
        let result = self.read("get_job", id_args(job_id)).await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        decode_job(&return_bytes(&result, "get_job")?).map(Some)
    }

    pub async fn get_job_count(&self) -> Result<u64> {
        let result = self.read("get_job_count", Vec::new()).await?;
        let data = return_bytes(&result, "get_job_count")?;
        require_length(&data, 8, "get_job_count")?;
        read_u64(&data, 0, "get_job_count")
    }

    pub async fn get_provider(
        &self,
        provider: &Pubkey,
    ) -> Result<Option<ComputeMarketProviderInfo>> {
        let result = self
            .read("get_provider_info", address_args(&[provider]))
            .await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        decode_provider(&return_bytes(&result, "get_provider_info")?).map(Some)
    }

    pub async fn get_provider_capacity(
        &self,
        provider: &Pubkey,
    ) -> Result<Option<ComputeMarketProviderCapacity>> {
        let result = self
            .read("get_provider_capacity", address_args(&[provider]))
            .await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        let data = return_bytes(&result, "get_provider_capacity")?;
        require_length(&data, 24, "get_provider_capacity")?;
        Ok(Some(ComputeMarketProviderCapacity {
            total: read_u64(&data, 0, "get_provider_capacity")?,
            reserved: read_u64(&data, 8, "get_provider_capacity")?,
            available: read_u64(&data, 16, "get_provider_capacity")?,
        }))
    }

    pub async fn get_job_timing(&self, job_id: u64) -> Result<Option<ComputeMarketJobTiming>> {
        let result = self.read("get_job_timing", id_args(job_id)).await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        let data = return_bytes(&result, "get_job_timing")?;
        require_length(&data, 48, "get_job_timing")?;
        Ok(Some(ComputeMarketJobTiming {
            created_slot: read_u64(&data, 0, "get_job_timing")?,
            claim_deadline: read_u64(&data, 8, "get_job_timing")?,
            claimed_slot: read_u64(&data, 16, "get_job_timing")?,
            completion_deadline: read_u64(&data, 24, "get_job_timing")?,
            completed_slot: read_u64(&data, 32, "get_job_timing")?,
            challenge_deadline: read_u64(&data, 40, "get_job_timing")?,
        }))
    }

    pub async fn get_platform_stats(&self) -> Result<ComputeMarketPlatformStats> {
        let result = self.read("get_platform_stats", Vec::new()).await?;
        let data = return_bytes(&result, "get_platform_stats")?;
        require_length(&data, 32, "get_platform_stats")?;
        Ok(ComputeMarketPlatformStats {
            job_count: read_u64(&data, 0, "get_platform_stats")?,
            completed_count: read_u64(&data, 8, "get_platform_stats")?,
            payment_volume: read_u64(&data, 16, "get_platform_stats")?,
            dispute_count: read_u64(&data, 24, "get_platform_stats")?,
        })
    }

    async fn get_amount(&self, function: &str, args: Vec<u8>) -> Result<u64> {
        let result = self.read(function, args).await?;
        let data = return_bytes(&result, function)?;
        require_length(&data, 8, function)?;
        read_u64(&data, 0, function)
    }

    pub async fn get_escrow(&self, job_id: u64) -> Result<u64> {
        self.get_amount("get_escrow", id_args(job_id)).await
    }

    pub async fn get_platform_fees(&self, token: &Pubkey) -> Result<u64> {
        self.get_amount("get_platform_fees", address_args(&[token]))
            .await
    }

    pub async fn get_unpaid_payout(&self, token: &Pubkey, recipient: &Pubkey) -> Result<u64> {
        self.get_amount("get_unpaid_payout", address_args(&[token, recipient]))
            .await
    }

    pub async fn get_agent_spend_window(&self, agent: &Pubkey, window: u64) -> Result<u64> {
        self.get_amount(
            "get_agent_spend_window",
            layout(
                &[0x20, 0x08],
                &[agent.as_ref().to_vec(), window.to_le_bytes().to_vec()],
            ),
        )
        .await
    }

    pub async fn get_agent_job_action(&self, job_id: u64) -> Result<Option<[u8; 32]>> {
        let result = self.read("get_agent_job_action", id_args(job_id)).await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        read_hash(
            &return_bytes(&result, "get_agent_job_action")?,
            0,
            "get_agent_job_action",
        )
        .map(Some)
    }

    pub async fn get_agent_controls(&self) -> Result<ComputeMarketAgentControls> {
        let result = self.read("get_agent_compute_controls", Vec::new()).await?;
        let data = return_bytes(&result, "get_agent_compute_controls")?;
        require_length(&data, 50, "get_agent_compute_controls")?;
        Ok(ComputeMarketAgentControls {
            enabled: data[0] == 1,
            route_paused: data[1] == 1,
            max_daily_cap: read_u64(&data, 2, "get_agent_compute_controls")?,
            max_per_task_cap: read_u64(&data, 10, "get_agent_compute_controls")?,
            policy_count: read_u64(&data, 18, "get_agent_compute_controls")?,
            payment_count: read_u64(&data, 26, "get_agent_compute_controls")?,
            payment_volume: read_u64(&data, 34, "get_agent_compute_controls")?,
            blocked_payment_count: read_u64(&data, 42, "get_agent_compute_controls")?,
            blocked_payment_count_supported: false,
        })
    }

    pub async fn get_agent_policy(
        &self,
        agent: &Pubkey,
    ) -> Result<Option<ComputeMarketAgentPolicy>> {
        let result = self
            .read("get_agent_spending_policy", address_args(&[agent]))
            .await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        let data = return_bytes(&result, "get_agent_spending_policy")?;
        require_length(&data, 73, "get_agent_spending_policy")?;
        Ok(Some(ComputeMarketAgentPolicy {
            policy_version: read_u64(&data, 0, "get_agent_spending_policy")?,
            daily_cap: read_u64(&data, 8, "get_agent_spending_policy")?,
            per_task_cap: read_u64(&data, 16, "get_agent_spending_policy")?,
            policy_hash: read_hash(&data, 24, "get_agent_spending_policy")?,
            created_slot: read_u64(&data, 56, "get_agent_spending_policy")?,
            updated_slot: read_u64(&data, 64, "get_agent_spending_policy")?,
            active: data[72] == 1,
        }))
    }

    pub async fn get_accounting_migration_status(
        &self,
    ) -> Result<ComputeMarketAccountingMigrationStatus> {
        let result = self
            .read("get_accounting_migration_status", Vec::new())
            .await?;
        let data = return_bytes(&result, "get_accounting_migration_status")?;
        require_length(&data, 48, "get_accounting_migration_status")?;
        Ok(ComputeMarketAccountingMigrationStatus {
            expected_job_count: read_u64(&data, 0, "get_accounting_migration_status")?,
            cursor: read_u64(&data, 8, "get_accounting_migration_status")?,
            reconstructed_escrow: read_u64(&data, 16, "get_accounting_migration_status")?,
            reconstructed_unpaid: read_u64(&data, 24, "get_accounting_migration_status")?,
            accounting_version: read_u64(&data, 32, "get_accounting_migration_status")?,
            locked: read_u64(&data, 40, "get_accounting_migration_status")? == 1,
        })
    }

    pub async fn get_accounting_health(&self) -> Result<ComputeMarketAccountingHealth> {
        let result = self.read("get_accounting_health", Vec::new()).await?;
        let data = return_bytes(&result, "get_accounting_health")?;
        require_length(&data, 64, "get_accounting_health")?;
        Ok(ComputeMarketAccountingHealth {
            accounting_version: read_u64(&data, 0, "get_accounting_health")?,
            migration_locked: read_u64(&data, 8, "get_accounting_health")? == 1,
            escrow_liability: read_u64(&data, 16, "get_accounting_health")?,
            unpaid_liability: read_u64(&data, 24, "get_accounting_health")?,
            platform_fees: read_u64(&data, 32, "get_accounting_health")?,
            total_liability: read_u64(&data, 40, "get_accounting_health")?,
            custody_balance: read_u64(&data, 48, "get_accounting_health")?,
            solvent: read_u64(&data, 56, "get_accounting_health")? == 1,
        })
    }

    pub async fn register_provider(
        &self,
        provider: &Keypair,
        capacity: u64,
        price_per_unit: u64,
    ) -> Result<String> {
        self.write(
            provider,
            "register_provider",
            layout(
                &[0x20, 0x08, 0x08],
                &[
                    provider.pubkey().as_ref().to_vec(),
                    capacity.to_le_bytes().to_vec(),
                    price_per_unit.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn update_provider(
        &self,
        provider: &Keypair,
        capacity: u64,
        price_per_unit: u64,
    ) -> Result<String> {
        self.write(
            provider,
            "update_provider",
            layout(
                &[0x20, 0x08, 0x08],
                &[
                    provider.pubkey().as_ref().to_vec(),
                    capacity.to_le_bytes().to_vec(),
                    price_per_unit.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn deactivate_provider(&self, provider: &Keypair) -> Result<String> {
        self.write(
            provider,
            "deactivate_provider",
            address_args(&[&provider.pubkey()]),
            0,
        )
        .await
    }

    pub async fn reactivate_provider(&self, provider: &Keypair) -> Result<String> {
        self.write(
            provider,
            "reactivate_provider",
            address_args(&[&provider.pubkey()]),
            0,
        )
        .await
    }

    pub async fn submit_job(
        &self,
        requester: &Keypair,
        params: SubmitComputeJobParams,
    ) -> Result<String> {
        require_nonzero_hash(&params.code_hash, "code_hash")?;
        self.write(
            requester,
            "submit_job",
            layout(
                &[0x20, 0x08, 0x08, 0x20],
                &[
                    requester.pubkey().as_ref().to_vec(),
                    params.compute_units.to_le_bytes().to_vec(),
                    params.max_price.to_le_bytes().to_vec(),
                    params.code_hash.to_vec(),
                ],
            ),
            params.payment_value,
        )
        .await
    }

    async fn actor_job(&self, actor: &Keypair, function: &str, job_id: u64) -> Result<String> {
        self.write(
            actor,
            function,
            layout(
                &[0x20, 0x08],
                &[
                    actor.pubkey().as_ref().to_vec(),
                    job_id.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn claim_job(&self, provider: &Keypair, job_id: u64) -> Result<String> {
        self.actor_job(provider, "claim_job", job_id).await
    }

    pub async fn dispute_job(&self, requester: &Keypair, job_id: u64) -> Result<String> {
        self.actor_job(requester, "dispute_job", job_id).await
    }

    pub async fn cancel_job(&self, requester: &Keypair, job_id: u64) -> Result<String> {
        self.actor_job(requester, "cancel_job", job_id).await
    }

    pub async fn complete_job(
        &self,
        provider: &Keypair,
        job_id: u64,
        result_hash: [u8; 32],
    ) -> Result<String> {
        require_nonzero_hash(&result_hash, "result_hash")?;
        self.write(
            provider,
            "complete_job",
            layout(
                &[0x20, 0x08, 0x20],
                &[
                    provider.pubkey().as_ref().to_vec(),
                    job_id.to_le_bytes().to_vec(),
                    result_hash.to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn release_payment(&self, caller: &Keypair, job_id: u64) -> Result<String> {
        self.write(caller, "release_payment", id_args(job_id), 0)
            .await
    }

    pub async fn resolve_dispute(
        &self,
        arbitrator: &Keypair,
        job_id: u64,
        provider_share_bps: u64,
    ) -> Result<String> {
        self.write(
            arbitrator,
            "resolve_dispute",
            layout(
                &[0x20, 0x08, 0x08],
                &[
                    arbitrator.pubkey().as_ref().to_vec(),
                    job_id.to_le_bytes().to_vec(),
                    provider_share_bps.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn claim_unpaid_payout(&self, recipient: &Keypair, token: &Pubkey) -> Result<String> {
        self.write(
            recipient,
            "claim_unpaid_payout",
            address_args(&[&recipient.pubkey(), token]),
            0,
        )
        .await
    }

    pub async fn set_agent_policy(
        &self,
        agent: &Keypair,
        daily_cap: u64,
        per_task_cap: u64,
        policy_hash: [u8; 32],
        policy_version: u64,
    ) -> Result<String> {
        require_nonzero_hash(&policy_hash, "policy_hash")?;
        self.write(
            agent,
            "set_agent_spending_policy",
            layout(
                &[0x20, 0x08, 0x08, 0x20, 0x08],
                &[
                    agent.pubkey().as_ref().to_vec(),
                    daily_cap.to_le_bytes().to_vec(),
                    per_task_cap.to_le_bytes().to_vec(),
                    policy_hash.to_vec(),
                    policy_version.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn disable_agent_policy(&self, agent: &Keypair) -> Result<String> {
        self.write(
            agent,
            "disable_agent_spending_policy",
            address_args(&[&agent.pubkey()]),
            0,
        )
        .await
    }

    pub async fn submit_agent_job(
        &self,
        agent: &Keypair,
        params: SubmitAgentComputeJobParams,
    ) -> Result<String> {
        require_nonzero_hash(&params.code_hash, "code_hash")?;
        require_nonzero_hash(&params.action_hash, "action_hash")?;
        self.write(
            agent,
            "submit_agent_job",
            layout(
                &[0x20, 0x08, 0x08, 0x20, 0x20],
                &[
                    agent.pubkey().as_ref().to_vec(),
                    params.compute_units.to_le_bytes().to_vec(),
                    params.max_price.to_le_bytes().to_vec(),
                    params.code_hash.to_vec(),
                    params.action_hash.to_vec(),
                ],
            ),
            params.payment_value,
        )
        .await
    }

    pub async fn initialize(&self, admin: &Keypair) -> Result<String> {
        self.write(admin, "initialize", address_args(&[&admin.pubkey()]), 0)
            .await
    }

    async fn admin_u64(&self, admin: &Keypair, function: &str, value: u64) -> Result<String> {
        self.write(
            admin,
            function,
            layout(
                &[0x20, 0x08],
                &[
                    admin.pubkey().as_ref().to_vec(),
                    value.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn set_claim_timeout(&self, admin: &Keypair, slots: u64) -> Result<String> {
        self.admin_u64(admin, "set_claim_timeout", slots).await
    }
    pub async fn set_complete_timeout(&self, admin: &Keypair, slots: u64) -> Result<String> {
        self.admin_u64(admin, "set_complete_timeout", slots).await
    }
    pub async fn set_challenge_period(&self, admin: &Keypair, slots: u64) -> Result<String> {
        self.admin_u64(admin, "set_challenge_period", slots).await
    }
    pub async fn set_platform_fee(&self, admin: &Keypair, fee_bps: u64) -> Result<String> {
        self.admin_u64(admin, "set_platform_fee", fee_bps).await
    }
    pub async fn set_identity_gate(&self, admin: &Keypair, min_reputation: u64) -> Result<String> {
        self.admin_u64(admin, "set_identity_gate", min_reputation)
            .await
    }

    async fn admin_address(
        &self,
        admin: &Keypair,
        function: &str,
        address: &Pubkey,
    ) -> Result<String> {
        self.write(
            admin,
            function,
            address_args(&[&admin.pubkey(), address]),
            0,
        )
        .await
    }

    pub async fn add_arbitrator(&self, admin: &Keypair, arbitrator: &Pubkey) -> Result<String> {
        self.admin_address(admin, "add_arbitrator", arbitrator)
            .await
    }
    pub async fn remove_arbitrator(&self, admin: &Keypair, arbitrator: &Pubkey) -> Result<String> {
        self.admin_address(admin, "remove_arbitrator", arbitrator)
            .await
    }
    pub async fn set_token_address(&self, admin: &Keypair, token: &Pubkey) -> Result<String> {
        self.admin_address(admin, "set_token_address", token).await
    }
    pub async fn set_fee_treasury(&self, admin: &Keypair, treasury: &Pubkey) -> Result<String> {
        self.admin_address(admin, "set_fee_treasury", treasury)
            .await
    }
    pub async fn set_lichenid_address(&self, admin: &Keypair, contract: &Pubkey) -> Result<String> {
        self.admin_address(admin, "set_lichenid_address", contract)
            .await
    }
    pub async fn set_identity_admin(&self, admin: &Keypair) -> Result<String> {
        self.write(
            admin,
            "set_identity_admin",
            address_args(&[&admin.pubkey()]),
            0,
        )
        .await
    }

    pub async fn set_agent_controls(
        &self,
        admin: &Keypair,
        enabled: bool,
        route_paused: bool,
        max_daily_cap: u64,
        max_per_task_cap: u64,
    ) -> Result<String> {
        self.write(
            admin,
            "set_agent_compute_controls",
            layout(
                &[0x20, 0x08, 0x08, 0x08, 0x08],
                &[
                    admin.pubkey().as_ref().to_vec(),
                    u64::from(enabled).to_le_bytes().to_vec(),
                    u64::from(route_paused).to_le_bytes().to_vec(),
                    max_daily_cap.to_le_bytes().to_vec(),
                    max_per_task_cap.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    async fn admin_no_args(&self, admin: &Keypair, function: &str) -> Result<String> {
        self.write(admin, function, address_args(&[&admin.pubkey()]), 0)
            .await
    }

    pub async fn pause(&self, admin: &Keypair) -> Result<String> {
        self.admin_no_args(admin, "pause").await
    }
    pub async fn unpause(&self, admin: &Keypair) -> Result<String> {
        self.admin_no_args(admin, "unpause").await
    }

    pub async fn withdraw_platform_fees(
        &self,
        admin: &Keypair,
        token: &Pubkey,
        amount: u64,
    ) -> Result<String> {
        self.write(
            admin,
            "withdraw_platform_fees",
            layout(
                &[0x20, 0x20, 0x08],
                &[
                    admin.pubkey().as_ref().to_vec(),
                    token.as_ref().to_vec(),
                    amount.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn begin_accounting_v3_migration(
        &self,
        admin: &Keypair,
        expected_job_count: u64,
    ) -> Result<String> {
        self.write(
            admin,
            "begin_accounting_v3_migration",
            layout(
                &[0x20, 0x08],
                &[
                    admin.pubkey().as_ref().to_vec(),
                    expected_job_count.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }

    pub async fn migrate_accounting_v3_job(&self, caller: &Keypair, job_id: u64) -> Result<String> {
        self.write(caller, "migrate_accounting_v3_job", id_args(job_id), 0)
            .await
    }

    pub async fn complete_accounting_v3_migration(
        &self,
        admin: &Keypair,
        expected_escrow: u64,
        expected_unpaid: u64,
        expected_platform_fees: u64,
        expected_total_liability: u64,
    ) -> Result<String> {
        self.write(
            admin,
            "complete_accounting_v3_migration",
            layout(
                &[0x20, 0x08, 0x08, 0x08, 0x08],
                &[
                    admin.pubkey().as_ref().to_vec(),
                    expected_escrow.to_le_bytes().to_vec(),
                    expected_unpaid.to_le_bytes().to_vec(),
                    expected_platform_fees.to_le_bytes().to_vec(),
                    expected_total_liability.to_le_bytes().to_vec(),
                ],
            ),
            0,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_decoder_matches_contract_layout() {
        let mut data = vec![3; 32];
        data.extend_from_slice(&100u64.to_le_bytes());
        data.extend_from_slice(&7u64.to_le_bytes());
        data.extend_from_slice(&9u64.to_le_bytes());
        data.push(1);
        data.extend_from_slice(&42u64.to_le_bytes());
        let provider = decode_provider(&data).unwrap();
        assert_eq!(provider.address, Pubkey([3; 32]));
        assert_eq!(provider.total_capacity, 100);
        assert_eq!(provider.price_per_unit, 7);
        assert_eq!(provider.jobs_completed, 9);
        assert!(provider.active);
        assert_eq!(provider.registered_slot, 42);
        data.push(0);
        assert!(decode_provider(&data).is_err());
    }

    #[test]
    fn job_decoder_matches_contract_layout() {
        let mut data = vec![1; 32];
        data.extend_from_slice(&10u64.to_le_bytes());
        data.extend_from_slice(&70u64.to_le_bytes());
        data.extend_from_slice(&[2; 32]);
        data.push(COMPUTE_JOB_COMPLETED);
        data.extend_from_slice(&[3; 32]);
        data.extend_from_slice(&[4; 32]);
        data.extend_from_slice(&5u64.to_le_bytes());
        data.extend_from_slice(&6u64.to_le_bytes());
        let job = decode_job(&data).unwrap();
        assert_eq!(job.requester, Pubkey([1; 32]));
        assert_eq!(job.compute_units, 10);
        assert_eq!(job.max_price, 70);
        assert_eq!(job.provider, Pubkey([3; 32]));
        assert_eq!(job.completed_slot, 6);
        data.push(0);
        assert!(decode_job(&data).is_err());
    }

    #[test]
    fn submit_job_encoding_matches_abi_layout() {
        let requester = Pubkey([7; 32]);
        let encoded = layout(
            &[0x20, 0x08, 0x08, 0x20],
            &[
                requester.as_ref().to_vec(),
                2u64.to_le_bytes().to_vec(),
                30u64.to_le_bytes().to_vec(),
                vec![8; 32],
            ],
        );
        assert_eq!(&encoded[..5], &[0xAB, 0x20, 0x08, 0x08, 0x20]);
        assert_eq!(&encoded[5..37], &[7; 32]);
        assert_eq!(u64::from_le_bytes(encoded[37..45].try_into().unwrap()), 2);
        assert_eq!(u64::from_le_bytes(encoded[45..53].try_into().unwrap()), 30);
        assert_eq!(&encoded[53..85], &[8; 32]);
    }

    #[test]
    fn accounting_payload_lengths_and_zero_hashes_fail_closed() {
        assert!(require_length(&[0u8; 48], 48, "migration").is_ok());
        assert!(require_length(&[0u8; 49], 48, "migration").is_err());
        assert!(require_length(&[0u8; 64], 64, "health").is_ok());
        assert!(require_nonzero_hash(&[0u8; 32], "code_hash").is_err());
        assert!(require_nonzero_hash(&[1u8; 32], "code_hash").is_ok());
    }
}
