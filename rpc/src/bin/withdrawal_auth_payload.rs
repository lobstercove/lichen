use lichen_core::Keypair;
use serde_json::json;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

const WITHDRAWAL_ACCESS_DOMAIN: &str = "LICHEN_WITHDRAWAL_ACCESS_V1";

fn usage() -> ! {
    eprintln!(
        "Usage: cargo run -p lichen-rpc --bin withdrawal_auth_payload -- \
  --asset ASSET --amount SPORES --dest-chain CHAIN --dest-address ADDRESS \
  [--preferred-stablecoin usdt|usdc] [--seed-byte N] [--ttl-secs N] [--nonce VALUE]"
    );
    std::process::exit(2);
}

fn next_arg(args: &[String], index: &mut usize, flag: &str) -> String {
    *index += 1;
    if *index >= args.len() {
        eprintln!("missing value for {}", flag);
        usage();
    }
    args[*index].clone()
}

fn canonical_chain(chain: &str) -> String {
    match chain.trim().to_ascii_lowercase().as_str() {
        "bnb" => "bsc".to_string(),
        "eth" => "ethereum".to_string(),
        "neo-x" | "neo_x" => "neox".to_string(),
        other => other.to_string(),
    }
}

fn withdrawal_access_message(
    user_id: &str,
    asset: &str,
    amount: u64,
    dest_chain: &str,
    dest_address: &str,
    preferred_stablecoin: &str,
    issued_at: u64,
    expires_at: u64,
    nonce: &str,
) -> Vec<u8> {
    format!(
        "{}\nuser_id={}\nasset={}\namount={}\ndest_chain={}\ndest_address={}\npreferred_stablecoin={}\nissued_at={}\nexpires_at={}\nnonce={}\n",
        WITHDRAWAL_ACCESS_DOMAIN,
        user_id,
        asset,
        amount,
        dest_chain,
        dest_address,
        preferred_stablecoin,
        issued_at,
        expires_at,
        nonce,
    )
    .into_bytes()
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        usage();
    }

    let mut asset = None;
    let mut amount = None;
    let mut dest_chain = None;
    let mut dest_address = None;
    let mut preferred_stablecoin = "usdt".to_string();
    let mut seed_byte: u8 = 42;
    let mut ttl_secs: u64 = 600;
    let mut nonce = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--asset" => asset = Some(next_arg(&args, &mut index, "--asset")),
            "--amount" => {
                amount = Some(
                    next_arg(&args, &mut index, "--amount")
                        .parse::<u64>()
                        .unwrap_or_else(|_| {
                            eprintln!("--amount must be an unsigned integer in base units");
                            usage();
                        }),
                )
            }
            "--dest-chain" => dest_chain = Some(next_arg(&args, &mut index, "--dest-chain")),
            "--dest-address" => dest_address = Some(next_arg(&args, &mut index, "--dest-address")),
            "--preferred-stablecoin" => {
                preferred_stablecoin =
                    next_arg(&args, &mut index, "--preferred-stablecoin").to_ascii_lowercase()
            }
            "--seed-byte" => {
                seed_byte = next_arg(&args, &mut index, "--seed-byte")
                    .parse()
                    .unwrap_or_else(|_| {
                        eprintln!("--seed-byte must fit in u8");
                        usage();
                    });
            }
            "--ttl-secs" => {
                ttl_secs = next_arg(&args, &mut index, "--ttl-secs")
                    .parse()
                    .unwrap_or_else(|_| {
                        eprintln!("--ttl-secs must be an unsigned integer");
                        usage();
                    });
            }
            "--nonce" => nonce = Some(next_arg(&args, &mut index, "--nonce")),
            unknown => {
                eprintln!("unknown argument: {}", unknown);
                usage();
            }
        }
        index += 1;
    }

    let asset = asset
        .as_deref()
        .unwrap_or_else(|| usage())
        .trim()
        .to_ascii_lowercase();
    let amount = amount.unwrap_or_else(|| usage());
    let dest_chain = canonical_chain(dest_chain.as_deref().unwrap_or_else(|| usage()));
    let dest_address = dest_address
        .as_deref()
        .unwrap_or_else(|| usage())
        .trim()
        .to_string();
    if asset.is_empty() || dest_chain.is_empty() || dest_address.is_empty() {
        usage();
    }
    if preferred_stablecoin != "usdt" && preferred_stablecoin != "usdc" {
        eprintln!("--preferred-stablecoin must be usdt or usdc");
        usage();
    }

    let keypair = Keypair::from_seed(&[seed_byte; 32]);
    let user_id = keypair.pubkey().to_base58();
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs();
    let expires_at = issued_at + ttl_secs;
    let nonce = nonce.unwrap_or_else(|| {
        format!(
            "ops-withdrawal-auth-{}-{}-{}-{}",
            dest_chain,
            asset,
            issued_at,
            std::process::id()
        )
    });

    let message = withdrawal_access_message(
        &user_id,
        &asset,
        amount,
        &dest_chain,
        &dest_address,
        &preferred_stablecoin,
        issued_at,
        expires_at,
        &nonce,
    );
    let signature = keypair.sign(&message);

    let payload = json!({
        "user_id": user_id,
        "asset": asset,
        "amount": amount,
        "dest_chain": dest_chain,
        "dest_address": dest_address,
        "preferred_stablecoin": preferred_stablecoin,
        "auth": {
            "issued_at": issued_at,
            "expires_at": expires_at,
            "nonce": nonce,
            "signature": signature,
        }
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&payload).expect("encode payload")
    );
}
