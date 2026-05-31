use lichen_core::Keypair;
use serde_json::json;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

const BRIDGE_AUTH_DOMAIN_V2: &str = "LICHEN_BRIDGE_ACCESS_V2";
const BRIDGE_AUTH_CREATE_ACTION: &str = "createBridgeDeposit";

fn usage() -> ! {
    eprintln!(
        "Usage: cargo run -p lichen-rpc --bin bridge_auth_payload -- \
  --chain CHAIN --asset ASSET [--seed-byte N] [--ttl-secs N] [--nonce VALUE]"
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

fn bridge_access_message_v2_create(
    user_id: &str,
    chain: &str,
    asset: &str,
    issued_at: u64,
    expires_at: u64,
    nonce: &str,
) -> Vec<u8> {
    format!(
        "{}\naction={}\nuser_id={}\nchain={}\nasset={}\nroute={}:{}\nissued_at={}\nexpires_at={}\nnonce={}\n",
        BRIDGE_AUTH_DOMAIN_V2,
        BRIDGE_AUTH_CREATE_ACTION,
        user_id,
        chain,
        asset,
        chain,
        asset,
        issued_at,
        expires_at,
        nonce
    )
    .into_bytes()
}

fn canonical_chain(chain: &str) -> String {
    match chain.trim().to_ascii_lowercase().as_str() {
        "bnb" => "bsc".to_string(),
        "neo-x" | "neo_x" => "neox".to_string(),
        other => other.to_string(),
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        usage();
    }

    let mut chain = None;
    let mut asset = None;
    let mut seed_byte: u8 = 42;
    let mut ttl_secs: u64 = 600;
    let mut nonce = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--chain" => chain = Some(next_arg(&args, &mut index, "--chain")),
            "--asset" => asset = Some(next_arg(&args, &mut index, "--asset")),
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

    let chain = canonical_chain(chain.as_deref().unwrap_or_else(|| usage()));
    let asset = asset
        .as_deref()
        .unwrap_or_else(|| usage())
        .trim()
        .to_ascii_lowercase();
    if chain.is_empty() || asset.is_empty() {
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
            "ops-bridge-auth-v2-{}-{}-{}-{}",
            chain,
            asset,
            issued_at,
            std::process::id()
        )
    });

    let message =
        bridge_access_message_v2_create(&user_id, &chain, &asset, issued_at, expires_at, &nonce);
    let signature = keypair.sign(&message);

    let payload = json!({
        "user_id": user_id,
        "chain": chain,
        "asset": asset,
        "auth": {
            "version": 2,
            "domain": BRIDGE_AUTH_DOMAIN_V2,
            "action": BRIDGE_AUTH_CREATE_ACTION,
            "user_id": user_id,
            "chain": chain,
            "asset": asset,
            "route": format!("{}:{}", chain, asset),
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
