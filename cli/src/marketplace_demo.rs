// Marketplace demo seeding tool
// Seeds collections, mints, and marketplace activity for UI parity testing.

use anyhow::{Context, Result};
use base64::Engine;
use clap::Parser;
use moltchain_core::{
    ContractInstruction, CreateCollectionData, Hash, Instruction, Keypair, Message, MintNftData,
    Pubkey, Transaction, CONTRACT_PROGRAM_ID, SYSTEM_PROGRAM_ID,
};
use serde_json::json;
use std::path::PathBuf;

mod keypair_manager;

use keypair_manager::KeypairManager;

#[derive(Parser, Debug)]
#[command(name = "marketplace-demo")]
#[command(about = "Seed marketplace demo data via RPC", long_about = None)]
struct Args {
    /// RPC server URL
    #[arg(long, default_value = "http://localhost:8899")]
    rpc_url: String,

    /// Genesis keypair path
    #[arg(long)]
    keypair: PathBuf,

    /// Number of collections to create
    #[arg(long, default_value_t = 3)]
    collections: usize,

    /// NFTs to mint per collection
    #[arg(long, default_value_t = 4)]
    mints_per_collection: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let signer = load_demo_keypair(&args.keypair).context("Failed to load keypair")?;

    let client = reqwest::Client::new();

    let marketplace_wasm = build_marketplace_wasm();
    let marketplace_program = derive_pubkey(&marketplace_wasm);

    println!("🦞 Seeding marketplace demo data");
    println!("RPC: {}", args.rpc_url);
    println!("Signer: {}", signer.pubkey().to_base58());
    println!("Marketplace program: {}", marketplace_program.to_base58());

    let deploy_ix = Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![signer.pubkey(), marketplace_program],
        data: ContractInstruction::Deploy {
            code: marketplace_wasm.clone(),
            init_data: vec![],
        }
        .serialize()
        .map_err(|e| anyhow::anyhow!("Failed to serialize deploy instruction: {}", e))?,
    };
    send_tx(&client, &args.rpc_url, &signer, vec![deploy_ix]).await?;

    let collection_names = [
        ("MoltPunks", "MOLTP"),
        ("Agent Apes", "APES"),
        ("Cyber Crustaceans", "CRAB"),
        ("Quantum Shells", "QSHL"),
        ("Neon Reefs", "REEF"),
        ("Digital Depths", "DEEP"),
    ];

    let mut collection_pubkeys = Vec::new();

    for index in 0..args.collections {
        let (name, symbol) = collection_names
            .get(index % collection_names.len())
            .copied()
            .unwrap_or(("Molt Collection", "MOLT"));

        let collection_pubkey = derive_pubkey(format!("collection:{}:{}", name, index).as_bytes());
        collection_pubkeys.push(collection_pubkey);

        let collection_data = CreateCollectionData {
            name: name.to_string(),
            symbol: symbol.to_string(),
            royalty_bps: 250,
            max_supply: (args.mints_per_collection * 10) as u64,
            public_mint: true,
            mint_authority: None,
        };

        let mut data = vec![6u8];
        data.extend_from_slice(&bincode::serialize(&collection_data)?);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![signer.pubkey(), collection_pubkey],
            data,
        };

        println!(
            "🧬 Creating collection {} ({})",
            name,
            collection_pubkey.to_base58()
        );
        send_tx(&client, &args.rpc_url, &signer, vec![ix]).await?;
    }

    let demo_accounts = create_demo_accounts(&client, &args.rpc_url, &signer).await?;
    let owner = signer.pubkey();

    for (collection_index, collection_pubkey) in collection_pubkeys.iter().enumerate() {
        for token_index in 0..args.mints_per_collection {
            let token_id = (collection_index as u64) * 10_000 + token_index as u64;
            let token_pubkey = derive_pubkey(
                format!("token:{}:{}", collection_pubkey.to_base58(), token_id).as_bytes(),
            );
            let owner_index = (collection_index + token_index) % demo_accounts.len();
            let token_owner = demo_accounts[owner_index].pubkey();

            let mint_data = MintNftData {
                token_id,
                metadata_uri: format!("ipfs://molt-demo/{}/{}", collection_index, token_id),
            };

            let mut data = vec![7u8];
            data.extend_from_slice(&bincode::serialize(&mint_data)?);

            let ix = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![owner, *collection_pubkey, token_pubkey, token_owner],
                data,
            };

            println!(
                "🎨 Minting token #{} in {}",
                token_id,
                collection_pubkey.to_base58()
            );
            send_tx(&client, &args.rpc_url, &signer, vec![ix]).await?;

            if token_index < 2 {
                let seller_keypair = &demo_accounts[owner_index];
                let listing_args = json!({
                    "collection": collection_pubkey.to_base58(),
                    "token": token_pubkey.to_base58(),
                    "token_id": token_id,
                    "price": 1_500_000_000 + (token_index as u64) * 250_000_000,
                    "seller": token_owner.to_base58(),
                });

                let call_ix = Instruction {
                    program_id: CONTRACT_PROGRAM_ID,
                    accounts: vec![token_owner, marketplace_program],
                    data: ContractInstruction::Call {
                        function: "list_nft".to_string(),
                        args: serde_json::to_vec(&listing_args)?,
                        value: 0,
                    }
                    .serialize()
                    .map_err(|e| anyhow::anyhow!("Failed to serialize call instruction: {}", e))?,
                };

                println!("🏷️  Listing token #{}", token_id);
                send_tx(&client, &args.rpc_url, seller_keypair, vec![call_ix]).await?;
            }

            if token_index == 0 {
                let buyer_index = (owner_index + 1) % demo_accounts.len();
                let buyer_keypair = &demo_accounts[buyer_index];
                let buyer = buyer_keypair.pubkey();
                let sale_args = json!({
                    "collection": collection_pubkey.to_base58(),
                    "token": token_pubkey.to_base58(),
                    "token_id": token_id,
                    "price": 2_200_000_000u64,
                    "seller": token_owner.to_base58(),
                    "buyer": buyer.to_base58(),
                });

                let call_ix = Instruction {
                    program_id: CONTRACT_PROGRAM_ID,
                    accounts: vec![buyer, marketplace_program],
                    data: ContractInstruction::Call {
                        function: "buy_nft".to_string(),
                        args: serde_json::to_vec(&sale_args)?,
                        value: 0,
                    }
                    .serialize()
                    .map_err(|e| anyhow::anyhow!("Failed to serialize call instruction: {}", e))?,
                };

                println!("💸 Recording sale for token #{}", token_id);
                send_tx(&client, &args.rpc_url, buyer_keypair, vec![call_ix]).await?;
            }
        }
    }

    let expected_listings = (args.collections * 2) as u64;
    let expected_sales = args.collections as u64;
    let (listings, sales) =
        poll_market_activity(&client, &args.rpc_url, expected_listings, expected_sales).await?;

    println!("✅ Demo complete");
    println!("Listings: {}", listings);
    println!("Sales: {}", sales);

    Ok(())
}

fn build_marketplace_wasm() -> Vec<u8> {
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, Function, FunctionSection,
        Instruction as WasmInstruction, Module, TypeSection, ValType,
    };

    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.function(Vec::<ValType>::new(), Vec::<ValType>::new());
    module.section(&types);

    let func_type = 0u32;

    let mut functions = FunctionSection::new();
    functions.function(func_type);
    functions.function(func_type);
    module.section(&functions);

    let mut exports = ExportSection::new();
    exports.export("list_nft", ExportKind::Func, 0);
    exports.export("buy_nft", ExportKind::Func, 1);
    module.section(&exports);

    let mut code = CodeSection::new();
    let mut list_fn = Function::new(Vec::new());
    list_fn.instruction(&WasmInstruction::End);
    code.function(&list_fn);

    let mut buy_fn = Function::new(Vec::new());
    buy_fn.instruction(&WasmInstruction::End);
    code.function(&buy_fn);

    module.section(&code);

    module.finish()
}

fn derive_pubkey(seed: &[u8]) -> Pubkey {
    let hash = Hash::hash(seed);
    Pubkey(hash.0)
}

fn load_demo_keypair(path: &PathBuf) -> Result<Keypair> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read keypair file: {}", path.display()))?;

    let json_value: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse keypair file")?;

    if json_value.get("privateKey").is_some() {
        let keypair_manager = KeypairManager::new();
        return keypair_manager.load_keypair(path);
    }

    let secret_hex = json_value
        .get("secret_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing privateKey or secret_key"))?;

    let bytes = hex::decode(secret_hex).context("Failed to decode secret_key hex")?;

    if bytes.len() != 32 {
        anyhow::bail!("Invalid secret_key length: expected 32 bytes");
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(Keypair::from_seed(&seed))
}

async fn send_tx(
    client: &reqwest::Client,
    rpc_url: &str,
    signer: &Keypair,
    instructions: Vec<Instruction>,
) -> Result<String> {
    // Fetch a recent blockhash from the validator for replay protection
    let bh_result = rpc_call(client, rpc_url, "getRecentBlockhash", json!([])).await?;
    let blockhash_str = bh_result
        .get("blockhash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to get recent blockhash from RPC"))?;
    let blockhash_bytes = hex::decode(blockhash_str).context("Failed to decode blockhash")?;
    let mut bh = [0u8; 32];
    if blockhash_bytes.len() >= 32 {
        bh.copy_from_slice(&blockhash_bytes[..32]);
    } else {
        bh[..blockhash_bytes.len()].copy_from_slice(&blockhash_bytes);
    }

    let message = Message {
        instructions,
        recent_blockhash: Hash::new(bh),
    };

    let signature = signer.sign(&message.serialize());
    let tx = Transaction {
        signatures: vec![signature],
        message,
    };

    let tx_bytes = bincode::serialize(&tx)?;
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&tx_bytes);

    let params = json!([tx_base64]);
    let result = rpc_call(client, rpc_url, "sendTransaction", params).await?;

    Ok(result.as_str().unwrap_or_default().to_string())
}

async fn create_demo_accounts(
    client: &reqwest::Client,
    rpc_url: &str,
    signer: &Keypair,
) -> Result<Vec<Keypair>> {
    let mut accounts = Vec::new();
    for index in 0..3 {
        let keypair = Keypair::new();
        let recipient = keypair.pubkey();
        let amount_shells = 20_000_000_000u64;

        let mut data = vec![0u8];
        data.extend_from_slice(&amount_shells.to_le_bytes());

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![signer.pubkey(), recipient],
            data,
        };

        println!(
            "💧 Funding demo account {} (20 MOLT)",
            recipient.to_base58()
        );
        send_tx(client, rpc_url, signer, vec![ix]).await?;

        wait_for_balance(client, rpc_url, &recipient, amount_shells).await?;

        println!("   Demo account {} ready", index + 1);
        accounts.push(keypair);
    }

    Ok(accounts)
}

async fn wait_for_balance(
    client: &reqwest::Client,
    rpc_url: &str,
    pubkey: &Pubkey,
    target_shells: u64,
) -> Result<()> {
    for _ in 0..30 {
        let result = rpc_call(
            client,
            rpc_url,
            "getBalance",
            serde_json::json!([pubkey.to_base58()]),
        )
        .await?;

        let spendable = result
            .get("spendable")
            .and_then(|v| v.as_u64())
            .or_else(|| result.get("shells").and_then(|v| v.as_u64()))
            .unwrap_or(0);

        if spendable >= target_shells {
            return Ok(());
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    anyhow::bail!("Timed out waiting for balance on {}", pubkey.to_base58());
}

async fn rpc_call(
    client: &reqwest::Client,
    rpc_url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let response = client
        .post(rpc_url)
        .json(&request)
        .send()
        .await
        .context("RPC request failed")?;

    let payload: serde_json::Value = response.json().await?;
    if let Some(error) = payload.get("error") {
        anyhow::bail!("RPC error: {}", error);
    }

    Ok(payload
        .get("result")
        .cloned()
        .unwrap_or_else(|| json!(null)))
}

async fn poll_market_activity(
    client: &reqwest::Client,
    rpc_url: &str,
    expected_listings: u64,
    expected_sales: u64,
) -> Result<(u64, u64)> {
    let mut last_listings = 0;
    let mut last_sales = 0;

    for _ in 0..90 {
        let listings = rpc_call(client, rpc_url, "getMarketListings", json!([{}])).await?;
        let sales = rpc_call(client, rpc_url, "getMarketSales", json!([{}])).await?;

        last_listings = listings["count"].as_u64().unwrap_or(0);
        last_sales = sales["count"].as_u64().unwrap_or(0);

        if last_listings >= expected_listings && last_sales >= expected_sales {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    Ok((last_listings, last_sales))
}
