use crate::{
    contract::ContractAbi,
    multisig::GovernedTransferVelocityTier,
    restrictions::{RestrictionLiftReason, RestrictionMode, RestrictionReason, RestrictionTarget},
    Hash, Pubkey,
};
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
    Restrict {
        target: RestrictionTarget,
        mode: RestrictionMode,
        reason: RestrictionReason,
        evidence_hash: Option<Hash>,
        evidence_uri_hash: Option<Hash>,
        expires_at_slot: Option<u64>,
    },
    LiftRestriction {
        restriction_id: u64,
        reason: RestrictionLiftReason,
    },
    ExtendRestriction {
        restriction_id: u64,
        new_expires_at_slot: Option<u64>,
        evidence_hash: Option<Hash>,
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
            GovernanceAction::Restrict { .. } => "restrict",
            GovernanceAction::LiftRestriction { .. } => "lift_restriction",
            GovernanceAction::ExtendRestriction { .. } => "extend_restriction",
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
            GovernanceAction::Restrict {
                target,
                mode,
                reason,
                evidence_hash,
                evidence_uri_hash,
                expires_at_slot,
            } => vec![
                (
                    "restriction_target_type",
                    target.target_type_label().to_string(),
                ),
                ("restriction_target", target.target_value_label()),
                ("restriction_mode", mode.as_str().to_string()),
                ("restriction_reason", reason.as_str().to_string()),
                (
                    "expires_at_slot",
                    expires_at_slot
                        .map(|slot| slot.to_string())
                        .unwrap_or_default(),
                ),
                (
                    "evidence_hash",
                    evidence_hash.map(|hash| hash.to_hex()).unwrap_or_default(),
                ),
                (
                    "evidence_uri_hash",
                    evidence_uri_hash
                        .map(|hash| hash.to_hex())
                        .unwrap_or_default(),
                ),
            ],
            GovernanceAction::LiftRestriction {
                restriction_id,
                reason,
            } => vec![
                ("restriction_id", restriction_id.to_string()),
                ("restriction_reason", reason.as_str().to_string()),
            ],
            GovernanceAction::ExtendRestriction {
                restriction_id,
                new_expires_at_slot,
                evidence_hash,
            } => vec![
                ("restriction_id", restriction_id.to_string()),
                (
                    "expires_at_slot",
                    new_expires_at_slot
                        .map(|slot| slot.to_string())
                        .unwrap_or_default(),
                ),
                (
                    "evidence_hash",
                    evidence_hash.map(|hash| hash.to_hex()).unwrap_or_default(),
                ),
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
            GovernanceAction::Restrict {
                target,
                mode,
                reason,
                evidence_hash,
                evidence_uri_hash,
                expires_at_slot,
            } => {
                format!(
                    "target_type={} target={} mode={} reason={} expires_at_slot={} evidence_hash={} evidence_uri_hash={}",
                    target.target_type_label(),
                    target.target_value_label(),
                    mode.as_str(),
                    reason.as_str(),
                    expires_at_slot
                        .map(|slot| slot.to_string())
                        .unwrap_or_default(),
                    evidence_hash.map(|hash| hash.to_hex()).unwrap_or_default(),
                    evidence_uri_hash
                        .map(|hash| hash.to_hex())
                        .unwrap_or_default()
                )
            }
            GovernanceAction::LiftRestriction {
                restriction_id,
                reason,
            } => {
                format!(
                    "restriction_id={} reason={}",
                    restriction_id,
                    reason.as_str()
                )
            }
            GovernanceAction::ExtendRestriction {
                restriction_id,
                new_expires_at_slot,
                evidence_hash,
            } => {
                format!(
                    "restriction_id={} expires_at_slot={} evidence_hash={}",
                    restriction_id,
                    new_expires_at_slot
                        .map(|slot| slot.to_string())
                        .unwrap_or_default(),
                    evidence_hash.map(|hash| hash.to_hex()).unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::restrictions::ProtocolModuleId;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn pubkey(byte: u8) -> Pubkey {
        Pubkey([byte; 32])
    }

    fn hash(byte: u8) -> Hash {
        Hash([byte; 32])
    }

    fn event_field_map(action: &GovernanceAction) -> BTreeMap<&'static str, String> {
        action.event_fields().into_iter().collect()
    }

    fn bincode_variant_index(action: &GovernanceAction) -> u32 {
        let bytes = bincode::serialize(action).expect("governance action serializes");
        u32::from_le_bytes(bytes[0..4].try_into().expect("variant index prefix"))
    }

    fn minimal_abi() -> ContractAbi {
        ContractAbi {
            version: "1.0".to_string(),
            name: "Test".to_string(),
            template: None,
            description: None,
            functions: Vec::new(),
            events: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn restriction_governance_action_labels_are_stable() {
        let restrict = GovernanceAction::Restrict {
            target: RestrictionTarget::Account(pubkey(1)),
            mode: RestrictionMode::OutgoingOnly,
            reason: RestrictionReason::TestnetDrill,
            evidence_hash: None,
            evidence_uri_hash: None,
            expires_at_slot: Some(42),
        };
        let lift = GovernanceAction::LiftRestriction {
            restriction_id: 7,
            reason: RestrictionLiftReason::TestnetDrillComplete,
        };
        let extend = GovernanceAction::ExtendRestriction {
            restriction_id: 8,
            new_expires_at_slot: Some(90),
            evidence_hash: Some(hash(9)),
        };

        assert_eq!(restrict.label(), "restrict");
        assert_eq!(lift.label(), "lift_restriction");
        assert_eq!(extend.label(), "extend_restriction");
    }

    #[test]
    fn restrict_metadata_and_events_cover_account_target() {
        let action = GovernanceAction::Restrict {
            target: RestrictionTarget::Account(pubkey(0xA1)),
            mode: RestrictionMode::OutgoingOnly,
            reason: RestrictionReason::TestnetDrill,
            evidence_hash: None,
            evidence_uri_hash: Some(hash(0xB1)),
            expires_at_slot: Some(242_000),
        };

        let target = pubkey(0xA1).to_base58();
        let evidence_uri_hash = hash(0xB1).to_hex();
        assert_eq!(
            action.metadata(),
            format!(
                "target_type=account target={} mode=outgoing_only reason=testnet_drill expires_at_slot=242000 evidence_hash= evidence_uri_hash={}",
                target,
                evidence_uri_hash
            )
        );

        let fields = event_field_map(&action);
        assert_eq!(
            fields.get("restriction_target_type"),
            Some(&"account".to_string())
        );
        assert_eq!(fields.get("restriction_target"), Some(&target));
        assert_eq!(
            fields.get("restriction_mode"),
            Some(&"outgoing_only".to_string())
        );
        assert_eq!(
            fields.get("restriction_reason"),
            Some(&"testnet_drill".to_string())
        );
        assert_eq!(fields.get("expires_at_slot"), Some(&"242000".to_string()));
        assert_eq!(fields.get("evidence_hash"), Some(&String::new()));
        assert_eq!(fields.get("evidence_uri_hash"), Some(&evidence_uri_hash));
    }

    #[test]
    fn restrict_events_cover_remaining_target_forms() {
        let account_asset = GovernanceAction::Restrict {
            target: RestrictionTarget::AccountAsset {
                account: pubkey(0x10),
                asset: pubkey(0x11),
            },
            mode: RestrictionMode::FrozenAmount { amount: 123 },
            reason: RestrictionReason::StolenFunds,
            evidence_hash: Some(hash(0x12)),
            evidence_uri_hash: None,
            expires_at_slot: None,
        };
        let account_asset_fields = event_field_map(&account_asset);
        assert_eq!(
            account_asset_fields.get("restriction_target_type"),
            Some(&"account_asset".to_string())
        );
        assert_eq!(
            account_asset_fields.get("restriction_target"),
            Some(&format!(
                "{}:{}",
                pubkey(0x10).to_base58(),
                pubkey(0x11).to_base58()
            ))
        );
        assert_eq!(
            account_asset_fields.get("restriction_mode"),
            Some(&"frozen_amount".to_string())
        );
        assert_eq!(
            account_asset_fields.get("restriction_reason"),
            Some(&"stolen_funds".to_string())
        );
        assert_eq!(
            account_asset_fields.get("evidence_hash"),
            Some(&hash(0x12).to_hex())
        );

        let asset = GovernanceAction::Restrict {
            target: RestrictionTarget::Asset(pubkey(0x13)),
            mode: RestrictionMode::AssetPaused,
            reason: RestrictionReason::CustodyIncident,
            evidence_hash: Some(hash(0x14)),
            evidence_uri_hash: None,
            expires_at_slot: Some(80),
        };
        let asset_fields = event_field_map(&asset);
        assert_eq!(
            asset_fields.get("restriction_target_type"),
            Some(&"asset".to_string())
        );
        assert_eq!(
            asset_fields.get("restriction_target"),
            Some(&pubkey(0x13).to_base58())
        );
        assert_eq!(
            asset_fields.get("restriction_mode"),
            Some(&"asset_paused".to_string())
        );

        let contract = GovernanceAction::Restrict {
            target: RestrictionTarget::Contract(pubkey(0x21)),
            mode: RestrictionMode::Quarantined,
            reason: RestrictionReason::ScamContract,
            evidence_hash: Some(hash(0x20)),
            evidence_uri_hash: None,
            expires_at_slot: Some(81),
        };
        let contract_fields = event_field_map(&contract);
        assert_eq!(
            contract_fields.get("restriction_target_type"),
            Some(&"contract".to_string())
        );
        assert_eq!(
            contract_fields.get("restriction_target"),
            Some(&pubkey(0x21).to_base58())
        );
        assert_eq!(
            contract_fields.get("restriction_mode"),
            Some(&"quarantined".to_string())
        );

        let code_hash = GovernanceAction::Restrict {
            target: RestrictionTarget::CodeHash(hash(0x22)),
            mode: RestrictionMode::DeployBlocked,
            reason: RestrictionReason::MaliciousCodeHash,
            evidence_hash: Some(hash(0x23)),
            evidence_uri_hash: Some(hash(0x24)),
            expires_at_slot: Some(90),
        };
        let code_hash_fields = event_field_map(&code_hash);
        assert_eq!(
            code_hash_fields.get("restriction_target_type"),
            Some(&"code_hash".to_string())
        );
        assert_eq!(
            code_hash_fields.get("restriction_target"),
            Some(&hash(0x22).to_hex())
        );
        assert_eq!(
            code_hash_fields.get("restriction_mode"),
            Some(&"deploy_blocked".to_string())
        );

        let bridge_route = GovernanceAction::Restrict {
            target: RestrictionTarget::BridgeRoute {
                chain_id: "neo-x-testnet".to_string(),
                asset: "WETH".to_string(),
            },
            mode: RestrictionMode::RoutePaused,
            reason: RestrictionReason::BridgeCompromise,
            evidence_hash: Some(hash(0x33)),
            evidence_uri_hash: None,
            expires_at_slot: Some(91),
        };
        let bridge_fields = event_field_map(&bridge_route);
        assert_eq!(
            bridge_fields.get("restriction_target_type"),
            Some(&"bridge_route".to_string())
        );
        assert_eq!(
            bridge_fields.get("restriction_target"),
            Some(&"neo-x-testnet:WETH".to_string())
        );
        assert_eq!(
            bridge_fields.get("restriction_mode"),
            Some(&"route_paused".to_string())
        );

        let module = GovernanceAction::Restrict {
            target: RestrictionTarget::ProtocolModule(ProtocolModuleId::Mempool),
            mode: RestrictionMode::ProtocolPaused,
            reason: RestrictionReason::ProtocolBug,
            evidence_hash: Some(hash(0x44)),
            evidence_uri_hash: None,
            expires_at_slot: Some(92),
        };
        let module_fields = event_field_map(&module);
        assert_eq!(
            module_fields.get("restriction_target_type"),
            Some(&"protocol_module".to_string())
        );
        assert_eq!(
            module_fields.get("restriction_target"),
            Some(&"mempool".to_string())
        );
        assert_eq!(
            module_fields.get("restriction_mode"),
            Some(&"protocol_paused".to_string())
        );
    }

    #[test]
    fn lift_and_extend_metadata_and_events_are_stable() {
        let lift = GovernanceAction::LiftRestriction {
            restriction_id: 17,
            reason: RestrictionLiftReason::FalsePositive,
        };
        assert_eq!(lift.metadata(), "restriction_id=17 reason=false_positive");
        let lift_fields = event_field_map(&lift);
        assert_eq!(lift_fields.get("restriction_id"), Some(&"17".to_string()));
        assert_eq!(
            lift_fields.get("restriction_reason"),
            Some(&"false_positive".to_string())
        );

        let extend = GovernanceAction::ExtendRestriction {
            restriction_id: 18,
            new_expires_at_slot: Some(300),
            evidence_hash: Some(hash(0x55)),
        };
        assert_eq!(
            extend.metadata(),
            format!(
                "restriction_id=18 expires_at_slot=300 evidence_hash={}",
                hash(0x55).to_hex()
            )
        );
        let extend_fields = event_field_map(&extend);
        assert_eq!(extend_fields.get("restriction_id"), Some(&"18".to_string()));
        assert_eq!(
            extend_fields.get("expires_at_slot"),
            Some(&"300".to_string())
        );
        assert_eq!(
            extend_fields.get("evidence_hash"),
            Some(&hash(0x55).to_hex())
        );
    }

    #[test]
    fn legacy_action_metadata_and_serialization_stay_stable() {
        let recipient = pubkey(0x01);
        let contract = pubkey(0x02);
        let destination = pubkey(0x03);

        let legacy_actions = [
            GovernanceAction::TreasuryTransfer {
                recipient,
                amount: 500,
            },
            GovernanceAction::ParamChange {
                param_id: 2,
                value: 99,
            },
            GovernanceAction::ContractUpgrade {
                contract,
                code: vec![1, 2, 3],
            },
            GovernanceAction::SetContractUpgradeTimelock {
                contract,
                epochs: 4,
            },
            GovernanceAction::ExecuteContractUpgrade { contract },
            GovernanceAction::VetoContractUpgrade { contract },
            GovernanceAction::ContractClose {
                contract,
                destination,
            },
            GovernanceAction::ContractCall {
                contract,
                function: "record_call".to_string(),
                args: vec![0xAA, 0xBB],
                value: 7,
            },
            GovernanceAction::RegisterContractSymbol {
                contract,
                symbol: "TEST".to_string(),
                name: Some("Test".to_string()),
                template: Some("custom".to_string()),
                metadata: Some(json!({"risk": "low"})),
                decimals: Some(8),
            },
            GovernanceAction::SetContractAbi {
                contract,
                abi: minimal_abi(),
            },
        ];

        for (expected_index, action) in legacy_actions.iter().enumerate() {
            assert_eq!(bincode_variant_index(action), expected_index as u32);
        }

        assert_eq!(
            legacy_actions[0].metadata(),
            format!("recipient={} amount_spores=500", recipient.to_base58())
        );
        assert_eq!(
            legacy_actions[7].metadata(),
            format!(
                "contract={} function=record_call args_len=2 value_spores=7",
                contract.to_base58()
            )
        );
        assert_eq!(
            serde_json::to_value(&legacy_actions[0]).expect("serialize treasury transfer"),
            json!({
                "TreasuryTransfer": {
                    "recipient": recipient,
                    "amount": 500
                }
            })
        );
    }

    #[test]
    fn restriction_actions_are_append_only_for_bincode() {
        let restrict = GovernanceAction::Restrict {
            target: RestrictionTarget::Account(pubkey(1)),
            mode: RestrictionMode::OutgoingOnly,
            reason: RestrictionReason::TestnetDrill,
            evidence_hash: None,
            evidence_uri_hash: None,
            expires_at_slot: None,
        };
        let lift = GovernanceAction::LiftRestriction {
            restriction_id: 1,
            reason: RestrictionLiftReason::IncidentResolved,
        };
        let extend = GovernanceAction::ExtendRestriction {
            restriction_id: 1,
            new_expires_at_slot: None,
            evidence_hash: None,
        };

        assert_eq!(bincode_variant_index(&restrict), 10);
        assert_eq!(bincode_variant_index(&lift), 11);
        assert_eq!(bincode_variant_index(&extend), 12);
    }
}
