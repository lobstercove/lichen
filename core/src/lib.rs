// MoltChain Core Library
// Week 1-2: Basic blockchain data structures + transaction processing
// Week 3: Consensus, RPC, CLI
// Week 4-5: Smart Contracts & WASM Runtime

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
pub use block::{Block, BlockHeader, MAX_BLOCK_SIZE, MAX_CONTRACT_CODE, MAX_TX_PER_BLOCK};
pub use consensus::{
    epoch_start_slot, is_epoch_boundary, molt_price_from_state, read_molt_price_feed_from_state,
    slot_to_epoch, BootstrapStatus, EpochInfo, FinalityTracker, ForkChoice, PriceOracle,
    RewardAdjustmentInfo, RewardConfig, SlashingEvidence, SlashingOffense, SlashingTracker,
    StakeInfo, StakePool, StakingStats, StateOracle, ValidatorInfo, ValidatorSet, Vote,
    VoteAggregator, VoteAuthority, BLOCK_REWARD, BOOTSTRAP_GRANT_AMOUNT,
    DOWNTIME_FORGIVENESS_SLOTS, DOWNTIME_SUSPENSION_SLOTS, DOWNTIME_TIER2_SLASH_BPS,
    FINALITY_DEPTH, HEARTBEAT_BLOCK_REWARD, MAX_BOOTSTRAP_SLOTS, MAX_BOOTSTRAP_VALIDATORS,
    MIGRATION_COOLDOWN_SLOTS, MIN_VALIDATOR_STAKE, PENALTY_REPAYMENT_BOOST_SLOTS,
    PERFORMANCE_BONUS_BPS, SLOTS_PER_EPOCH, TRANSACTION_BLOCK_REWARD, UPTIME_BONUS_THRESHOLD_BPS,
};
pub use contract::{
    decode_program_call_activity, encode_program_call_activity, AbiError, AbiEvent, AbiEventField,
    AbiFunction, AbiParam, AbiReturn, AbiType, ContractAbi, ContractAccount, ContractContext,
    ContractResult, ContractRuntime, ProgramCallActivity,
};
pub use contract_instruction::ContractInstruction;
pub use evm::{
    decode_evm_transaction, evm_tx_hash, execute_evm_transaction, shells_to_u256,
    simulate_evm_call, u256_is_multiple_of_shell, u256_to_shells, EvmAccount, EvmExecutionResult,
    EvmReceipt, EvmStateChange, EvmStateChanges, EvmTx, EvmTxRecord, EVM_PROGRAM_ID,
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
    get_trust_tier, FeeConfig, SimulationResult, TxProcessor, TxResult, BASE_FEE,
    CONTRACT_DEPLOY_FEE, CONTRACT_PROGRAM_ID, CONTRACT_UPGRADE_FEE, EVM_SENTINEL_BLOCKHASH,
    NFT_COLLECTION_FEE, NFT_MINT_FEE, SYSTEM_PROGRAM_ID,
};
pub use reefstake::{
    ReefStakePool, StMoltToken, StakingPosition, UnstakeRequest, REEFSTAKE_BLOCK_SHARE_BPS,
};
pub use state::CheckpointMeta;
pub use state::StateBatch;
pub use state::StateStore;
pub use state::SymbolRegistryEntry;
pub use transaction::{Instruction, Message, Transaction};
