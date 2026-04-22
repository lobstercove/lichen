use anyhow::Result;
use lichen_core::ContractAbi;
use std::path::PathBuf;

use crate::cli_args::CodegenLang;
use crate::client::RpcClient;
use crate::codegen_support::{generate_python, generate_typescript};

pub(super) async fn handle_generate_contract_client(
    client: &RpcClient,
    abi: Option<PathBuf>,
    address: Option<String>,
    lang: CodegenLang,
    output: PathBuf,
) -> Result<()> {
    let contract_abi: ContractAbi = if let Some(ref path) = abi {
        let content = std::fs::read_to_string(path)
            .map_err(|error| anyhow::anyhow!("Cannot read ABI file: {}", error))?;
        serde_json::from_str(&content)
            .map_err(|error| anyhow::anyhow!("Invalid ABI JSON: {}", error))?
    } else if let Some(ref contract_address) = address {
        let abi_json = client
            .get_contract_abi(contract_address)
            .await
            .map_err(|error| anyhow::anyhow!("Cannot fetch ABI: {}", error))?;
        serde_json::from_value(abi_json)
            .map_err(|error| anyhow::anyhow!("Invalid ABI: {}", error))?
    } else {
        anyhow::bail!("Either --abi or --address must be specified");
    };

    let code = match lang {
        CodegenLang::Typescript => generate_typescript(&contract_abi),
        CodegenLang::Python => generate_python(&contract_abi),
    };

    std::fs::write(&output, &code)
        .map_err(|error| anyhow::anyhow!("Cannot write output: {}", error))?;

    println!(
        "✅ Generated {} client → {}",
        contract_abi.name,
        output.display()
    );
    println!(
        "   {} functions, {} lang",
        contract_abi.functions.len(),
        match lang {
            CodegenLang::Typescript => "TypeScript",
            CodegenLang::Python => "Python",
        }
    );

    Ok(())
}