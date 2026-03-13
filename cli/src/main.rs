// MoltChain CLI - Command-line interface for agents
// "Every crab, lobster, and shrimp can access the reef!"

use anyhow::Result;
use clap::{Parser, Subcommand};
use moltchain_core::{Keypair, Pubkey};
use std::path::PathBuf;

mod client;
mod keygen;
mod keypair_manager;
mod wallet;

use client::RpcClient;
use keypair_manager::KeypairManager;
use wallet::WalletManager;

/// MoltChain CLI - Blockchain for autonomous agents
#[derive(Parser)]
#[command(name = "molt")]
#[command(about = "MoltChain CLI - Economic freedom for agents 🦞⚡", long_about = None)]
struct Cli {
    /// RPC server URL
    #[arg(long, default_value = "http://localhost:8899")]
    rpc_url: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Identity management
    #[command(subcommand)]
    Identity(IdentityCommands),

    /// Wallet management (multi-wallet support)
    #[command(subcommand)]
    Wallet(WalletCommands),

    /// Initialize a new validator keypair (alias for 'identity new')
    Init {
        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Check account balance
    Balance {
        /// Account address (Base58 or hex)
        address: Option<String>,

        /// Keypair file to check balance for (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Transfer MOLT to another account
    Transfer {
        /// Destination address (Base58)
        to: String,

        /// Amount in MOLT
        amount: f64,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Request test tokens from faucet
    Airdrop {
        /// Amount in MOLT to request (default: 100)
        #[arg(default_value = "100.0")]
        amount: f64,

        /// Account to receive tokens (defaults to your identity)
        #[arg(short, long)]
        pubkey: Option<String>,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Deploy a smart contract
    Deploy {
        /// WASM contract file path
        contract: PathBuf,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,

        /// Register symbol in the symbol registry (e.g. VYRN)
        #[arg(long)]
        symbol: Option<String>,

        /// Display name for the contract (e.g. "VYRN Token")
        #[arg(long)]
        name: Option<String>,

        /// Contract template category: token, wrapped, dex, defi, nft, governance, infra
        #[arg(long)]
        template: Option<String>,

        /// Token decimals (e.g. 9 for MOLT-style tokens)
        #[arg(long)]
        decimals: Option<u8>,

        /// Total token supply (e.g. 1000000000 for 1B tokens)
        #[arg(long)]
        supply: Option<u64>,

        /// Additional metadata as JSON (e.g. '{"website":"https://example.com"}')
        #[arg(long)]
        metadata: Option<String>,
    },

    /// Upgrade an existing smart contract
    Upgrade {
        /// Contract address (Base58)
        address: String,

        /// New WASM contract file path
        contract: PathBuf,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Call a smart contract function
    Call {
        /// Contract address (Base58)
        contract: String,

        /// Function name to call
        function: String,

        /// Arguments as JSON array (e.g. '[1,2,3]')
        #[arg(short, long, default_value = "[]")]
        args: String,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Get block information
    Block {
        /// Block slot number
        slot: u64,
    },

    /// Get latest block
    Latest,

    /// Get current slot
    Slot,

    /// Get recent blockhash
    Blockhash,

    /// Get total burned MOLT
    Burned,

    /// List all validators
    Validators,

    /// Network operations
    #[command(subcommand)]
    Network(NetworkCommands),

    /// Validator operations
    #[command(subcommand)]
    Validator(ValidatorCommands),

    /// Staking operations
    #[command(subcommand)]
    Stake(StakeCommands),

    /// Account operations
    #[command(subcommand)]
    Account(AccountCommands),

    /// Contract operations
    #[command(subcommand)]
    Contract(ContractCommands),

    /// Show comprehensive chain status
    Status,

    /// Show performance metrics
    Metrics,

    /// Token operations (create, mint, info)
    #[command(subcommand)]
    Token(TokenCommands),

    /// Governance operations (propose, vote, list)
    #[command(subcommand)]
    Gov(GovCommands),
}

#[derive(Subcommand)]
enum NetworkCommands {
    /// Show network status
    Status,

    /// List connected peers
    Peers,

    /// Show network information
    Info,
}

#[derive(Subcommand)]
enum ValidatorCommands {
    /// Show validator information
    Info {
        /// Validator public key (Base58)
        address: String,
    },

    /// Show validator performance metrics
    Performance {
        /// Validator public key (Base58)
        address: String,
    },

    /// Show all validators (same as top-level 'validators' command)
    List,
}

#[derive(Subcommand)]
enum StakeCommands {
    /// Stake MOLT to become a validator
    Add {
        /// Amount in shells to stake
        amount: u64,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Unstake MOLT
    Remove {
        /// Amount in shells to unstake
        amount: u64,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Show staking status
    Status {
        /// Account address (defaults to your identity)
        #[arg(short, long)]
        address: Option<String>,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Show staking rewards
    Rewards {
        /// Account address (defaults to your identity)
        #[arg(short, long)]
        address: Option<String>,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum WalletCommands {
    /// Create a new wallet
    Create {
        /// Wallet name (optional, will auto-generate if not provided)
        name: Option<String>,
    },

    /// Import an existing wallet
    Import {
        /// Wallet name
        name: String,

        /// Path to keypair file to import
        #[arg(short, long)]
        keypair: PathBuf,
    },

    /// List all wallets
    List,

    /// Show wallet details
    Show {
        /// Wallet name
        name: String,
    },

    /// Remove a wallet
    Remove {
        /// Wallet name
        name: String,
    },

    /// Get wallet balance
    Balance {
        /// Wallet name
        name: String,
    },
}

#[derive(Subcommand)]
enum AccountCommands {
    /// Show account details
    Info {
        /// Account address (Base58)
        address: String,
    },

    /// Show transaction history
    History {
        /// Account address (Base58)
        address: String,

        /// Number of transactions to show (default: 10)
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum ContractCommands {
    /// Show contract information
    Info {
        /// Contract address (Base58)
        address: String,
    },

    /// Show contract logs
    Logs {
        /// Contract address (Base58)
        address: String,

        /// Number of logs to show (default: 20)
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// List all deployed contracts
    List,

    /// Register a deployed contract in the symbol registry
    Register {
        /// Contract address (Base58)
        address: String,

        /// Symbol to register (e.g. VYRN)
        #[arg(long)]
        symbol: String,

        /// Display name (e.g. "VYRN Token")
        #[arg(long)]
        name: Option<String>,

        /// Template category: token, wrapped, dex, defi, nft, governance, infra
        #[arg(long)]
        template: Option<String>,

        /// Decimals (e.g. 9)
        #[arg(long)]
        decimals: Option<u8>,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum IdentityCommands {
    /// Create a new identity
    New {
        /// Output file path (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Show your identity
    Show {
        /// Keypair file path (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TokenCommands {
    /// Create and deploy a new token contract
    Create {
        /// Token name (e.g. "VYRN Token")
        name: String,

        /// Token symbol (3-5 chars, e.g. VYRN)
        symbol: String,

        /// WASM contract file for the token
        #[arg(long)]
        wasm: PathBuf,

        /// Decimals (default: 9)
        #[arg(short, long, default_value = "9")]
        decimals: u8,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Get token info
    Info {
        /// Token address / symbol
        token: String,
    },

    /// Mint tokens (token owner only)
    Mint {
        /// Token address
        token: String,

        /// Amount to mint (in whole tokens)
        amount: u64,

        /// Recipient address (defaults to self)
        #[arg(short, long)]
        to: Option<String>,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Transfer tokens
    Send {
        /// Token address
        token: String,

        /// Recipient address
        to: String,

        /// Amount to send (in whole tokens)
        amount: u64,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Get token balance
    Balance {
        /// Token address
        token: String,

        /// Account address (defaults to self)
        #[arg(short, long)]
        address: Option<String>,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// List all registered tokens
    List,
}

#[derive(Subcommand)]
enum GovCommands {
    /// Create a governance proposal
    Propose {
        /// Proposal title
        title: String,

        /// Proposal description
        description: String,

        /// Proposal type: fast-track, standard, constitutional
        #[arg(short = 't', long, default_value = "standard")]
        proposal_type: String,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Vote on a proposal
    Vote {
        /// Proposal ID
        proposal_id: u64,

        /// Vote: yes/no/abstain
        vote: String,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// List active proposals
    List {
        /// Show all proposals (including executed/cancelled)
        #[arg(short, long)]
        all: bool,
    },

    /// Show proposal details
    Info {
        /// Proposal ID
        proposal_id: u64,
    },

    /// Execute a passed proposal
    Execute {
        /// Proposal ID
        proposal_id: u64,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Veto a proposal during time-lock
    Veto {
        /// Proposal ID
        proposal_id: u64,

        /// Keypair file (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },
}

/// Convert MOLT (f64) to shells (u64) with precise integer arithmetic.
/// Avoids floating-point precision loss for amounts near the f64 precision boundary
/// by splitting into whole and fractional parts and computing with integers.
fn molt_to_shells(molt: f64) -> u64 {
    if molt <= 0.0 {
        return 0;
    }
    // Clamp to max safe value representable in u64 shells
    if molt >= (u64::MAX / 1_000_000_000) as f64 {
        return u64::MAX;
    }
    let whole = molt.trunc() as u64;
    // Extract fractional part, round to 9 decimal places to avoid float noise
    let frac = ((molt.fract() * 1_000_000_000.0).round()) as u64;
    whole.saturating_mul(1_000_000_000).saturating_add(frac)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = RpcClient::new(&cli.rpc_url);
    let keypair_mgr = KeypairManager::new();

    match cli.command {
        Commands::Identity(id_cmd) => match id_cmd {
            IdentityCommands::New { output } => {
                let keypair = Keypair::new();
                let pubkey = keypair.pubkey();

                let path = output.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                keypair_mgr.save_keypair(&keypair, &path)?;

                println!("🦞 Generated new identity!");
                println!("📍 Pubkey: {}", pubkey.to_base58());
                println!("🔐 EVM Address: {}", pubkey.to_evm());
                println!("💾 Saved to: {}", path.display());
                println!();
                println!("💡 Get test tokens: molt airdrop 100");
            }

            IdentityCommands::Show { keypair } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let kp = keypair_mgr.load_keypair(&path)?;
                let pubkey = kp.pubkey();

                println!("🦞 Your MoltChain Identity");
                println!("📍 Pubkey: {}", pubkey.to_base58());
                println!("🔐 EVM Address: {}", pubkey.to_evm());
                println!("📄 Keypair: {}", path.display());
            }
        },

        Commands::Wallet(wallet_cmd) => {
            let wallet_mgr = WalletManager::new()?;

            match wallet_cmd {
                WalletCommands::Create { name } => {
                    wallet_mgr.create_wallet(name)?;
                }

                WalletCommands::Import { name, keypair } => {
                    wallet_mgr.import_wallet(name, keypair)?;
                }

                WalletCommands::List => {
                    wallet_mgr.list_wallets()?;
                }

                WalletCommands::Show { name } => {
                    wallet_mgr.show_wallet(&name)?;
                }

                WalletCommands::Remove { name } => {
                    wallet_mgr.remove_wallet(&name)?;
                }

                WalletCommands::Balance { name } => {
                    let wallet = wallet_mgr.get_wallet(&name)?;
                    let pubkey = Pubkey::from_base58(&wallet.address)
                        .map_err(|e| anyhow::anyhow!("Invalid address: {}", e))?;
                    let balance = client.get_balance(&pubkey).await?;
                    let to_molt = |shells: u64| shells as f64 / 1_000_000_000.0;

                    println!("\n🦞 Wallet: {}", wallet.name);
                    println!("📍 Address: {}", wallet.address);
                    println!("─────────────────────────────────────────────────────────");
                    println!("💰 Total:     {:>12.4} MOLT", to_molt(balance.shells));
                    println!("   Spendable: {:>12.4} MOLT", to_molt(balance.spendable));
                    println!("   Staked:    {:>12.4} MOLT", to_molt(balance.staked));
                    println!("   Locked:    {:>12.4} MOLT", to_molt(balance.locked));
                    println!("─────────────────────────────────────────────────────────\n");
                }
            }
        }

        Commands::Init { output } => {
            let keypair = Keypair::new();
            let pubkey = keypair.pubkey();

            let path = match output {
                Some(p) => p,
                None => {
                    eprintln!("Error: --output is required for init command");
                    std::process::exit(1);
                }
            };

            keypair_mgr.save_keypair(&keypair, &path)?;

            println!("🦞 Validator keypair initialized!");
            println!("📍 Pubkey: {}", pubkey.to_base58());
            println!("💾 Saved to: {}", path.display());
        }

        Commands::Balance { address, keypair } => {
            let pubkey = if let Some(addr) = address {
                if addr.starts_with("0x") {
                    anyhow::bail!(
                        "EVM addresses not yet supported for balance queries. Use Base58 format."
                    );
                } else {
                    Pubkey::from_base58(&addr)
                        .map_err(|e| anyhow::anyhow!("Invalid Base58 address: {}", e))?
                }
            } else {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let kp = keypair_mgr.load_keypair(&path)?;
                kp.pubkey()
            };

            let balance = client.get_balance(&pubkey).await?;
            let to_molt = |shells: u64| shells as f64 / 1_000_000_000.0;

            println!("\n🦞 Balance for {}", pubkey.to_base58());
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!(
                "💰 Total:     {:>12.4} MOLT ({} shells)",
                to_molt(balance.shells),
                balance.shells
            );
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!(
                "   Spendable: {:>12.4} MOLT (available for transfers)",
                to_molt(balance.spendable)
            );
            println!(
                "   Staked:    {:>12.4} MOLT (locked in validation)",
                to_molt(balance.staked)
            );
            println!(
                "   Locked:    {:>12.4} MOLT (locked in contracts)",
                to_molt(balance.locked)
            );
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
        }

        Commands::Transfer {
            to,
            amount,
            keypair,
        } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let from_keypair = keypair_mgr.load_keypair(&path)?;
            let from_pubkey = from_keypair.pubkey();

            let to_pubkey = Pubkey::from_base58(&to)
                .map_err(|e| anyhow::anyhow!("Invalid destination address: {}", e))?;
            let shells = molt_to_shells(amount);

            println!("🦞 Transferring {} MOLT ({} shells)", amount, shells);
            println!("📤 From: {}", from_pubkey.to_base58());
            println!("📥 To: {}", to_pubkey.to_base58());

            let signature = client.transfer(&from_keypair, &to_pubkey, shells).await?;

            println!("✅ Transaction sent!");
            println!("📝 Signature: {}", signature);
        }

        Commands::Block { slot } => {
            let block = client.get_block(slot).await?;

            println!("🧊 Block #{}", slot);
            println!("🔗 Hash: {}", block.hash);
            println!("⬅️  Parent: {}", block.parent_hash);
            println!("🌳 State Root: {}", block.state_root);
            println!("🦞 Validator: {}", block.validator);
            println!("⏰ Timestamp: {}", block.timestamp);
            println!("📦 Transactions: {}", block.transaction_count);
        }

        Commands::Slot => {
            let slot = client.get_slot().await?;
            println!("🦞 Current slot: {}", slot);
        }

        Commands::Blockhash => {
            let hash = client.get_recent_blockhash().await?;
            println!("🦞 Recent blockhash: {}", hash);
        }

        Commands::Latest => {
            let block = client.get_latest_block().await?;

            println!("🧊 Latest Block #{}", block.slot);
            println!("🔗 Hash: {}", block.hash);
            println!("⬅️  Parent: {}", block.parent_hash);
            println!("🌳 State Root: {}", block.state_root);
            println!("🦞 Validator: {}", block.validator);
            println!("⏰ Timestamp: {}", block.timestamp);
            println!("📦 Transactions: {}", block.transaction_count);
        }

        Commands::Burned => {
            let burned = client.get_total_burned().await?;
            let molt = burned.shells as f64 / 1_000_000_000.0;
            println!("🔥 Total MOLT Burned");
            println!("💰 {} MOLT ({} shells)", molt, burned.shells);
            println!();
            println!(
                "Deflationary mechanism: 50% of all transaction fees are burned forever! 🦞⚡"
            );
        }

        Commands::Validators => {
            let validators_info = client.get_validators().await?;
            let validators = &validators_info.validators;

            println!("🦞 Active Validators");
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!();

            if validators.is_empty() {
                println!("No validators found");
            } else {
                for (i, v) in validators.iter().enumerate() {
                    println!("#{} {}", i + 1, v.pubkey);
                    println!("   Stake: {} MOLT", v.stake as f64 / 1_000_000_000.0);
                    println!("   Reputation: {}", v.reputation);
                    println!();
                }

                let total_stake: u64 = validators.iter().map(|v| v.stake).sum();
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!(
                    "Total: {} validators, {} MOLT staked",
                    validators.len(),
                    total_stake as f64 / 1_000_000_000.0
                );
            }
        }

        Commands::Network(net_cmd) => match net_cmd {
            NetworkCommands::Status => {
                println!("🦞 Network Status");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

                // Get current slot
                let slot = client.get_slot().await?;
                println!("📊 Current slot: {}", slot);

                // Get validator count
                let validators_info = client.get_validators().await?;
                println!("👥 Active validators: {}", validators_info.validators.len());

                // Get metrics
                match client.get_metrics().await {
                    Ok(metrics) => {
                        println!("⚡ TPS: {}", metrics.tps);
                        println!("📦 Total blocks: {}", metrics.total_blocks);
                        println!("📝 Total transactions: {}", metrics.total_transactions);
                    }
                    Err(_) => {
                        println!("⚠️  Metrics unavailable");
                    }
                }

                println!();
                println!("✅ Network is healthy");
            }

            NetworkCommands::Peers => {
                println!("🦞 Connected Peers");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_peers().await {
                    Ok(peers) => {
                        if peers.is_empty() {
                            println!("No connected peers");
                        } else {
                            for (i, peer) in peers.iter().enumerate() {
                                println!(
                                    "#{} {} ({})",
                                    i + 1,
                                    peer.peer_id,
                                    if peer.connected {
                                        "Connected"
                                    } else {
                                        "Disconnected"
                                    }
                                );
                                println!("   Address: {}", peer.address);
                            }
                            println!();
                            println!("Total: {} peers", peers.len());
                        }
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch peers: {}", e);
                        println!("💡 Make sure the validator is running");
                    }
                }
            }

            NetworkCommands::Info => {
                println!("🦞 Network Information");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_network_info().await {
                    Ok(info) => {
                        println!("🌐 Network ID: {}", info.network_id);
                        println!("⛓️  Chain ID: {}", info.chain_id);
                        println!("🔗 RPC Endpoint: {}", cli.rpc_url);
                        println!();
                        println!("📊 Statistics:");
                        println!("   Current slot: {}", info.current_slot);
                        println!("   Validators: {}", info.validator_count);
                        println!("   TPS: {}", info.tps);
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch network info: {}", e);
                    }
                }
            }
        },

        Commands::Validator(val_cmd) => match val_cmd {
            ValidatorCommands::Info { address } => {
                println!("🦞 Validator Information");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_validator_info(&address).await {
                    Ok(info) => {
                        println!("📍 Pubkey: {}", info.pubkey);
                        println!("💰 Stake: {} MOLT", info.stake as f64 / 1_000_000_000.0);
                        println!("⭐ Reputation: {}", info.reputation);
                        println!(
                            "📊 Status: {}",
                            if info.is_active { "Active" } else { "Inactive" }
                        );
                        println!("📦 Blocks produced: {}", info.blocks_produced);
                    }
                    Err(e) => {
                        println!("⚠️  Validator not found: {}", e);
                    }
                }
            }

            ValidatorCommands::Performance { address } => {
                println!("🦞 Validator Performance");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_validator_performance(&address).await {
                    Ok(perf) => {
                        println!("📍 Validator: {}", address);
                        println!();
                        println!("📊 Epoch Performance:");
                        println!("   Blocks produced: {}", perf.blocks_produced);
                        println!("   Blocks expected: {}", perf.blocks_expected);
                        let success_rate = if perf.blocks_expected > 0 {
                            (perf.blocks_produced as f64 / perf.blocks_expected as f64) * 100.0
                        } else {
                            0.0
                        };
                        println!("   Success rate: {:.2}%", success_rate);
                        println!("   Average block time: {}ms", perf.avg_block_time_ms);
                        println!();
                        println!("⏰ Uptime: {:.2}%", perf.uptime_percent);
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch performance: {}", e);
                    }
                }
            }

            ValidatorCommands::List => {
                // Same as top-level Validators command
                let validators_info = client.get_validators().await?;
                let validators = &validators_info.validators;

                println!("🦞 Active Validators");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                if validators.is_empty() {
                    println!("No validators found");
                } else {
                    for (i, v) in validators.iter().enumerate() {
                        println!("#{} {}", i + 1, v.pubkey);
                        println!("   Stake: {} MOLT", v.stake as f64 / 1_000_000_000.0);
                        println!("   Reputation: {}", v.reputation);
                        println!();
                    }

                    let total_stake: u64 = validators.iter().map(|v| v.stake).sum();
                    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                    println!(
                        "Total: {} validators, {} MOLT staked",
                        validators.len(),
                        total_stake as f64 / 1_000_000_000.0
                    );
                }
            }
        },

        Commands::Stake(stake_cmd) => match stake_cmd {
            StakeCommands::Add { amount, keypair } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let kp = keypair_mgr.load_keypair(&path)?;

                println!("🦞 Staking MOLT");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!("💰 Amount: {} MOLT", amount as f64 / 1_000_000_000.0);
                println!("👤 Validator: {}", kp.pubkey().to_base58());
                println!();

                match client.stake(&kp, amount).await {
                    Ok(signature) => {
                        println!("✅ Stake transaction sent!");
                        println!("📝 Signature: {}", signature);
                        println!();
                        println!("💡 Your stake will be active in the next epoch");
                    }
                    Err(e) => {
                        println!("⚠️  Staking failed: {}", e);
                    }
                }
            }

            StakeCommands::Remove { amount, keypair } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let kp = keypair_mgr.load_keypair(&path)?;

                println!("🦞 Unstaking MOLT");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!("💰 Amount: {} MOLT", amount as f64 / 1_000_000_000.0);
                println!("👤 Validator: {}", kp.pubkey().to_base58());
                println!();

                match client.unstake(&kp, amount).await {
                    Ok(signature) => {
                        println!("✅ Unstake transaction sent!");
                        println!("📝 Signature: {}", signature);
                        println!();
                        println!("💡 Tokens will be available after unbonding period");
                    }
                    Err(e) => {
                        println!("⚠️  Unstaking failed: {}", e);
                    }
                }
            }

            StakeCommands::Status { address, keypair } => {
                let addr_str = if let Some(addr) = address {
                    addr
                } else {
                    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                    let kp = keypair_mgr.load_keypair(&path)?;
                    kp.pubkey().to_base58()
                };

                println!("🦞 Staking Status");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_staking_status(&addr_str).await {
                    Ok(status) => {
                        println!("👤 Account: {}", status.address);
                        println!("💰 Staked: {} MOLT", status.staked as f64 / 1_000_000_000.0);
                        println!(
                            "📊 Status: {}",
                            if status.is_validator {
                                "Active Validator"
                            } else {
                                "Not Validating"
                            }
                        );
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch staking status: {}", e);
                    }
                }
            }

            StakeCommands::Rewards { address, keypair } => {
                let addr_str = if let Some(addr) = address {
                    addr
                } else {
                    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                    let kp = keypair_mgr.load_keypair(&path)?;
                    kp.pubkey().to_base58()
                };

                println!("🦞 Staking Rewards");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_staking_rewards(&addr_str).await {
                    Ok(rewards) => {
                        println!("👤 Account: {}", rewards.address);
                        println!(
                            "💰 Total rewards: {} MOLT",
                            rewards.total_rewards as f64 / 1_000_000_000.0
                        );
                        println!(
                            "⏳ Pending rewards: {} MOLT",
                            rewards.pending_rewards as f64 / 1_000_000_000.0
                        );
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch rewards: {}", e);
                    }
                }
            }
        },

        Commands::Account(acc_cmd) => match acc_cmd {
            AccountCommands::Info { address } => {
                println!("🦞 Account Information");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_account_info(&address).await {
                    Ok(info) => {
                        println!("📍 Address: {}", info.pubkey);
                        println!("💰 Balance: {} MOLT ({} shells)", info.molt, info.balance);
                        println!("📦 Exists: {}", if info.exists { "Yes" } else { "No" });
                        println!(
                            "⚙️  Executable: {}",
                            if info.is_executable {
                                "Yes (Contract)"
                            } else {
                                "No"
                            }
                        );
                        println!(
                            "🦞 Validator: {}",
                            if info.is_validator { "Yes" } else { "No" }
                        );
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch account info: {}", e);
                    }
                }
            }

            AccountCommands::History { address, limit } => {
                println!("🦞 Transaction History");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!("📍 Account: {}", address);
                println!("📊 Showing last {} transactions", limit);
                println!();

                match client.get_transaction_history(&address, limit).await {
                    Ok(txs) => {
                        if txs.is_empty() {
                            println!("No transactions found");
                        } else {
                            for (i, tx) in txs.iter().enumerate() {
                                println!("#{} Slot {}", i + 1, tx.slot);
                                println!("   Signature: {}", tx.signature);
                                println!("   From: {}", tx.from);
                                println!("   To: {}", tx.to);
                                println!("   Amount: {} MOLT", tx.amount as f64 / 1_000_000_000.0);
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch transaction history: {}", e);
                    }
                }
            }
        },

        Commands::Contract(contract_cmd) => match contract_cmd {
            ContractCommands::Info { address } => {
                println!("🦞 Contract Information");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_contract_info(&address).await {
                    Ok(info) => {
                        println!("📍 Address: {}", info.address);
                        println!("👤 Deployer: {}", info.deployer);
                        println!("📏 Code size: {} bytes", info.code_size);
                        println!("📅 Deployed at slot: {}", info.deployed_at);
                    }
                    Err(e) => {
                        println!("⚠️  Contract not found: {}", e);
                    }
                }
            }

            ContractCommands::Logs { address, limit } => {
                println!("🦞 Contract Logs");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!("📍 Contract: {}", address);
                println!("📊 Showing last {} logs", limit);
                println!();

                match client.get_contract_logs(&address, limit).await {
                    Ok(logs) => {
                        if logs.is_empty() {
                            println!("No logs found");
                        } else {
                            for (i, log) in logs.iter().enumerate() {
                                println!("#{} [Slot {}] {}", i + 1, log.slot, log.message);
                            }
                            println!();
                            println!("Total: {} log entries", logs.len());
                        }
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch contract logs: {}", e);
                    }
                }
            }

            ContractCommands::List => {
                println!("🦞 Deployed Contracts");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_all_contracts().await {
                    Ok(contracts) => {
                        if contracts.is_empty() {
                            println!("No contracts deployed");
                        } else {
                            for (i, contract) in contracts.iter().enumerate() {
                                println!("#{} {}", i + 1, contract.address);
                                println!("   Deployer: {}", contract.deployer);
                                println!();
                            }
                            println!("Total: {} contracts", contracts.len());
                        }
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch contracts: {}", e);
                    }
                }
            }

            ContractCommands::Register {
                address,
                symbol,
                name,
                template,
                decimals,
                keypair,
            } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let owner = keypair_mgr.load_keypair(&path)?;
                let contract_pubkey = moltchain_core::Pubkey::from_base58(&address)
                    .map_err(|e| anyhow::anyhow!("Invalid contract address: {}", e))?;

                println!("🏷️  Registering contract in symbol registry");
                println!("📍 Contract: {}", address);
                println!("🏷️  Symbol: {}", symbol);
                if let Some(ref n) = name {
                    println!("📛 Name: {}", n);
                }
                if let Some(ref t) = template {
                    println!("📂 Template: {}", t);
                }
                if let Some(d) = decimals {
                    println!("🔢 Decimals: {}", d);
                }
                println!("👤 Owner: {}", owner.pubkey().to_base58());
                println!();

                let signature = client
                    .register_symbol(
                        &owner,
                        &contract_pubkey,
                        &symbol,
                        name.as_deref(),
                        template.as_deref(),
                        decimals,
                    )
                    .await?;

                println!("✅ Symbol registered!");
                println!("📝 Signature: {}", signature);
            }
        },

        Commands::Status => {
            println!("🦞 MoltChain Status");
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!();

            // Get comprehensive chain status
            match client.get_chain_status().await {
                Ok(status) => {
                    println!("⛓️  Chain: {}", status.chain_id);
                    println!("🌐 Network: {}", status.network);
                    println!();

                    println!("📊 Block Production:");
                    println!("   Current slot: {}", status.current_slot);
                    println!("   Latest block: {}", status.latest_block);
                    println!("   Block time: {}ms", status.block_time_ms);
                    println!();

                    println!("👥 Network:");
                    println!("   Validators: {}", status.validator_count);
                    println!("   Connected peers: {}", status.peer_count);
                    println!();

                    println!("📈 Activity:");
                    println!("   TPS: {}", status.tps);
                    println!("   Total transactions: {}", status.total_transactions);
                    println!("   Total blocks: {}", status.total_blocks);
                    println!();

                    println!("💰 Economics:");
                    println!(
                        "   Total supply: {} MOLT",
                        status.total_supply as f64 / 1_000_000_000.0
                    );
                    println!(
                        "   Total burned: {} MOLT",
                        status.total_burned as f64 / 1_000_000_000.0
                    );
                    println!(
                        "   Total staked: {} MOLT",
                        status.total_staked as f64 / 1_000_000_000.0
                    );
                    println!();

                    println!("✅ Chain is healthy");
                }
                Err(e) => {
                    println!("⚠️  Could not fetch chain status: {}", e);
                }
            }
        }

        Commands::Metrics => {
            println!("🦞 Chain Metrics");
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!();

            match client.get_metrics().await {
                Ok(metrics) => {
                    println!("📊 Performance:");
                    println!("   TPS: {}", metrics.tps);
                    println!("   Average block time: {}ms", metrics.avg_block_time_ms);
                    println!(
                        "   Transactions per block: {:.1}",
                        metrics.avg_txs_per_block
                    );
                    println!();

                    println!("📈 Totals:");
                    println!("   Blocks: {}", metrics.total_blocks);
                    println!("   Transactions: {}", metrics.total_transactions);
                    println!("   Accounts: {}", metrics.total_accounts);
                    println!("   Contracts: {}", metrics.total_contracts);
                    println!();

                    println!("💰 Economics:");
                    println!(
                        "   Total supply: {} MOLT",
                        metrics.total_supply as f64 / 1_000_000_000.0
                    );
                    println!(
                        "   Circulating: {} MOLT",
                        metrics.circulating_supply as f64 / 1_000_000_000.0
                    );
                    let burn_pct = if metrics.total_supply > 0 {
                        (metrics.total_burned as f64 / metrics.total_supply as f64) * 100.0
                    } else {
                        0.0
                    };
                    let stake_pct = if metrics.total_supply > 0 {
                        (metrics.total_staked as f64 / metrics.total_supply as f64) * 100.0
                    } else {
                        0.0
                    };
                    println!(
                        "   Burned: {} MOLT ({:.2}%)",
                        metrics.total_burned as f64 / 1_000_000_000.0,
                        burn_pct
                    );
                    println!(
                        "   Staked: {} MOLT ({:.2}%)",
                        metrics.total_staked as f64 / 1_000_000_000.0,
                        stake_pct
                    );
                }
                Err(e) => {
                    println!("⚠️  Could not fetch metrics: {}", e);
                }
            }
        }

        Commands::Airdrop {
            amount,
            pubkey,
            keypair,
        } => {
            // AUDIT-FIX I-1: Use requestAirdrop RPC (faucet mint) instead of user self-transfer
            let recipient = if let Some(addr) = pubkey {
                Pubkey::from_base58(&addr).map_err(|e| anyhow::anyhow!("Invalid address: {}", e))?
            } else {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let kp = keypair_mgr.load_keypair(&path)?;
                kp.pubkey()
            };

            println!("🦞 Requesting {} MOLT airdrop...", amount);
            println!("📥 To: {}", recipient.to_base58());
            println!();

            match client.request_airdrop(&recipient, amount).await {
                Ok(signature) => {
                    println!("✅ Airdrop received!");
                    println!("📝 Signature: {}", signature);
                }
                Err(e) => {
                    println!("⚠️  Airdrop failed: {}", e);
                    println!("💡 Ensure the node is running in testnet/devnet mode");
                }
            }
        }

        Commands::Deploy {
            contract,
            keypair,
            symbol,
            name,
            template,
            decimals,
            supply,
            metadata,
        } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let deployer = keypair_mgr.load_keypair(&path)?;

            // Read WASM file
            let wasm_code = std::fs::read(&contract)
                .map_err(|e| anyhow::anyhow!("Failed to read contract file: {}", e))?;

            // Pre-flight validation: WASM magic bytes
            const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
            if wasm_code.len() < 8 || wasm_code[..4] != WASM_MAGIC {
                anyhow::bail!(
                    "Invalid WASM file: {} does not have valid WASM magic bytes (\\0asm).\n\
                     Make sure you compiled with: cargo build --target wasm32-unknown-unknown --release\n\
                     The WASM file is at: target/wasm32-unknown-unknown/release/<name>.wasm",
                    contract.display()
                );
            }

            // Pre-flight validation: size limit (512 KB, matching on-chain limit)
            const MAX_CONTRACT_SIZE: usize = 512 * 1024;
            if wasm_code.len() > MAX_CONTRACT_SIZE {
                anyhow::bail!(
                    "Contract too large: {} bytes (max {} bytes = 512 KB).\n\
                     Tip: use wasm-opt or enable LTO in your Cargo.toml [profile.release]",
                    wasm_code.len(),
                    MAX_CONTRACT_SIZE
                );
            }

            if wasm_code.is_empty() {
                anyhow::bail!("Contract file is empty");
            }

            // Generate contract address (deterministic from deployer + code hash)
            use moltchain_core::Hash;
            let code_hash = Hash::hash(&wasm_code);
            let mut addr_bytes = [0u8; 32];
            addr_bytes[..16].copy_from_slice(&deployer.pubkey().0[..16]);
            addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
            let contract_addr = moltchain_core::Pubkey(addr_bytes);

            // Build init_data for symbol registry if any metadata flags provided
            let init_data = if symbol.is_some()
                || name.is_some()
                || template.is_some()
                || decimals.is_some()
                || supply.is_some()
                || metadata.is_some()
            {
                let mut registry = serde_json::Map::new();
                if let Some(ref s) = symbol {
                    registry.insert("symbol".to_string(), serde_json::json!(s));
                }
                if let Some(ref n) = name {
                    registry.insert("name".to_string(), serde_json::json!(n));
                }
                if let Some(ref t) = template {
                    registry.insert("template".to_string(), serde_json::json!(t));
                }
                if let Some(d) = decimals {
                    registry.insert("decimals".to_string(), serde_json::json!(d));
                }

                // Build metadata object — merge --supply and --metadata
                let mut meta = if let Some(ref m) = metadata {
                    serde_json::from_str::<serde_json::Value>(m)
                        .map_err(|e| anyhow::anyhow!("Invalid --metadata JSON: {}", e))?
                        .as_object()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    serde_json::Map::new()
                };
                if let Some(s) = supply {
                    let decs = decimals.unwrap_or(9) as u32;
                    let total_shells = (s as u128) * 10u128.pow(decs);
                    meta.insert(
                        "total_supply".to_string(),
                        serde_json::json!(total_shells.to_string()),
                    );
                }
                if !meta.is_empty() {
                    registry.insert("metadata".to_string(), serde_json::json!(meta));
                }

                serde_json::to_vec(&registry).unwrap_or_default()
            } else {
                vec![]
            };

            println!("🦞 Deploying contract: {}", contract.display());
            println!("📦 Size: {} KB", wasm_code.len() / 1024);
            println!("📍 Contract address: {}", contract_addr.to_base58());
            println!("👤 Deployer: {}", deployer.pubkey().to_base58());
            if let Some(ref s) = symbol {
                println!("🏷️  Symbol: {}", s);
            }
            if let Some(ref t) = template {
                println!("📂 Template: {}", t);
            }
            if let Some(s) = supply {
                println!(
                    "💎 Total supply: {} (decimals: {})",
                    s,
                    decimals.unwrap_or(9)
                );
            }
            println!("💰 Deploy fee: 25.001 MOLT (25 MOLT deploy + 0.001 MOLT base fee)");
            println!();

            let signature = client
                .deploy_contract(&deployer, wasm_code, &contract_addr, init_data)
                .await?;

            println!("📝 Signature: {}", signature);

            // Phase 1: Wait for transaction confirmation (up to 15s)
            println!("⏳ Waiting for transaction confirmation...");
            let mut tx_confirmed = false;
            let mut tx_error: Option<String> = None;
            for attempt in 1..=15 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                match client.confirm_transaction(&signature).await {
                    Ok(Some(true)) => {
                        tx_confirmed = true;
                        break;
                    }
                    Ok(Some(false) | None) => {
                        if attempt % 5 == 0 {
                            println!("   ...still waiting ({}/15s)", attempt);
                        }
                    }
                    Err(e) => {
                        tx_error = Some(e.to_string());
                        break;
                    }
                }
            }

            if let Some(ref err) = tx_error {
                println!("❌ Deploy transaction FAILED on-chain: {}", err);
                println!("   The deploy fee premium (25 MOLT) should be refunded.");
                println!("   Only the base fee (0.001 MOLT) is kept.");
                println!("   Check your balance: molt balance --keypair <keypair>");
            } else if tx_confirmed {
                // Phase 2: Verify contract account exists
                let mut verified = false;
                for attempt in 1..=5 {
                    match client.get_account_info(&contract_addr.to_base58()).await {
                        Ok(info) if info.is_executable => {
                            verified = true;
                            break;
                        }
                        _ if attempt < 5 => {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                        _ => break,
                    }
                }
                if verified {
                    println!("✅ Contract deployed and verified on-chain!");
                } else {
                    println!(
                        "⚠️  Transaction confirmed but contract not found at expected address."
                    );
                    println!("   This is a known issue — please report the following:");
                    println!("   Signature: {}", signature);
                    println!("   Expected:  {}", contract_addr.to_base58());
                }
            } else {
                println!("⚠️  Transaction not confirmed after 15 seconds.");
                println!("   The transaction may still be processing. Check:");
                println!("   molt balance --keypair <keypair>");
                println!(
                    "   Explorer: https://explorer.moltchain.network/address/{}",
                    contract_addr.to_base58()
                );
            }
            println!("🔗 Address: {}", contract_addr.to_base58());
            if symbol.is_some() {
                println!("🏷️  Symbol registered in symbol registry");
            }
        }

        Commands::Upgrade {
            address,
            contract,
            keypair,
        } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let owner = keypair_mgr.load_keypair(&path)?;

            let contract_pubkey = moltchain_core::Pubkey::from_base58(&address)
                .map_err(|e| anyhow::anyhow!("Invalid contract address: {}", e))?;

            let wasm_code = std::fs::read(&contract)
                .map_err(|e| anyhow::anyhow!("Failed to read contract file: {}", e))?;

            println!("🦞 Upgrading contract: {}", contract_pubkey.to_base58());
            println!("📦 New code size: {} KB", wasm_code.len() / 1024);
            println!("👤 Owner: {}", owner.pubkey().to_base58());
            println!();

            let signature = client
                .upgrade_contract(&owner, wasm_code, &contract_pubkey)
                .await?;

            println!("✅ Contract upgraded!");
            println!("📝 Signature: {}", signature);
            println!("🔗 Address: {}", contract_pubkey.to_base58());
        }

        Commands::Call {
            contract,
            function,
            args,
            keypair,
        } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let caller = keypair_mgr.load_keypair(&path)?;
            let contract_addr = moltchain_core::Pubkey::from_base58(&contract)
                .map_err(|e| anyhow::anyhow!("Invalid contract address: {}", e))?;

            // Parse JSON args
            let args_json: Vec<serde_json::Value> = serde_json::from_str(&args)
                .map_err(|e| anyhow::anyhow!("Invalid args JSON: {}", e))?;
            let args_bytes = serde_json::to_vec(&args_json)?;

            println!("🦞 Calling contract: {}", contract);
            println!("📞 Function: {}", function);
            println!("📋 Args: {}", args);
            println!();

            let signature = client
                .call_contract(
                    &caller,
                    &contract_addr,
                    function.clone(),
                    args_bytes,
                    0, // No value transfer
                )
                .await?;

            println!("✅ Contract called!");
            println!("📝 Signature: {}", signature);
        }

        // ====================================================================
        // TOKEN COMMANDS
        // ====================================================================
        Commands::Token(token_cmd) => match token_cmd {
            TokenCommands::Create {
                name,
                symbol,
                wasm,
                decimals,
                keypair,
            } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let deployer = keypair_mgr.load_keypair(&path)?;

                // Read WASM file
                let wasm_code = std::fs::read(&wasm)
                    .map_err(|e| anyhow::anyhow!("Failed to read WASM file: {}", e))?;

                const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
                if wasm_code.len() < 8 || wasm_code[..4] != WASM_MAGIC {
                    anyhow::bail!(
                        "Invalid WASM file: {} does not have valid WASM magic bytes.\n\
                         Compile with: cargo build --target wasm32-unknown-unknown --release",
                        wasm.display()
                    );
                }

                const MAX_CONTRACT_SIZE: usize = 512 * 1024;
                if wasm_code.len() > MAX_CONTRACT_SIZE {
                    anyhow::bail!(
                        "Contract too large: {} bytes (max {} bytes = 512 KB)",
                        wasm_code.len(),
                        MAX_CONTRACT_SIZE
                    );
                }

                // Generate contract address
                use moltchain_core::Hash;
                let code_hash = Hash::hash(&wasm_code);
                let mut addr_bytes = [0u8; 32];
                addr_bytes[..16].copy_from_slice(&deployer.pubkey().0[..16]);
                addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
                let contract_addr = moltchain_core::Pubkey(addr_bytes);

                // Build init_data with token template metadata
                let init_data = serde_json::json!({
                    "symbol": symbol,
                    "name": name,
                    "template": "token",
                    "decimals": decimals,
                });
                let init_data_bytes = serde_json::to_vec(&init_data).unwrap_or_default();

                println!("🪙 Deploying token: {} ({})", name, symbol);
                println!(
                    "📦 WASM: {} ({} KB)",
                    wasm.display(),
                    wasm_code.len() / 1024
                );
                println!("📍 Contract address: {}", contract_addr.to_base58());
                println!("👤 Creator: {}", deployer.pubkey().to_base58());
                println!("🔢 Decimals: {}", decimals);
                println!("💰 Deploy fee: 25.001 MOLT (25 MOLT deploy + 0.001 MOLT base fee)");
                println!();

                let signature = client
                    .deploy_contract(&deployer, wasm_code, &contract_addr, init_data_bytes)
                    .await?;

                println!("✅ Token deployed and registered!");
                println!("📝 Signature: {}", signature);
                println!("🔗 Address: {}", contract_addr.to_base58());
                println!("🏷️  Symbol: {} registered in symbol registry", symbol);
            }
            TokenCommands::Info { token } => {
                println!("🪙 Token Info: {}", token);
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

                let _contract_addr = moltchain_core::Pubkey::from_base58(&token)
                    .map_err(|e| anyhow::anyhow!("Invalid token address: {}", e))?;

                match client.get_contract_info(&token).await {
                    Ok(info) => {
                        println!("📍 Address: {}", info.address);
                        println!("👤 Deployer: {}", info.deployer);
                        println!("📏 Code size: {} bytes", info.code_size);
                        println!("📅 Deployed at slot: {}", info.deployed_at);
                        println!();
                        println!("💡 Query token metadata: molt call {} get_info '[]'", token);
                    }
                    Err(e) => {
                        println!("⚠️  Token contract not found: {}", e);
                        println!("💡 Verify the token address is a deployed contract");
                    }
                }
            }
            TokenCommands::Mint {
                token,
                amount,
                to,
                keypair,
            } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let minter = keypair_mgr.load_keypair(&path)?;
                let recipient = to.unwrap_or_else(|| minter.pubkey().to_base58());
                println!("🪙 Minting {} tokens to {}", amount, recipient);

                let contract_addr = moltchain_core::Pubkey::from_base58(&token)
                    .map_err(|e| anyhow::anyhow!("Invalid token address: {}", e))?;

                let mut data = Vec::new();
                data.extend_from_slice(&amount.to_le_bytes());
                data.extend_from_slice(recipient.as_bytes());

                let signature = client
                    .call_contract(&minter, &contract_addr, "mint".to_string(), data, 0)
                    .await?;
                println!("✅ Tokens minted! Sig: {}", signature);
            }
            TokenCommands::Send {
                token,
                to,
                amount,
                keypair,
            } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let sender = keypair_mgr.load_keypair(&path)?;
                println!("📤 Sending {} tokens to {}", amount, to);

                let contract_addr = moltchain_core::Pubkey::from_base58(&token)
                    .map_err(|e| anyhow::anyhow!("Invalid token address: {}", e))?;

                let mut data = Vec::new();
                data.extend_from_slice(&amount.to_le_bytes());
                data.extend_from_slice(to.as_bytes());

                let signature = client
                    .call_contract(&sender, &contract_addr, "transfer".to_string(), data, 0)
                    .await?;
                println!("✅ Tokens sent! Sig: {}", signature);
            }
            TokenCommands::Balance {
                token,
                address,
                keypair,
            } => {
                let addr = if let Some(a) = address {
                    a
                } else {
                    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                    let kp = keypair_mgr.load_keypair(&path)?;
                    kp.pubkey().to_base58()
                };

                let contract_addr = moltchain_core::Pubkey::from_base58(&token)
                    .map_err(|e| anyhow::anyhow!("Invalid token address: {}", e))?;

                println!("🪙 Token Balance");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!("📍 Token:   {}", token);
                println!("👤 Account: {}", addr);
                println!();

                let data = addr.as_bytes().to_vec();
                // I-2: Error instead of querying with a random keypair
                let query_kp = keypair_mgr
                    .load_keypair(&keypair_mgr.default_keypair_path())
                    .map_err(|_| {
                        anyhow::anyhow!("No wallet configured. Run `molt wallet create` first.")
                    })?;
                match client
                    .call_contract(&query_kp, &contract_addr, "balance_of".to_string(), data, 0)
                    .await
                {
                    Ok(sig) => {
                        println!("📝 Query submitted (sig: {})", sig);
                        println!("💡 Check transaction result for balance value");
                    }
                    Err(e) => {
                        println!("⚠️  Could not query token balance: {}", e);
                        println!("💡 Ensure token contract is deployed and supports balance_of");
                    }
                }
            }
            TokenCommands::List => {
                println!("🪙 Deployed Token Contracts");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                match client.get_all_contracts().await {
                    Ok(contracts) => {
                        if contracts.is_empty() {
                            println!("No token contracts deployed yet");
                        } else {
                            for (i, contract) in contracts.iter().enumerate() {
                                println!("#{} {}", i + 1, contract.address);
                                println!("   Deployer: {}", contract.deployer);
                                println!();
                            }
                            println!("Total: {} contracts", contracts.len());
                            println!();
                            println!("💡 Get token details: molt token info <address>");
                        }
                    }
                    Err(e) => {
                        println!("⚠️  Could not fetch contracts: {}", e);
                    }
                }
            }
        },

        // ====================================================================
        // GOVERNANCE COMMANDS
        // ====================================================================
        Commands::Gov(gov_cmd) => {
            // Resolve DAO contract address from on-chain symbol registry.
            // Falls back to well-known marker [0xDA; 32] if registry unavailable.
            let dao_addr = match client.resolve_symbol("DAO").await {
                Ok(Some(addr)) => addr,
                _ => {
                    eprintln!(
                        "⚠️  DAO contract not found in symbol registry, using well-known address"
                    );
                    moltchain_core::Pubkey([0xDA; 32])
                }
            };

            match gov_cmd {
                GovCommands::Propose {
                    title,
                    description,
                    proposal_type,
                    keypair,
                } => {
                    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                    let proposer = keypair_mgr.load_keypair(&path)?;

                    let ptype = match proposal_type.as_str() {
                        "fast-track" | "fast" => 0u8,
                        "standard" => 1u8,
                        "constitutional" | "const" => 2u8,
                        _ => {
                            println!("⚠️  Invalid proposal type. Use: fast-track, standard, constitutional");
                            return Ok(());
                        }
                    };

                    println!("📜 Creating {} proposal", proposal_type);
                    println!("   Title: {}", title);
                    let desc_preview: String = description.chars().take(80).collect();
                    println!(
                        "   Description: {}{}",
                        desc_preview,
                        if description.len() > desc_preview.len() {
                            "..."
                        } else {
                            ""
                        }
                    );
                    println!("   Proposer: {}", proposer.pubkey().to_base58());
                    println!("   Stake: 1000 MOLT required");
                    println!();

                    // Build governance proposal instruction
                    let mut data = Vec::new();
                    data.push(ptype);
                    data.extend_from_slice(&(title.len() as u32).to_le_bytes());
                    data.extend_from_slice(title.as_bytes());
                    data.extend_from_slice(&(description.len() as u32).to_le_bytes());
                    data.extend_from_slice(description.as_bytes());

                    // MoltyDAO contract address should be well-known

                    let signature = client
                        .call_contract(
                            &proposer,
                            &dao_addr,
                            "create_proposal_typed".to_string(),
                            data,
                            0,
                        )
                        .await?;
                    println!("✅ Proposal created! Sig: {}", signature);
                }
                GovCommands::Vote {
                    proposal_id,
                    vote,
                    keypair,
                } => {
                    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                    let voter = keypair_mgr.load_keypair(&path)?;

                    let vote_value = match vote.to_lowercase().as_str() {
                        "yes" | "y" | "1" => 1u8,
                        "no" | "n" | "0" => 0u8,
                        "abstain" | "a" | "2" => 2u8,
                        _ => {
                            println!("⚠️  Invalid vote. Use: yes, no, abstain");
                            return Ok(());
                        }
                    };

                    println!("🗳️  Voting {} on proposal #{}", vote, proposal_id);
                    println!("   Voter: {}", voter.pubkey().to_base58());

                    let mut data = Vec::new();
                    data.extend_from_slice(&proposal_id.to_le_bytes());
                    data.push(vote_value);

                    let signature = client
                        .call_contract(&voter, &dao_addr, "vote".to_string(), data, 0)
                        .await?;
                    println!("✅ Vote cast! Sig: {}", signature);
                }
                GovCommands::List { all } => {
                    println!(
                        "📜 Governance Proposals {}",
                        if all { "(all)" } else { "(active)" }
                    );
                    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                    println!();

                    let filter = if all { "all" } else { "active" };
                    let data = filter.as_bytes().to_vec();

                    // I-2: Error instead of querying with a random keypair
                    let query_kp = keypair_mgr
                        .load_keypair(&keypair_mgr.default_keypair_path())
                        .map_err(|_| {
                            anyhow::anyhow!("No wallet configured. Run `molt wallet create` first.")
                        })?;
                    match client
                        .call_contract(&query_kp, &dao_addr, "get_proposals".to_string(), data, 0)
                        .await
                    {
                        Ok(sig) => {
                            println!("📝 Query submitted (sig: {})", sig);
                            println!("💡 Check transaction logs for proposal list");
                        }
                        Err(e) => {
                            println!("⚠️  Could not query proposals: {}", e);
                            println!("💡 Ensure the MoltyDAO contract is deployed at the well-known address");
                        }
                    }
                }
                GovCommands::Info { proposal_id } => {
                    println!("📜 Proposal #{}", proposal_id);
                    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                    println!();

                    let data = proposal_id.to_le_bytes().to_vec();

                    // I-2: Error instead of querying with a random keypair
                    let query_kp = keypair_mgr
                        .load_keypair(&keypair_mgr.default_keypair_path())
                        .map_err(|_| {
                            anyhow::anyhow!("No wallet configured. Run `molt wallet create` first.")
                        })?;
                    match client
                        .call_contract(&query_kp, &dao_addr, "get_proposal".to_string(), data, 0)
                        .await
                    {
                        Ok(sig) => {
                            println!("📝 Query submitted (sig: {})", sig);
                            println!("💡 Check transaction logs for proposal details");
                        }
                        Err(e) => {
                            println!("⚠️  Could not query proposal: {}", e);
                            println!("💡 Ensure the MoltyDAO contract is deployed at the well-known address");
                        }
                    }
                }
                GovCommands::Execute {
                    proposal_id,
                    keypair,
                } => {
                    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                    let executor = keypair_mgr.load_keypair(&path)?;

                    println!("⚡ Executing proposal #{}", proposal_id);
                    println!("   Executor: {}", executor.pubkey().to_base58());

                    let mut data = Vec::new();
                    data.extend_from_slice(&proposal_id.to_le_bytes());

                    let signature = client
                        .call_contract(
                            &executor,
                            &dao_addr,
                            "execute_proposal".to_string(),
                            data,
                            0,
                        )
                        .await?;
                    println!("✅ Proposal executed! Sig: {}", signature);
                }
                GovCommands::Veto {
                    proposal_id,
                    keypair,
                } => {
                    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                    let vetoer = keypair_mgr.load_keypair(&path)?;

                    println!("🚫 Vetoing proposal #{}", proposal_id);
                    println!("   Vetoer: {}", vetoer.pubkey().to_base58());

                    let mut data = Vec::new();
                    data.extend_from_slice(&proposal_id.to_le_bytes());

                    let signature = client
                        .call_contract(&vetoer, &dao_addr, "veto_proposal".to_string(), data, 0)
                        .await?;
                    println!("✅ Veto cast! Sig: {}", signature);
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_molt_to_shells_basic() {
        assert_eq!(molt_to_shells(1.0), 1_000_000_000);
        assert_eq!(molt_to_shells(0.5), 500_000_000);
        assert_eq!(molt_to_shells(100.0), 100_000_000_000);
    }

    #[test]
    fn test_molt_to_shells_zero_and_negative() {
        assert_eq!(molt_to_shells(0.0), 0);
        assert_eq!(molt_to_shells(-1.0), 0);
        assert_eq!(molt_to_shells(-0.001), 0);
    }

    #[test]
    fn test_molt_to_shells_fractional_precision() {
        // Exact fractional values
        assert_eq!(molt_to_shells(0.000000001), 1); // 1 shell
        assert_eq!(molt_to_shells(1.123456789), 1_123_456_789);
        assert_eq!(molt_to_shells(0.1), 100_000_000);
        assert_eq!(molt_to_shells(0.01), 10_000_000);
    }

    #[test]
    fn test_molt_to_shells_large_values() {
        // Large values that could cause float precision loss with naive (amount * 1e9) as u64
        assert_eq!(molt_to_shells(1_000_000.0), 1_000_000_000_000_000);
        // Near the u64 overflow boundary → saturates
        assert_eq!(molt_to_shells(f64::MAX), u64::MAX);
    }

    #[test]
    fn test_molt_to_shells_saturating() {
        // Values near the u64 limit should saturate, not overflow
        let huge = (u64::MAX / 1_000_000_000) as f64 + 1.0;
        assert_eq!(molt_to_shells(huge), u64::MAX);
    }
}
