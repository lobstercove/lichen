use anyhow::{Context, Result};
use serde_json::{json, Map, Number, Value};

use crate::cli_args::{
    RestrictionBuildCommands, RestrictionBuilderBaseArgs, RestrictionCommands,
    RestrictionLiftCommonArgs, RestrictionRestrictCommonArgs, RestrictionStatusCommands,
};
use crate::client::RpcClient;
use crate::output_support::print_json;
use crate::restriction_output_support::{render_restriction_output, RestrictionOutputKind};

pub(super) async fn handle_restriction_command(
    client: &RpcClient,
    command: RestrictionCommands,
    json_output: bool,
) -> Result<()> {
    let request = restriction_rpc_request(command)?;
    let result = client
        .restriction_rpc(request.method, request.params)
        .await
        .with_context(|| format!("restriction RPC {} failed", request.method))?;

    if json_output {
        print_json(&result);
    } else {
        print!("{}", render_restriction_output(&request.output, &result));
    }

    Ok(())
}

struct RestrictionRpcRequest {
    method: &'static str,
    params: Value,
    output: RestrictionOutputKind,
}

fn restriction_rpc_request(command: RestrictionCommands) -> Result<RestrictionRpcRequest> {
    match command {
        RestrictionCommands::Get { id } => Ok(RestrictionRpcRequest {
            method: "getRestriction",
            params: json!([id]),
            output: RestrictionOutputKind::Get,
        }),
        RestrictionCommands::List {
            active,
            limit,
            after_id,
        } => Ok(RestrictionRpcRequest {
            method: if active {
                "listActiveRestrictions"
            } else {
                "listRestrictions"
            },
            params: json!([page_params(limit, after_id)]),
            output: RestrictionOutputKind::List {
                active_only: active,
            },
        }),
        RestrictionCommands::Status(command) => status_rpc_request(command),
        RestrictionCommands::CanSend {
            account,
            asset,
            amount,
        } => Ok(RestrictionRpcRequest {
            method: "canSend",
            params: json!([{ "account": account, "asset": asset, "amount": amount }]),
            output: RestrictionOutputKind::Movement {
                label: "can-send".to_string(),
            },
        }),
        RestrictionCommands::CanReceive {
            account,
            asset,
            amount,
        } => Ok(RestrictionRpcRequest {
            method: "canReceive",
            params: json!([{ "account": account, "asset": asset, "amount": amount }]),
            output: RestrictionOutputKind::Movement {
                label: "can-receive".to_string(),
            },
        }),
        RestrictionCommands::CanTransfer {
            from,
            to,
            asset,
            amount,
        } => Ok(RestrictionRpcRequest {
            method: "canTransfer",
            params: json!([{ "from": from, "to": to, "asset": asset, "amount": amount }]),
            output: RestrictionOutputKind::Movement {
                label: "can-transfer".to_string(),
            },
        }),
        RestrictionCommands::Build(command) => builder_rpc_request(command),
    }
}

fn status_rpc_request(command: RestrictionStatusCommands) -> Result<RestrictionRpcRequest> {
    match command {
        RestrictionStatusCommands::Account { account } => Ok(RestrictionRpcRequest {
            method: "getAccountRestrictionStatus",
            params: json!([account]),
            output: RestrictionOutputKind::Status {
                label: "account".to_string(),
            },
        }),
        RestrictionStatusCommands::AccountAsset { account, asset } => Ok(RestrictionRpcRequest {
            method: "getAccountAssetRestrictionStatus",
            params: json!([account, asset]),
            output: RestrictionOutputKind::Status {
                label: "account-asset".to_string(),
            },
        }),
        RestrictionStatusCommands::Asset { asset } => Ok(RestrictionRpcRequest {
            method: "getAssetRestrictionStatus",
            params: json!([asset]),
            output: RestrictionOutputKind::Status {
                label: "asset".to_string(),
            },
        }),
        RestrictionStatusCommands::Contract { contract } => Ok(RestrictionRpcRequest {
            method: "getContractLifecycleStatus",
            params: json!([contract]),
            output: RestrictionOutputKind::Status {
                label: "contract".to_string(),
            },
        }),
        RestrictionStatusCommands::CodeHash { code_hash } => Ok(RestrictionRpcRequest {
            method: "getCodeHashRestrictionStatus",
            params: json!([code_hash]),
            output: RestrictionOutputKind::Status {
                label: "code-hash".to_string(),
            },
        }),
        RestrictionStatusCommands::BridgeRoute { chain, asset } => Ok(RestrictionRpcRequest {
            method: "getBridgeRouteRestrictionStatus",
            params: json!([chain, asset]),
            output: RestrictionOutputKind::Status {
                label: "bridge-route".to_string(),
            },
        }),
        RestrictionStatusCommands::ProtocolModule { module } => Ok(RestrictionRpcRequest {
            method: "getRestrictionStatus",
            params: json!([{ "type": "protocol_module", "module": string_or_u64(&module) }]),
            output: RestrictionOutputKind::Status {
                label: "protocol-module".to_string(),
            },
        }),
        RestrictionStatusCommands::Target { target_json } => {
            let target: Value = serde_json::from_str(&target_json)
                .context("invalid restriction target JSON object")?;
            Ok(RestrictionRpcRequest {
                method: "getRestrictionStatus",
                params: json!([target]),
                output: RestrictionOutputKind::Status {
                    label: "target".to_string(),
                },
            })
        }
    }
}

fn builder_rpc_request(command: RestrictionBuildCommands) -> Result<RestrictionRpcRequest> {
    let (method, payload) = match command {
        RestrictionBuildCommands::RestrictAccount {
            account,
            mode,
            common,
        } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "account", account);
            insert_optional_label(&mut payload, "mode", mode.as_deref());
            ("buildRestrictAccountTx", payload)
        }
        RestrictionBuildCommands::UnrestrictAccount { account, common } => {
            let mut payload = lift_common_payload(&common);
            insert_string(&mut payload, "account", account);
            ("buildUnrestrictAccountTx", payload)
        }
        RestrictionBuildCommands::RestrictAccountAsset {
            account,
            asset,
            mode,
            amount,
            common,
        } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "account", account);
            insert_string(&mut payload, "asset", asset);
            insert_optional_label(&mut payload, "mode", mode.as_deref());
            insert_optional_u64(&mut payload, "amount", amount);
            ("buildRestrictAccountAssetTx", payload)
        }
        RestrictionBuildCommands::UnrestrictAccountAsset {
            account,
            asset,
            common,
        } => {
            let mut payload = lift_common_payload(&common);
            insert_string(&mut payload, "account", account);
            insert_string(&mut payload, "asset", asset);
            ("buildUnrestrictAccountAssetTx", payload)
        }
        RestrictionBuildCommands::SetFrozenAssetAmount {
            account,
            asset,
            amount,
            common,
        } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "account", account);
            insert_string(&mut payload, "asset", asset);
            insert_u64(&mut payload, "amount", amount);
            ("buildSetFrozenAssetAmountTx", payload)
        }
        RestrictionBuildCommands::SuspendContract { contract, common } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "contract", contract);
            ("buildSuspendContractTx", payload)
        }
        RestrictionBuildCommands::ResumeContract { contract, common } => {
            let mut payload = lift_common_payload(&common);
            insert_string(&mut payload, "contract", contract);
            ("buildResumeContractTx", payload)
        }
        RestrictionBuildCommands::QuarantineContract { contract, common } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "contract", contract);
            ("buildQuarantineContractTx", payload)
        }
        RestrictionBuildCommands::TerminateContract { contract, common } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "contract", contract);
            ("buildTerminateContractTx", payload)
        }
        RestrictionBuildCommands::BanCodeHash { code_hash, common } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "code_hash", code_hash);
            ("buildBanCodeHashTx", payload)
        }
        RestrictionBuildCommands::UnbanCodeHash { code_hash, common } => {
            let mut payload = lift_common_payload(&common);
            insert_string(&mut payload, "code_hash", code_hash);
            ("buildUnbanCodeHashTx", payload)
        }
        RestrictionBuildCommands::PauseBridgeRoute {
            chain,
            asset,
            common,
        } => {
            let mut payload = restrict_common_payload(&common);
            insert_string(&mut payload, "chain", chain);
            insert_string(&mut payload, "asset", asset);
            ("buildPauseBridgeRouteTx", payload)
        }
        RestrictionBuildCommands::ResumeBridgeRoute {
            chain,
            asset,
            common,
        } => {
            let mut payload = lift_common_payload(&common);
            insert_string(&mut payload, "chain", chain);
            insert_string(&mut payload, "asset", asset);
            ("buildResumeBridgeRouteTx", payload)
        }
        RestrictionBuildCommands::ExtendRestriction {
            restriction_id,
            new_expires_at_slot,
            evidence_hash,
            base,
        } => {
            let mut payload = builder_base_payload(&base);
            insert_u64(&mut payload, "restriction_id", restriction_id);
            insert_optional_u64(&mut payload, "new_expires_at_slot", new_expires_at_slot);
            insert_optional_string(&mut payload, "evidence_hash", evidence_hash);
            ("buildExtendRestrictionTx", payload)
        }
        RestrictionBuildCommands::LiftRestriction {
            restriction_id,
            lift_reason,
            base,
        } => {
            let mut payload = builder_base_payload(&base);
            insert_u64(&mut payload, "restriction_id", restriction_id);
            insert_label(&mut payload, "lift_reason", &lift_reason);
            ("buildLiftRestrictionTx", payload)
        }
    };

    Ok(RestrictionRpcRequest {
        method,
        params: Value::Array(vec![Value::Object(payload)]),
        output: RestrictionOutputKind::Builder,
    })
}

fn page_params(limit: u64, after_id: Option<u64>) -> Value {
    let mut object = Map::new();
    insert_u64(&mut object, "limit", limit);
    insert_optional_u64(&mut object, "after_id", after_id);
    Value::Object(object)
}

fn builder_base_payload(base: &RestrictionBuilderBaseArgs) -> Map<String, Value> {
    let mut payload = Map::new();
    insert_string(&mut payload, "proposer", base.proposer.clone());
    insert_string(
        &mut payload,
        "governance_authority",
        base.governance_authority.clone(),
    );
    insert_optional_string(
        &mut payload,
        "recent_blockhash",
        base.recent_blockhash.clone(),
    );
    payload
}

fn restrict_common_payload(common: &RestrictionRestrictCommonArgs) -> Map<String, Value> {
    let mut payload = builder_base_payload(&common.base);
    insert_label(&mut payload, "reason", &common.reason);
    insert_optional_string(&mut payload, "evidence_hash", common.evidence_hash.clone());
    insert_optional_string(
        &mut payload,
        "evidence_uri_hash",
        common.evidence_uri_hash.clone(),
    );
    insert_optional_u64(&mut payload, "expires_at_slot", common.expires_at_slot);
    payload
}

fn lift_common_payload(common: &RestrictionLiftCommonArgs) -> Map<String, Value> {
    let mut payload = builder_base_payload(&common.base);
    insert_label(&mut payload, "lift_reason", &common.lift_reason);
    insert_optional_u64(&mut payload, "restriction_id", common.restriction_id);
    payload
}

fn insert_string(object: &mut Map<String, Value>, key: &str, value: String) {
    object.insert(key.to_string(), Value::String(value));
}

fn insert_optional_string(object: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        insert_string(object, key, value);
    }
}

fn insert_u64(object: &mut Map<String, Value>, key: &str, value: u64) {
    object.insert(key.to_string(), Value::Number(Number::from(value)));
}

fn insert_optional_u64(object: &mut Map<String, Value>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        insert_u64(object, key, value);
    }
}

fn insert_label(object: &mut Map<String, Value>, key: &str, value: &str) {
    object.insert(key.to_string(), string_or_u64(value));
}

fn insert_optional_label(object: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        insert_label(object, key, value);
    }
}

fn string_or_u64(value: &str) -> Value {
    value
        .parse::<u64>()
        .map(|parsed| Value::Number(Number::from(parsed)))
        .unwrap_or_else(|_| Value::String(normalize_label(value)))
}

fn normalize_label(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Commands, OutputFormat};

    #[test]
    fn parses_restriction_json_output_command() {
        let cli = Cli::try_parse_from([
            "lichen",
            "--output",
            "json",
            "restriction",
            "can-send",
            "account111",
            "--asset",
            "native",
            "--amount",
            "42",
        ])
        .unwrap();

        assert_eq!(cli.output, OutputFormat::Json);
        match cli.command {
            Commands::Restriction(RestrictionCommands::CanSend {
                account,
                asset,
                amount,
            }) => {
                assert_eq!(account, "account111");
                assert_eq!(asset, "native");
                assert_eq!(amount, 42);
            }
            _ => panic!("expected restriction can-send command"),
        }
    }

    #[test]
    fn builds_restrict_account_payload_without_null_optionals() {
        let cli = Cli::try_parse_from([
            "lichen",
            "restriction",
            "build",
            "restrict-account",
            "account111",
            "--mode",
            "outgoing-only",
            "--proposer",
            "proposer111",
            "--governance-authority",
            "authority111",
            "--reason",
            "testnet-drill",
        ])
        .unwrap();

        let request = match cli.command {
            Commands::Restriction(command) => restriction_rpc_request(command).unwrap(),
            _ => panic!("expected restriction command"),
        };

        assert_eq!(request.method, "buildRestrictAccountTx");
        let payload = request.params[0].as_object().unwrap();
        assert_eq!(payload["account"], "account111");
        assert_eq!(payload["mode"], "outgoing_only");
        assert_eq!(payload["reason"], "testnet_drill");
        assert!(!payload.contains_key("recent_blockhash"));
        assert!(!payload.contains_key("evidence_hash"));
    }

    #[test]
    fn builds_protocol_module_status_payload_with_numeric_module() {
        let cli = Cli::try_parse_from([
            "lichen",
            "restriction",
            "status",
            "protocol-module",
            "10",
        ])
        .unwrap();

        let request = match cli.command {
            Commands::Restriction(command) => restriction_rpc_request(command).unwrap(),
            _ => panic!("expected restriction command"),
        };

        assert_eq!(request.method, "getRestrictionStatus");
        assert_eq!(
            request.params,
            serde_json::json!([{ "type": "protocol_module", "module": 10 }])
        );
    }

    #[test]
    fn builds_lift_payload_with_required_reason() {
        let cli = Cli::try_parse_from([
            "lichen",
            "restriction",
            "build",
            "lift-restriction",
            "77",
            "--lift-reason",
            "testnet-drill-complete",
            "--proposer",
            "proposer111",
            "--governance-authority",
            "authority111",
            "--recent-blockhash",
            "abcd",
        ])
        .unwrap();

        let request = match cli.command {
            Commands::Restriction(command) => restriction_rpc_request(command).unwrap(),
            _ => panic!("expected restriction command"),
        };
        let payload = request.params[0].as_object().unwrap();

        assert_eq!(request.method, "buildLiftRestrictionTx");
        assert_eq!(payload["restriction_id"], 77);
        assert_eq!(payload["lift_reason"], "testnet_drill_complete");
        assert_eq!(payload["recent_blockhash"], "abcd");
    }

    #[test]
    fn builds_account_asset_frozen_amount_payload() {
        let cli = Cli::try_parse_from([
            "lichen",
            "restriction",
            "build",
            "restrict-account-asset",
            "account111",
            "native",
            "--mode",
            "frozen-amount",
            "--amount",
            "500",
            "--proposer",
            "proposer111",
            "--governance-authority",
            "authority111",
            "--reason",
            "testnet-drill",
        ])
        .unwrap();

        let request = match cli.command {
            Commands::Restriction(command) => restriction_rpc_request(command).unwrap(),
            _ => panic!("expected restriction command"),
        };
        let payload = request.params[0].as_object().unwrap();

        assert_eq!(request.method, "buildRestrictAccountAssetTx");
        assert_eq!(payload["asset"], "native");
        assert_eq!(payload["mode"], "frozen_amount");
        assert_eq!(payload["amount"], 500);
    }
}
