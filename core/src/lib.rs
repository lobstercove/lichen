// MoltChain Core Library
// Week 1-2: Basic blockchain data structures + transaction processing
// Week 3: Consensus, RPC, CLI
// Week 4-5: Smart Contracts & WASM Runtime

// Wasmer 4.x JIT references __rust_probestack on x86_64, removed in Rust 1.84+.
// Provide a no-op stub so the linker resolves the symbol.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn __rust_probestack() {}

pub mod account;
pub mod block;
pub mod consensus;
pub mod contract;
pub mod contract_instruction;
pub mod event_stream;
pub mod evm;
pub mod genesis;
pub mod hash;
pub mod marketplace;
pub mod mempool;
pub mod multisig; // Multi-signature wallet support
pub mod network;
pub mod nft;
pub mod processor;
pub mod reefstake; // Liquid staking protocol
pub mod state;
pub mod transaction;
#[cfg(feature = "zk")]
pub mod zk;

// Re-exports
pub use account::{Account, Keypair, Pubkey};
pub use block::{
    compute_bft_timestamp, compute_validators_hash, Block, BlockHeader, CommitSignature,
    MAX_BLOCK_SIZE, MAX_CONTRACT_CODE, MAX_TX_PER_BLOCK,
};
pub use consensus::{
    compute_block_reward, compute_epoch_mint, consensus_oracle_price_from_state, epoch_start_slot,
    inflation_rate_bps, is_epoch_boundary, molt_price_from_state,
    read_consensus_oracle_price_from_state, read_molt_price_feed_from_state, slot_to_epoch,
    BootstrapStatus, EpochInfo, FinalityTracker, ForkChoice, PendingValidatorChange, Precommit,
    Prevote, PriceOracle, Proposal, RewardAdjustmentInfo, RewardConfig, RoundStep,
    SlashingEvidence, SlashingOffense, SlashingTracker, StakeInfo, StakePool, StakingStats,
    StateOracle, ValidatorChangeType, ValidatorInfo, ValidatorSet, Vote, VoteAggregator,
    VoteAuthority, BOOTSTRAP_GRANT_AMOUNT, DOWNTIME_FORGIVENESS_SLOTS, DOWNTIME_SUSPENSION_SLOTS,
    DOWNTIME_TIER2_SLASH_BPS, FINALITY_DEPTH, GENESIS_SUPPLY_SHELLS, INFLATION_DECAY_RATE_BPS,
    INITIAL_INFLATION_RATE_BPS, MAX_BOOTSTRAP_SLOTS, MAX_BOOTSTRAP_VALIDATORS,
    MIGRATION_COOLDOWN_SLOTS, MIN_VALIDATOR_STAKE, PENALTY_REPAYMENT_BOOST_SLOTS,
    PERFORMANCE_BONUS_BPS, SLOTS_PER_EPOCH, SLOTS_PER_YEAR, TERMINAL_INFLATION_RATE_BPS,
    UPTIME_BONUS_THRESHOLD_BPS,
};
pub use contract::{
    decode_program_call_activity, encode_program_call_activity, AbiError, AbiEvent, AbiEventField,
    AbiFunction, AbiParam, AbiReturn, AbiType, ContractAbi, ContractAccount, ContractContext,
    ContractResult, ContractRuntime, PendingUpgrade, ProgramCallActivity,
    DEFAULT_WASM_MEMORY_PAGES, MAX_WASM_MEMORY_PAGES, WASM_CU_DIVISOR,
};
pub use contract_instruction::ContractInstruction;
pub use evm::{
    decode_evm_transaction, evm_tx_hash, execute_evm_transaction, shells_to_u256,
    simulate_evm_call, supported_precompiles, topics_match, u256_is_multiple_of_shell,
    u256_to_shells, EvmAccount, EvmExecutionResult, EvmLog, EvmLogEntry, EvmReceipt,
    EvmStateChange, EvmStateChanges, EvmTx, EvmTxRecord, EVM_PROGRAM_ID, MOLTCHAIN_CHAIN_ID,
    PRECOMPILE_BLAKE2F, PRECOMPILE_BN256_ADD, PRECOMPILE_BN256_MUL, PRECOMPILE_BN256_PAIRING,
    PRECOMPILE_ECRECOVER, PRECOMPILE_IDENTITY, PRECOMPILE_MODEXP, PRECOMPILE_RIPEMD160,
    PRECOMPILE_SHA256,
};
pub use genesis::{
    ConsensusParams, FeatureFlags, GenesisAccount, GenesisConfig, GenesisValidator, NetworkConfig,
};
pub use hash::Hash;
pub use marketplace::{
    decode_market_activity, encode_market_activity, MarketActivity, MarketActivityKind,
};
pub use mempool::Mempool;
pub use multisig::{DistributionWallet, GenesisWallet, MultiSigConfig, GENESIS_DISTRIBUTION};
pub use nft::{
    CollectionState, CreateCollectionData, MintNftData, NftActivity, NftActivityKind, TokenState,
};
pub use processor::{
    compute_graduated_rent, compute_stake_weighted_median, compute_units_for_system_ix,
    compute_units_for_tx, get_trust_tier, FeeConfig, NonceState, OracleAttestation,
    OracleConsensusPrice, SimulationResult, TxProcessor, TxResult, BASE_FEE, CONTRACT_DEPLOY_FEE,
    CONTRACT_PROGRAM_ID, CONTRACT_UPGRADE_FEE, CU_DEPLOY_CONTRACT, CU_MINT_NFT, CU_NONCE,
    CU_ORACLE_ATTESTATION, CU_STAKE, CU_TRANSFER, CU_ZK_SHIELD, CU_ZK_TRANSFER,
    DORMANCY_THRESHOLD_EPOCHS, EVM_SENTINEL_BLOCKHASH, GOV_PARAM_BASE_FEE, GOV_PARAM_EPOCH_SLOTS,
    GOV_PARAM_FEE_BURN_PERCENT, GOV_PARAM_FEE_COMMUNITY_PERCENT, GOV_PARAM_FEE_PRODUCER_PERCENT,
    GOV_PARAM_FEE_TREASURY_PERCENT, GOV_PARAM_FEE_VOTERS_PERCENT, GOV_PARAM_MIN_VALIDATOR_STAKE,
    MAX_TX_AGE_BLOCKS, NFT_COLLECTION_FEE, NFT_MINT_FEE, NONCE_ACCOUNT_MARKER,
    NONCE_ACCOUNT_MIN_BALANCE, ORACLE_ASSET_MAX_LEN, ORACLE_ASSET_MIN_LEN, ORACLE_STALENESS_SLOTS,
    RENT_FREE_BYTES, SYSTEM_PROGRAM_ID,
};
pub use reefstake::{
    ReefStakePool, StMoltToken, StakingPosition, UnstakeRequest, REEFSTAKE_BLOCK_SHARE_BPS,
};
pub use state::AccountProof;
pub use state::CheckpointMeta;
pub use state::MerkleProof;
pub use state::StateBatch;
pub use state::StateStore;
pub use state::SymbolRegistryEntry;
pub use transaction::{
    Instruction, Message, Transaction, TransactionType, DEFAULT_COMPUTE_BUDGET, MAX_COMPUTE_BUDGET,
    TX_WIRE_MAGIC, TX_WIRE_VERSION,
};
