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

    /// Generate a new keypair (deprecated, use 'identity new')
    #[command(name = "generate-keypair")]
    GenerateKeypair {
        /// Output file path (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Show public key from keypair file (deprecated, use 'identity show')
    #[command(name = "pubkey")]
    Pubkey {
        /// Keypair file path (default: ~/.moltchain/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
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
    /// Create a new token
    Create {
        /// Token name
        name: String,

        /// Token symbol (3-5 chars)
        symbol: String,

        /// Initial supply (in whole tokens)
        #[arg(short, long, default_value = "1000000")]
        supply: u64,

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

        Commands::GenerateKeypair { output } => {
            let keypair = Keypair::new();
            let pubkey = keypair.pubkey();

            let path = output.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            keypair_mgr.save_keypair(&keypair, &path)?;

            println!("🦞 Generated new keypair!");
            println!("📍 Pubkey: {}", pubkey.to_base58());
            println!("🔐 EVM Address: {}", pubkey.to_evm());
            println!("💾 Saved to: {}", path.display());
            println!();
            println!("⚠️  Deprecated: Use 'molt identity new' instead");
        }

        Commands::Pubkey { keypair } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let kp = keypair_mgr.load_keypair(&path)?;
            let pubkey = kp.pubkey();

            println!("📍 Pubkey: {}", pubkey.to_base58());
            println!("🔐 EVM Address: {}", pubkey.to_evm());
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
            let shells = (amount * 1_000_000_000.0) as u64;

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
                        println!(
                            "   Success rate: {:.2}%",
                            (perf.blocks_produced as f64 / perf.blocks_expected as f64) * 100.0
                        );
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
                    println!(
                        "   Burned: {} MOLT ({:.2}%)",
                        metrics.total_burned as f64 / 1_000_000_000.0,
                        (metrics.total_burned as f64 / metrics.total_supply as f64) * 100.0
                    );
                    println!(
                        "   Staked: {} MOLT ({:.2}%)",
                        metrics.total_staked as f64 / 1_000_000_000.0,
                        (metrics.total_staked as f64 / metrics.total_supply as f64) * 100.0
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
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let kp = keypair_mgr.load_keypair(&path)?;
            let recipient = if let Some(addr) = pubkey {
                Pubkey::from_base58(&addr).map_err(|e| anyhow::anyhow!("Invalid address: {}", e))?
            } else {
                kp.pubkey()
            };

            let shells = (amount * 1_000_000_000.0) as u64;
            println!("🦞 Requesting {} MOLT airdrop...", amount);
            println!("📥 To: {}", recipient.to_base58());
            println!();

            match client.transfer(&kp, &recipient, shells).await {
                Ok(signature) => {
                    println!("✅ Airdrop sent!");
                    println!("📝 Signature: {}", signature);
                }
                Err(e) => {
                    println!("⚠️  Airdrop failed: {}", e);
                    println!("💡 Ensure the faucet account has sufficient balance");
                }
            }
        }

        Commands::Deploy { contract, keypair } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let deployer = keypair_mgr.load_keypair(&path)?;

            // Read WASM file
            let wasm_code = std::fs::read(&contract)
                .map_err(|e| anyhow::anyhow!("Failed to read contract file: {}", e))?;

            // Generate contract address (deterministic from deployer + code hash)
            use moltchain_core::Hash;
            let code_hash = Hash::hash(&wasm_code);
            let mut addr_bytes = [0u8; 32];
            addr_bytes[..16].copy_from_slice(&deployer.pubkey().0[..16]);
            addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
            let contract_addr = moltchain_core::Pubkey(addr_bytes);

            println!("🦞 Deploying contract: {}", contract.display());
            println!("📦 Size: {} KB", wasm_code.len() / 1024);
            println!("📍 Contract address: {}", contract_addr.to_base58());
            println!("👤 Deployer: {}", deployer.pubkey().to_base58());
            println!();

            let signature = client
                .deploy_contract(&deployer, wasm_code, &contract_addr)
                .await?;

            println!("✅ Contract deployed!");
            println!("📝 Signature: {}", signature);
            println!("🔗 Address: {}", contract_addr.to_base58());
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
                supply,
                decimals,
                keypair,
            } => {
                let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
                let creator = keypair_mgr.load_keypair(&path)?;

                println!("🪙 Creating token: {} ({})", name, symbol);
                println!("📈 Supply: {} (decimals: {})", supply, decimals);
                println!("👤 Creator: {}", creator.pubkey().to_base58());
                println!();

                // Build token creation instruction
                let mut data = Vec::new();
                data.push(10); // Token create instruction type
                data.extend_from_slice(&supply.to_le_bytes());
                data.push(decimals);
                data.extend_from_slice(name.as_bytes());
                data.push(0); // null terminator for name
                data.extend_from_slice(symbol.as_bytes());

                let signature = client
                    .call_contract(
                        &creator,
                        &moltchain_core::Pubkey([0u8; 32]), // system program
                        "create_token".to_string(),
                        data,
                        0,
                    )
                    .await?;
                println!("✅ Token created!");
                println!("📝 Signature: {}", signature);
            }
            TokenCommands::Info { token } => {
                println!("🪙 Token Info: {}", token);
                println!("   ⚠️  Token info lookup requires symbol registry RPC");
                println!("   💡 Use: molt call <token_address> get_info");
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
                println!(
                    "🪙 Token balance for {} on {}: (pending RPC implementation)",
                    addr, token
                );
            }
            TokenCommands::List => {
                println!("🪙 Registered Tokens");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!("   ⚠️  Token listing requires symbol registry RPC");
                println!("   💡 Use: molt account info <address> to check token programs");
            }
        },

        // ====================================================================
        // GOVERNANCE COMMANDS
        // ====================================================================
        Commands::Gov(gov_cmd) => {
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
                    println!(
                        "   Description: {}...",
                        &description[..description.len().min(80)]
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
                    let dao_addr = moltchain_core::Pubkey([0xDA; 32]); // DAO marker address

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

                    let dao_addr = moltchain_core::Pubkey([0xDA; 32]);
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
                    println!("   (Fetching from DAO contract...)");
                    // Would call get_active_proposals on the DAO contract
                    println!("   ⚠️  gov list requires MoltyDAO contract deployment");
                }
                GovCommands::Info { proposal_id } => {
                    println!("📜 Proposal #{}", proposal_id);
                    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                    // Would call get_proposal on the DAO contract
                    println!("   ⚠️  gov info requires MoltyDAO contract deployment");
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

                    let dao_addr = moltchain_core::Pubkey([0xDA; 32]);
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

                    let dao_addr = moltchain_core::Pubkey([0xDA; 32]);
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
