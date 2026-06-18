use crate::client::{ContractInfo, ContractLog, ContractSummary};

pub(super) fn print_contract_info(info: &ContractInfo) {
    println!("🦞 Contract Information");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📍 Address: {}", info.address);
    println!("👤 Owner:    {}", info.owner);
    println!("🚦 Lifecycle: {}", info.lifecycle_status);
    if let Some(restriction_id) = info.lifecycle_restriction_id {
        println!("🔒 Restriction: #{}", restriction_id);
    }
    println!("🧭 Lifecycle updated slot: {}", info.lifecycle_updated_slot);
    if info.lifecycle_effective_at_slot != 0 {
        println!("⏱️  Lifecycle effective slot: {}", info.lifecycle_effective_at_slot);
    }
    println!("📏 Code size: {} bytes", info.code_size);
    println!("📅 Deployed at slot: {}", info.deployed_at);
}

pub(super) fn print_contract_info_error(error: &anyhow::Error) {
    println!("🦞 Contract Information");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("⚠️  Contract not found: {}", error);
}

pub(super) fn print_contract_logs(address: &str, limit: usize, logs: &[ContractLog]) {
    println!("🦞 Contract Logs");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📍 Contract: {}", address);
    println!("📊 Showing last {} logs", limit);
    println!();

    if logs.is_empty() {
        println!("No logs found");
    } else {
        for (index, log) in logs.iter().enumerate() {
            let display = log.message.clone().unwrap_or_else(|| {
                let name = log.name.as_deref().unwrap_or("event");
                match log.data.as_ref() {
                    Some(data) if !data.is_null() => format!("{} {}", name, data),
                    _ => name.to_string(),
                }
            });
            if let Some(program) = log.program.as_deref() {
                println!(
                    "#{} [Slot {}] {} — {}",
                    index + 1,
                    log.slot,
                    program,
                    display
                );
            } else {
                println!("#{} [Slot {}] {}", index + 1, log.slot, display);
            }
        }
        println!();
        println!("Total: {} log entries", logs.len());
    }
}

pub(super) fn print_contract_logs_error(address: &str, limit: usize, error: &anyhow::Error) {
    println!("🦞 Contract Logs");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📍 Contract: {}", address);
    println!("📊 Showing last {} logs", limit);
    println!();
    println!("⚠️  Could not fetch contract logs: {}", error);
}

pub(super) fn print_contract_list(contracts: &[ContractSummary]) {
    println!("🦞 Deployed Contracts");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    if contracts.is_empty() {
        println!("No contracts deployed");
    } else {
        for (index, contract) in contracts.iter().enumerate() {
            println!("#{} {}", index + 1, contract.program_id);
            if let Some(symbol) = contract.symbol.as_deref() {
                println!("   Symbol: {}", symbol);
            }
            if let Some(name) = contract.name.as_deref() {
                println!("   Name: {}", name);
            }
            if let Some(owner) = contract.owner.as_deref() {
                println!("   Owner: {}", owner);
            }
            if let Some(template) = contract.template.as_deref() {
                println!("   Template: {}", template);
            }
            println!("   Code size: {} bytes", contract.code_size);
            if !contract.lifecycle_status.is_empty() {
                println!("   Lifecycle: {}", contract.lifecycle_status);
            }
            println!();
        }
        println!("Total: {} contracts", contracts.len());
    }
}

pub(super) fn print_contract_list_error(error: &anyhow::Error) {
    println!("🦞 Deployed Contracts");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("⚠️  Could not fetch contracts: {}", error);
}
