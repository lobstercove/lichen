use crate::{contract::ContractAbi, multisig::GovernedTransferVelocityTier, Pubkey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GovernanceAction {
    TreasuryTransfer {
        recipient: Pubkey,
        amount: u64,
    },
    ParamChange {
        param_id: u8,
        value: u64,
    },
    ContractUpgrade {
        contract: Pubkey,
        code: Vec<u8>,
    },
    SetContractUpgradeTimelock {
        contract: Pubkey,
        epochs: u32,
    },
    ExecuteContractUpgrade {
        contract: Pubkey,
    },
    VetoContractUpgrade {
        contract: Pubkey,
    },
    ContractClose {
        contract: Pubkey,
        destination: Pubkey,
    },
    ContractCall {
        contract: Pubkey,
        function: String,
        args: Vec<u8>,
        value: u64,
    },
    RegisterContractSymbol {
        contract: Pubkey,
        symbol: String,
        name: Option<String>,
        template: Option<String>,
        metadata: Option<Value>,
        decimals: Option<u8>,
    },
    SetContractAbi {
        contract: Pubkey,
        abi: ContractAbi,
    },
}

impl GovernanceAction {
    pub fn label(&self) -> &'static str {
        match self {
            GovernanceAction::TreasuryTransfer { .. } => "treasury_transfer",
            GovernanceAction::ParamChange { .. } => "governance_param_change",
            GovernanceAction::ContractUpgrade { .. } => "contract_upgrade",
            GovernanceAction::SetContractUpgradeTimelock { .. } => "set_contract_upgrade_timelock",
            GovernanceAction::ExecuteContractUpgrade { .. } => "execute_contract_upgrade",
            GovernanceAction::VetoContractUpgrade { .. } => "veto_contract_upgrade",
            GovernanceAction::ContractClose { .. } => "contract_close",
            GovernanceAction::ContractCall { .. } => "contract_call",
            GovernanceAction::RegisterContractSymbol { .. } => "register_contract_symbol",
            GovernanceAction::SetContractAbi { .. } => "set_contract_abi",
        }
    }

    pub fn event_fields(&self) -> Vec<(&'static str, String)> {
        match self {
            GovernanceAction::ContractCall {
                contract,
                function,
                args,
                value,
            } => vec![
                ("target_contract", contract.to_base58()),
                ("target_function", function.clone()),
                ("call_args_len", args.len().to_string()),
                ("call_value_spores", value.to_string()),
            ],
            _ => Vec::new(),
        }
    }

    pub fn metadata(&self) -> String {
        match self {
            GovernanceAction::TreasuryTransfer { recipient, amount } => {
                format!(
                    "recipient={} amount_spores={}",
                    recipient.to_base58(),
                    amount
                )
            }
            GovernanceAction::ParamChange { param_id, value } => {
                format!("param_id={} value={}", param_id, value)
            }
            GovernanceAction::ContractUpgrade { contract, code } => {
                format!("contract={} code_len={}", contract.to_base58(), code.len())
            }
            GovernanceAction::SetContractUpgradeTimelock { contract, epochs } => {
                format!("contract={} epochs={}", contract.to_base58(), epochs)
            }
            GovernanceAction::ExecuteContractUpgrade { contract } => {
                format!("contract={}", contract.to_base58())
            }
            GovernanceAction::VetoContractUpgrade { contract } => {
                format!("contract={}", contract.to_base58())
            }
            GovernanceAction::ContractClose {
                contract,
                destination,
            } => {
                format!(
                    "contract={} destination={}",
                    contract.to_base58(),
                    destination.to_base58()
                )
            }
            GovernanceAction::ContractCall {
                contract,
                function,
                args,
                value,
            } => {
                format!(
                    "contract={} function={} args_len={} value_spores={}",
                    contract.to_base58(),
                    function,
                    args.len(),
                    value
                )
            }
            GovernanceAction::RegisterContractSymbol {
                contract,
                symbol,
                name,
                template,
                metadata,
                decimals,
            } => {
                format!(
                    "contract={} symbol={} name={} template={} decimals={} has_metadata={}",
                    contract.to_base58(),
                    symbol,
                    name.as_deref().unwrap_or(""),
                    template.as_deref().unwrap_or(""),
                    decimals.map(|value| value.to_string()).unwrap_or_default(),
                    metadata.is_some()
                )
            }
            GovernanceAction::SetContractAbi { contract, abi } => {
                format!(
                    "contract={} abi_name={} abi_version={} functions={}",
                    contract.to_base58(),
                    abi.name,
                    abi.version,
                    abi.functions.len()
                )
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceProposal {
    pub id: u64,
    pub authority: Pubkey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_authority: Option<Pubkey>,
    pub proposer: Pubkey,
    pub action: GovernanceAction,
    pub action_label: String,
    pub metadata: String,
    pub approvals: Vec<Pubkey>,
    pub threshold: u8,
    pub execute_after_epoch: u64,
    #[serde(default)]
    pub velocity_tier: GovernedTransferVelocityTier,
    #[serde(default)]
    pub daily_cap_spores: u64,
    pub executed: bool,
    #[serde(default)]
    pub cancelled: bool,
}

impl GovernanceProposal {
    pub fn approval_authority(&self) -> Pubkey {
        self.approval_authority.unwrap_or(self.authority)
    }

    pub fn is_ready(&self, current_epoch: u64) -> bool {
        !self.executed
            && !self.cancelled
            && self.approvals.len() >= self.threshold as usize
            && current_epoch >= self.execute_after_epoch
    }
}
