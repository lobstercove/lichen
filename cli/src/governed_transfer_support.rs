use anyhow::{anyhow, bail, Context, Result};
use lichen_core::{Instruction, Keypair, Pubkey, Transaction, SYSTEM_PROGRAM_ID};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

use crate::cli_args::GovernedTransferCommands;
use crate::client::RpcClient;
use crate::client_transport_support::encode_base64;
use crate::client_tx_support::build_signed_instruction;
use crate::keypair_manager::KeypairManager;
use crate::output_support::print_json;

const SPORES_PER_LICN: u64 = 1_000_000_000;
const IX_PROPOSE_GOVERNED_TRANSFER: u8 = 21;
const IX_APPROVE_GOVERNED_TRANSFER: u8 = 22;
const IX_EXECUTE_GOVERNED_TRANSFER: u8 = 32;
const IX_CANCEL_GOVERNED_TRANSFER: u8 = 33;
const PROPOSAL_LOOKUP_RETRIES: usize = 8;

#[derive(Debug, Clone, Deserialize)]
struct GovernedProposalView {
    id: u64,
    source: String,
    recipient: String,
    amount: u64,
    #[serde(default)]
    approvals: Vec<String>,
    threshold: u8,
    executed: bool,
    #[serde(default)]
    cancelled: bool,
    #[serde(default)]
    execute_after_epoch: Option<u64>,
    #[serde(default)]
    velocity_tier: Option<String>,
    #[serde(default)]
    daily_cap_spores: Option<u64>,
}

struct SubmissionOutcome {
    signature: Option<String>,
    preflight: Option<serde_json::Value>,
}

pub(super) async fn handle_governed_transfer_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    command: GovernedTransferCommands,
    json_output: bool,
) -> Result<()> {
    match command {
        GovernedTransferCommands::Propose {
            to,
            amount,
            source,
            keypair,
            dry_run,
            skip_preflight,
            scan_limit,
        } => {
            handle_propose(
                client,
                keypair_mgr,
                ProposeRequest {
                    to,
                    amount,
                    source,
                    keypair,
                    dry_run,
                    skip_preflight,
                    scan_limit,
                },
                json_output,
            )
            .await
        }
        GovernedTransferCommands::Approve {
            proposal_id,
            keypair,
            dry_run,
            skip_preflight,
        } => {
            handle_proposal_action(
                client,
                keypair_mgr,
                ProposalActionRequest {
                    action: ProposalAction::Approve,
                    proposal_id,
                    keypair,
                    dry_run,
                    skip_preflight,
                },
                json_output,
            )
            .await
        }
        GovernedTransferCommands::Execute {
            proposal_id,
            keypair,
            dry_run,
            skip_preflight,
        } => {
            handle_proposal_action(
                client,
                keypair_mgr,
                ProposalActionRequest {
                    action: ProposalAction::Execute,
                    proposal_id,
                    keypair,
                    dry_run,
                    skip_preflight,
                },
                json_output,
            )
            .await
        }
        GovernedTransferCommands::Cancel {
            proposal_id,
            keypair,
            dry_run,
            skip_preflight,
        } => {
            handle_proposal_action(
                client,
                keypair_mgr,
                ProposalActionRequest {
                    action: ProposalAction::Cancel,
                    proposal_id,
                    keypair,
                    dry_run,
                    skip_preflight,
                },
                json_output,
            )
            .await
        }
        GovernedTransferCommands::Info { proposal_id } => {
            let proposal = require_governed_proposal(client, proposal_id).await?;
            if json_output {
                print_json(&proposal_to_json(&proposal));
            } else {
                print_proposal(&proposal);
            }
            Ok(())
        }
    }
}

struct ProposeRequest {
    to: String,
    amount: String,
    source: String,
    keypair: Option<PathBuf>,
    dry_run: bool,
    skip_preflight: bool,
    scan_limit: u64,
}

async fn handle_propose(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    request: ProposeRequest,
    json_output: bool,
) -> Result<()> {
    let signer = load_signer(keypair_mgr, request.keypair)?;
    let signer_pubkey = signer.pubkey();
    let source = resolve_governed_source(client, &request.source).await?;
    let recipient = Pubkey::from_base58(&request.to)
        .map_err(|error| anyhow!("Invalid recipient address: {}", error))?;
    let amount_spores = parse_licn_amount_to_spores(&request.amount)?;
    if amount_spores == 0 {
        bail!("Amount must be greater than zero");
    }

    let last_known_proposal_id = if request.dry_run {
        None
    } else {
        find_last_governed_proposal_id(client, request.scan_limit).await?
    };

    let instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![signer_pubkey, source, recipient],
        data: encode_amount_instruction(IX_PROPOSE_GOVERNED_TRANSFER, amount_spores),
    };

    if !json_output {
        println!("Governed transfer proposal");
        println!("  Source:    {}", source.to_base58());
        println!("  Recipient: {}", recipient.to_base58());
        println!(
            "  Amount:    {} LICN ({} spores)",
            format_licn(amount_spores),
            amount_spores
        );
        println!("  Proposer:  {}", signer_pubkey.to_base58());
        println!("  Dry run:   {}", request.dry_run);
    }

    let outcome = submit_or_simulate(
        client,
        &signer,
        instruction,
        request.dry_run,
        request.skip_preflight,
    )
    .await?;

    let created_proposal = if outcome.signature.is_some() {
        wait_for_created_proposal(
            client,
            last_known_proposal_id.unwrap_or(0).saturating_add(1),
            request.scan_limit,
            &source.to_base58(),
            &recipient.to_base58(),
            amount_spores,
            &signer_pubkey.to_base58(),
        )
        .await?
    } else {
        None
    };

    if json_output {
        print_json(&json!({
            "action": "propose",
            "dry_run": request.dry_run,
            "source": source.to_base58(),
            "recipient": recipient.to_base58(),
            "amount_spores": amount_spores,
            "amount_licn": format_licn(amount_spores),
            "signer": signer_pubkey.to_base58(),
            "signature": outcome.signature,
            "preflight": outcome.preflight,
            "proposal": created_proposal.as_ref().map(proposal_to_json),
        }));
        return Ok(());
    }

    print_submission_outcome(&outcome);
    if let Some(proposal) = created_proposal {
        println!();
        print_proposal(&proposal);
    } else if outcome.signature.is_some() {
        println!();
        println!("Proposal was submitted, but the created proposal ID was not found yet.");
        println!("Run: lichen governed-transfer info <proposal_id>");
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum ProposalAction {
    Approve,
    Execute,
    Cancel,
}

impl ProposalAction {
    fn name(self) -> &'static str {
        match self {
            ProposalAction::Approve => "approve",
            ProposalAction::Execute => "execute",
            ProposalAction::Cancel => "cancel",
        }
    }

    fn instruction_type(self) -> u8 {
        match self {
            ProposalAction::Approve => IX_APPROVE_GOVERNED_TRANSFER,
            ProposalAction::Execute => IX_EXECUTE_GOVERNED_TRANSFER,
            ProposalAction::Cancel => IX_CANCEL_GOVERNED_TRANSFER,
        }
    }
}

struct ProposalActionRequest {
    action: ProposalAction,
    proposal_id: u64,
    keypair: Option<PathBuf>,
    dry_run: bool,
    skip_preflight: bool,
}

async fn handle_proposal_action(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    request: ProposalActionRequest,
    json_output: bool,
) -> Result<()> {
    let before = require_governed_proposal(client, request.proposal_id).await?;
    let signer = load_signer(keypair_mgr, request.keypair)?;
    let signer_pubkey = signer.pubkey();

    let instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![signer_pubkey],
        data: encode_proposal_id_instruction(request.action.instruction_type(), request.proposal_id),
    };

    if !json_output {
        println!("Governed transfer {}", request.action.name());
        println!("  Proposal: {}", request.proposal_id);
        println!("  Signer:   {}", signer_pubkey.to_base58());
        println!("  Dry run:  {}", request.dry_run);
        println!();
        print_proposal(&before);
    }

    let outcome = submit_or_simulate(
        client,
        &signer,
        instruction,
        request.dry_run,
        request.skip_preflight,
    )
    .await?;
    let after = if outcome.signature.is_some() {
        get_governed_proposal(client, request.proposal_id).await?
    } else {
        None
    };

    if json_output {
        print_json(&json!({
            "action": request.action.name(),
            "dry_run": request.dry_run,
            "proposal_id": request.proposal_id,
            "signer": signer_pubkey.to_base58(),
            "signature": outcome.signature,
            "preflight": outcome.preflight,
            "before": proposal_to_json(&before),
            "after": after.as_ref().map(proposal_to_json),
        }));
        return Ok(());
    }

    println!();
    print_submission_outcome(&outcome);
    if let Some(proposal) = after {
        println!();
        print_proposal(&proposal);
    }

    Ok(())
}

fn load_signer(keypair_mgr: &KeypairManager, keypair: Option<PathBuf>) -> Result<Keypair> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    keypair_mgr
        .load_keypair(&path)
        .with_context(|| format!("Failed to load signer keypair {}", path.display()))
}

async fn submit_or_simulate(
    client: &RpcClient,
    signer: &Keypair,
    instruction: Instruction,
    dry_run: bool,
    skip_preflight: bool,
) -> Result<SubmissionOutcome> {
    let tx = build_signed_instruction(client, signer, instruction).await?;
    let preflight = if skip_preflight {
        None
    } else {
        let result = simulate_transaction(client, &tx).await?;
        if !result
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            let error = result
                .get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown simulation failure");
            bail!("Preflight failed: {}", error);
        }
        Some(result)
    };

    if dry_run {
        return Ok(SubmissionOutcome {
            signature: None,
            preflight,
        });
    }

    let signature = client.submit_wire_transaction(tx.to_wire()).await?;
    Ok(SubmissionOutcome {
        signature: Some(signature),
        preflight,
    })
}

async fn simulate_transaction(client: &RpcClient, tx: &Transaction) -> Result<serde_json::Value> {
    client
        .call("simulateTransaction", json!([encode_base64(&tx.to_wire())]))
        .await
}

async fn resolve_governed_source(client: &RpcClient, source: &str) -> Result<Pubkey> {
    if let Ok(pubkey) = Pubkey::from_base58(source) {
        return Ok(pubkey);
    }

    let wanted = normalize_label(source);
    let result = client.call("getGenesisAccounts", json!([])).await?;
    let accounts = result
        .get("accounts")
        .and_then(|value| value.as_array())
        .context("getGenesisAccounts response missing accounts array")?;

    for account in accounts {
        let role = account
            .get("role")
            .and_then(|value| value.as_str())
            .map(normalize_label);
        let label = account
            .get("label")
            .and_then(|value| value.as_str())
            .map(normalize_label);
        if role.as_deref() == Some(wanted.as_str()) || label.as_deref() == Some(wanted.as_str())
        {
            let pubkey = account
                .get("pubkey")
                .and_then(|value| value.as_str())
                .context("genesis account matched source but had no pubkey")?;
            return Pubkey::from_base58(pubkey)
                .map_err(|error| anyhow!("Invalid source pubkey from RPC: {}", error));
        }
    }

    bail!(
        "Unknown governed source '{}'. Use a Base58 address or a role from getGenesisAccounts.",
        source
    )
}

fn normalize_label(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

async fn find_last_governed_proposal_id(
    client: &RpcClient,
    scan_limit: u64,
) -> Result<Option<u64>> {
    let mut last = None;
    for id in 1..=scan_limit {
        match get_governed_proposal(client, id).await? {
            Some(_) => last = Some(id),
            None => break,
        }
    }
    Ok(last)
}

async fn wait_for_created_proposal(
    client: &RpcClient,
    start_id: u64,
    scan_limit: u64,
    source: &str,
    recipient: &str,
    amount: u64,
    proposer: &str,
) -> Result<Option<GovernedProposalView>> {
    if scan_limit == 0 {
        return Ok(None);
    }
    let end_id = start_id.saturating_add(scan_limit.saturating_sub(1));

    for _ in 0..PROPOSAL_LOOKUP_RETRIES {
        for id in start_id..=end_id {
            let Some(proposal) = get_governed_proposal(client, id).await? else {
                break;
            };
            if proposal.source == source
                && proposal.recipient == recipient
                && proposal.amount == amount
                && proposal.approvals.iter().any(|approval| approval == proposer)
            {
                return Ok(Some(proposal));
            }
        }
        sleep(Duration::from_secs(1)).await;
    }

    Ok(None)
}

async fn require_governed_proposal(
    client: &RpcClient,
    proposal_id: u64,
) -> Result<GovernedProposalView> {
    get_governed_proposal(client, proposal_id)
        .await?
        .ok_or_else(|| anyhow!("Governed proposal {} not found", proposal_id))
}

async fn get_governed_proposal(
    client: &RpcClient,
    proposal_id: u64,
) -> Result<Option<GovernedProposalView>> {
    match client.call("getGovernedProposal", json!([proposal_id])).await {
        Ok(value) => serde_json::from_value(value)
            .map(Some)
            .context("Failed to parse getGovernedProposal response"),
        Err(error) if is_not_found_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("not found") || message.contains("-32001")
}

fn encode_amount_instruction(instruction_type: u8, amount: u64) -> Vec<u8> {
    let mut data = vec![instruction_type];
    data.extend_from_slice(&amount.to_le_bytes());
    data
}

fn encode_proposal_id_instruction(instruction_type: u8, proposal_id: u64) -> Vec<u8> {
    let mut data = vec![instruction_type];
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data
}

fn parse_licn_amount_to_spores(raw: &str) -> Result<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Amount cannot be empty");
    }
    if trimmed.starts_with('-') || trimmed.starts_with('+') || trimmed.contains('e') || trimmed.contains('E') {
        bail!("Amount must be a plain decimal LICN value");
    }

    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() > 2 {
        bail!("Amount has too many decimal points");
    }

    let whole_part = parts[0];
    let frac_part = parts.get(1).copied().unwrap_or("");
    if whole_part.is_empty() && frac_part.is_empty() {
        bail!("Amount cannot be empty");
    }
    if !whole_part.chars().all(|ch| ch.is_ascii_digit())
        || !frac_part.chars().all(|ch| ch.is_ascii_digit())
    {
        bail!("Amount must contain only digits and an optional decimal point");
    }
    if frac_part.len() > 9 {
        bail!("Amount supports at most 9 decimal places");
    }

    let whole = if whole_part.is_empty() {
        0
    } else {
        whole_part
            .parse::<u64>()
            .context("Invalid whole LICN amount")?
    };
    let mut frac_padded = frac_part.to_string();
    while frac_padded.len() < 9 {
        frac_padded.push('0');
    }
    let fractional = if frac_padded.is_empty() {
        0
    } else {
        frac_padded
            .parse::<u64>()
            .context("Invalid fractional LICN amount")?
    };

    whole
        .checked_mul(SPORES_PER_LICN)
        .and_then(|value| value.checked_add(fractional))
        .ok_or_else(|| anyhow!("Amount is too large"))
}

fn format_licn(spores: u64) -> String {
    let whole = spores / SPORES_PER_LICN;
    let fractional = spores % SPORES_PER_LICN;
    if fractional == 0 {
        return whole.to_string();
    }
    let mut frac = format!("{:09}", fractional);
    while frac.ends_with('0') {
        frac.pop();
    }
    format!("{}.{}", whole, frac)
}

fn print_submission_outcome(outcome: &SubmissionOutcome) {
    if let Some(preflight) = &outcome.preflight {
        let fee = preflight
            .get("fee")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let compute = preflight
            .get("computeUsed")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        println!("Preflight: ok, fee {} spores, compute {}", fee, compute);
    }

    if let Some(signature) = &outcome.signature {
        println!("Transaction sent: {}", signature);
    } else {
        println!("Dry run only: no transaction was broadcast.");
    }
}

fn print_proposal(proposal: &GovernedProposalView) {
    println!("Governed proposal #{}", proposal.id);
    println!("  Source:     {}", proposal.source);
    println!("  Recipient:  {}", proposal.recipient);
    println!(
        "  Amount:     {} LICN ({} spores)",
        format_licn(proposal.amount),
        proposal.amount
    );
    println!(
        "  Approvals:  {}/{}",
        proposal.approvals.len(),
        proposal.threshold
    );
    if !proposal.approvals.is_empty() {
        for approval in &proposal.approvals {
            println!("    - {}", approval);
        }
    }
    if let Some(epoch) = proposal.execute_after_epoch {
        println!("  Execute after epoch: {}", epoch);
    }
    if let Some(tier) = &proposal.velocity_tier {
        println!("  Velocity tier: {}", tier);
    }
    if let Some(cap) = proposal.daily_cap_spores {
        println!("  Daily cap: {} LICN ({} spores)", format_licn(cap), cap);
    }
    println!("  Executed:   {}", proposal.executed);
    println!("  Cancelled:  {}", proposal.cancelled);
}

fn proposal_to_json(proposal: &GovernedProposalView) -> serde_json::Value {
    json!({
        "id": proposal.id,
        "source": proposal.source,
        "recipient": proposal.recipient,
        "amount_spores": proposal.amount,
        "amount_licn": format_licn(proposal.amount),
        "approvals": proposal.approvals,
        "threshold": proposal.threshold,
        "executed": proposal.executed,
        "cancelled": proposal.cancelled,
        "execute_after_epoch": proposal.execute_after_epoch,
        "velocity_tier": proposal.velocity_tier,
        "daily_cap_spores": proposal.daily_cap_spores,
        "daily_cap_licn": proposal.daily_cap_spores.map(format_licn),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_and_fractional_licn_amounts() {
        assert_eq!(parse_licn_amount_to_spores("100000").unwrap(), 100_000_000_000_000);
        assert_eq!(parse_licn_amount_to_spores("1.25").unwrap(), 1_250_000_000);
        assert_eq!(parse_licn_amount_to_spores(".000000001").unwrap(), 1);
        assert_eq!(parse_licn_amount_to_spores("0.1").unwrap(), 100_000_000);
    }

    #[test]
    fn rejects_ambiguous_or_over_precise_amounts() {
        assert!(parse_licn_amount_to_spores("1.0000000001").is_err());
        assert!(parse_licn_amount_to_spores("1e3").is_err());
        assert!(parse_licn_amount_to_spores("-1").is_err());
        assert!(parse_licn_amount_to_spores("1,000").is_err());
    }

    #[test]
    fn formats_spores_as_licn() {
        assert_eq!(format_licn(100_000_000_000_000), "100000");
        assert_eq!(format_licn(1_250_000_000), "1.25");
        assert_eq!(format_licn(1), "0.000000001");
    }
}
