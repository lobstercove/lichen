use super::*;

impl TxProcessor {
    /// H16 fix: Set contract ABI through consensus (instruction type 18).
    /// Instruction data: [18 | abi_json_bytes]
    /// Accounts: [contract_owner, contract_id]
    /// Only the contract owner/deployer can set the ABI.
    pub(super) fn system_set_contract_abi(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("SetContractAbi requires [owner, contract_id] accounts".to_string());
        }
        if ix.data.len() < 2 {
            return Err("SetContractAbi: missing ABI data".to_string());
        }

        let owner = ix.accounts[0];
        let contract_id = ix.accounts[1];
        let abi_bytes = &ix.data[1..];

        let abi: crate::ContractAbi =
            serde_json::from_slice(abi_bytes).map_err(|e| format!("Invalid ABI format: {}", e))?;

        if self.contract_owner_requires_governance_flow(&owner)? {
            return Err(
                "Governed contract owner must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        self.set_contract_abi_as_owner(&owner, &contract_id, abi)
    }

    pub(super) fn set_contract_abi_as_owner(
        &self,
        owner: &Pubkey,
        contract_id: &Pubkey,
        abi: ContractAbi,
    ) -> Result<(), String> {
        let mut account = self
            .b_get_account(contract_id)?
            .ok_or_else(|| "Contract not found".to_string())?;
        if !account.executable {
            return Err("Account is not a contract".to_string());
        }

        let mut contract: crate::ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to decode contract: {}", e))?;

        if contract.owner != *owner {
            return Err(format!(
                "Only the contract deployer ({}) can set the ABI",
                contract.owner.to_base58()
            ));
        }

        contract.abi = Some(abi);
        account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;
        self.b_put_account(contract_id, &account)
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct DeployRegistryData {
    pub(super) symbol: Option<String>,
    pub(super) name: Option<String>,
    pub(super) template: Option<String>,
    pub(super) metadata: Option<serde_json::Value>,
    pub(super) upgrade_authority: Option<String>,
    pub(super) make_public: Option<bool>,
    /// Explicit ABI provided by the deployer (takes priority over auto-extracted)
    pub(super) abi: Option<ContractAbi>,
    /// Token decimals (e.g. 9 for LICN, 18 for ERC-20 style)
    pub(super) decimals: Option<u8>,
}

impl DeployRegistryData {
    pub(super) fn from_init_data(init_data: &[u8]) -> Option<Self> {
        if init_data.is_empty() {
            return None;
        }
        let raw = match std::str::from_utf8(init_data) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(
                    "⚠️  DeployRegistryData::from_init_data: UTF-8 decode failed ({} bytes): {}",
                    init_data.len(),
                    e
                );
                return None;
            }
        };
        match serde_json::from_str(raw) {
            Ok(data) => Some(data),
            Err(e) => {
                tracing::debug!(
                    "⚠️  DeployRegistryData::from_init_data: JSON parse failed: {} (first 200 chars: {:?})",
                    e,
                    &raw[..raw.len().min(200)]
                );
                None
            }
        }
    }
}
