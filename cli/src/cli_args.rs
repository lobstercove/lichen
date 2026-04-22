use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Recognized contract template categories
#[derive(Clone, Debug, ValueEnum)]
pub(super) enum ContractTemplate {
    Token,
    Nft,
    Defi,
    Dex,
    Governance,
    Wrapped,
    Bridge,
    Oracle,
    Lending,
    Marketplace,
    Auction,
    Identity,
    Launchpad,
    Vault,
    Payments,
}

impl std::fmt::Display for ContractTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = format!("{:?}", self);
        write!(f, "{}", value.to_lowercase())
    }
}

/// Code generation target language
#[derive(Clone, Debug, ValueEnum)]
pub(super) enum CodegenLang {
    Typescript,
    Python,
}

/// Lichen CLI - Blockchain for autonomous agents
#[derive(Parser)]
#[command(name = "lichen")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Lichen CLI - Economic freedom for agents 🦞⚡")]
#[command(
    long_about = "Lichen CLI — command-line interface for Lichen, a Layer 1 blockchain\n\
    built by agents, for agents. Tendermint BFT consensus, ~800ms blocks,\n\
    WASM smart contracts, ML-DSA-65 signing, ZK privacy (Plonky3 STARK).\n\n\
    Native token: LICN (1 LICN = 1,000,000,000 spores)\n\
    Run 'lichen fees' for current fee schedule\n\n\
    Mainnet RPC: https://rpc.lichen.network\n\
    Testnet RPC: https://testnet-rpc.lichen.network\n\
    Explorer:    https://explorer.lichen.network\n\
    Docs:        https://developers.lichen.network"
)]
#[command(after_help = "EXAMPLES:\n\
    lichen identity new                              Create a new keypair\n\
    lichen airdrop 100                               Get 100 testnet LICN\n\
    lichen balance                                   Check your balance\n\
    lichen transfer <ADDRESS> 10.5                   Send 10.5 LICN\n\
    lichen deploy token.wasm --symbol TKN            Deploy a contract\n\
    lichen call <ADDR> get_info                      Call a contract\n\
    lichen status                                    Chain status dashboard\n\
    lichen --output json status                      JSON output (agent-friendly)\n\
    lichen --rpc-url https://rpc.lichen.network balance")]
pub(super) struct Cli {
    /// RPC server URL
    #[arg(
        long,
        global = true,
        default_value = "http://localhost:8899",
        env = "LICHEN_RPC_URL"
    )]
    pub(super) rpc_url: String,

    /// Output format: human (default) or json (machine-readable for AI agents)
    #[arg(long, global = true, default_value = "human", env = "LICHEN_OUTPUT")]
    pub(super) output: OutputFormat,

    #[command(subcommand)]
    pub(super) command: Commands,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum OutputFormat {
    Human,
    Json,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "human" | "text" | "h" => Ok(OutputFormat::Human),
            "json" | "j" => Ok(OutputFormat::Json),
            _ => Err(format!(
                "Unknown output format '{}'. Use 'human' or 'json'.",
                s
            )),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Human => write!(f, "human"),
            OutputFormat::Json => write!(f, "json"),
        }
    }
}

#[derive(Subcommand)]
pub(super) enum Commands {
    /// Identity management
    #[command(subcommand)]
    Identity(IdentityCommands),

    /// Wallet management (multi-wallet support)
    #[command(subcommand)]
    Wallet(WalletCommands),

    /// [DEPRECATED] Use 'identity new' instead. Creates a new keypair.
    #[command(hide = true)]
    Init {
        /// Output file path
        #[arg(short, long, id = "init_output_path")]
        output: Option<PathBuf>,
    },

    /// Check account balance
    Balance {
        /// Account address (Base58 or hex)
        address: Option<String>,

        /// Keypair file to check balance for (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Transfer LICN to another account
    Transfer {
        /// Destination address (Base58)
        to: String,

        /// Amount in LICN
        amount: f64,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Request test tokens from faucet
    Airdrop {
        /// Amount in LICN to request (default: 100)
        #[arg(default_value = "100.0")]
        amount: f64,

        /// Account to receive tokens (defaults to your identity)
        #[arg(short, long)]
        pubkey: Option<String>,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Deploy a smart contract
    Deploy {
        /// WASM contract file path
        contract: PathBuf,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,

        /// Register symbol in the symbol registry (e.g. VYRN)
        #[arg(long)]
        symbol: Option<String>,

        /// Display name for the contract (e.g. "VYRN Token")
        #[arg(long)]
        name: Option<String>,

        /// Contract template category
        #[arg(long, value_enum)]
        template: Option<ContractTemplate>,

        /// Token decimals (e.g. 9 for LICN-style tokens)
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

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
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

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
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

    /// Get total burned LICN
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

    /// Show version and build information
    Version,

    /// CLI configuration management
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Symbol registry operations (lookup, list)
    #[command(subcommand)]
    Symbol(SymbolCommands),

    /// Transaction lookup by signature
    Tx {
        /// Transaction signature (hex)
        signature: String,
    },

    /// NFT operations (collections, minting, transfers)
    #[command(subcommand)]
    Nft(NftCommands),

    /// DeFi protocol stats (DEX, lending, swaps)
    #[command(subcommand)]
    Defi(DefiCommands),

    /// Supply and economics information
    Supply,

    /// Fee configuration and calculator
    Fees,

    /// Epoch information
    Epoch,

    /// Show available WASM host functions for contract developers
    HostFunctions,
}

#[derive(Subcommand)]
pub(super) enum NetworkCommands {
    /// Show network status
    Status,

    /// List connected peers
    Peers,

    /// Show network information
    Info,
}

#[derive(Subcommand)]
pub(super) enum ValidatorCommands {
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
pub(super) enum StakeCommands {
    /// Stake LICN to become a validator
    Add {
        /// Amount in spores to stake
        amount: u64,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Unstake LICN
    Remove {
        /// Amount in spores to unstake
        amount: u64,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Show staking status
    Status {
        /// Account address (defaults to your identity)
        #[arg(short, long)]
        address: Option<String>,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Show staking rewards
    Rewards {
        /// Account address (defaults to your identity)
        #[arg(short, long)]
        address: Option<String>,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(super) enum WalletCommands {
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
pub(super) enum AccountCommands {
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
pub(super) enum ContractCommands {
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

        /// Template category
        #[arg(long, value_enum)]
        template: Option<ContractTemplate>,

        /// Decimals (e.g. 9)
        #[arg(long)]
        decimals: Option<u8>,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Generate a typed client SDK from a contract ABI
    GenerateClient {
        /// Path to abi.json file
        #[arg(long, group = "source")]
        abi: Option<PathBuf>,

        /// Contract address (fetches ABI via RPC)
        #[arg(long, group = "source")]
        address: Option<String>,

        /// Target language
        #[arg(long, value_enum)]
        lang: CodegenLang,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
pub(super) enum IdentityCommands {
    /// Create a new identity
    New {
        /// Output file path (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long, id = "identity_output_path")]
        output: Option<PathBuf>,
    },

    /// Show your identity
    Show {
        /// Keypair file path (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Decrypt and export a keypair file (validator or wallet key)
    ///
    /// Reads an encrypted keypair file, decrypts it using LICHEN_KEYPAIR_PASSWORD,
    /// and shows the public key and address. Use --reveal-seed to also display the
    /// private seed (WARNING: handle with extreme care).
    Export {
        /// Keypair file path (e.g. /var/lib/lichen/testnet/validator-keypair.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,

        /// Also print the decrypted 32-byte private seed (hex). Handle with care!
        #[arg(long)]
        reveal_seed: bool,
    },
}

#[derive(Subcommand)]
pub(super) enum TokenCommands {
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

        /// Initial supply in whole tokens minted to the creator after initialization
        #[arg(long)]
        initial_supply: Option<u64>,

        /// Project website URL
        #[arg(long)]
        website: Option<String>,

        /// Token logo URL
        #[arg(long)]
        logo_url: Option<String>,

        /// Short token description
        #[arg(long)]
        description: Option<String>,

        /// Twitter/X profile URL
        #[arg(long)]
        twitter: Option<String>,

        /// Telegram group/channel URL
        #[arg(long)]
        telegram: Option<String>,

        /// Discord invite URL
        #[arg(long)]
        discord: Option<String>,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
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

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
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

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
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

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// List all registered tokens
    List,
}

#[derive(Subcommand)]
pub(super) enum GovCommands {
    /// Create a governance proposal
    Propose {
        /// Proposal title
        title: String,

        /// Proposal description
        description: String,

        /// Proposal type: fast-track, standard, constitutional
        #[arg(short = 't', long, default_value = "standard")]
        proposal_type: String,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Vote on a proposal
    Vote {
        /// Proposal ID
        proposal_id: u64,

        /// Vote: yes/no/abstain
        vote: String,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
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

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// Veto a proposal during time-lock
    Veto {
        /// Proposal ID
        proposal_id: u64,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(super) enum ConfigCommands {
    /// Show current CLI configuration
    Show,

    /// Set RPC endpoint URL
    Set {
        /// Configuration key (rpc_url, ws_url, keypair)
        key: String,

        /// Value to set
        value: String,
    },

    /// Reset configuration to defaults
    Reset,
}

#[derive(Subcommand)]
pub(super) enum SymbolCommands {
    /// Look up a symbol in the registry (e.g. LICN, DEX, DAO)
    Lookup {
        /// Symbol to look up (case-insensitive)
        symbol: String,
    },

    /// List all symbols in the registry
    List,

    /// Look up a contract address in the registry
    ByAddress {
        /// Contract address (Base58)
        address: String,
    },
}

#[derive(Subcommand)]
pub(super) enum NftCommands {
    /// List NFTs owned by an account
    List {
        /// Owner address (defaults to your identity)
        #[arg(short, long)]
        owner: Option<String>,

        /// Keypair file (default: ~/.lichen/keypairs/id.json)
        #[arg(short, long)]
        keypair: Option<PathBuf>,
    },

    /// List NFTs in a collection
    Collection {
        /// Collection address (Base58)
        address: String,
    },

    /// Show NFT marketplace listings
    Marketplace {
        /// Number of listings to show (default: 20)
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
}

#[derive(Subcommand)]
pub(super) enum DefiCommands {
    /// Show DEX overview (SporeSwap core stats)
    Dex,

    /// Show AMM pool stats
    Amm,

    /// Show lending protocol stats (ThallLend)
    Lending,

    /// Show all DeFi protocol stats
    Overview,
}
