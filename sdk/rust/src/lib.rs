//! # Lichen Rust SDK
//!
//! Official Rust SDK for interacting with Lichen blockchain.
//!
//! ## Features
//!
//! - **Type-safe RPC client** - Interact with validators via JSON-RPC
//! - **Transaction building** - Create and sign transactions
//! - **Keypair management** - Native PQ keypair generation and signing
//! - **Async/await** - Built on Tokio for async operations
//! - **PQ-native wire format** - Matches the core `PqSignature` transaction model
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use lichen_client_sdk::{Client, Keypair};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Connect to validator
//!     let client = Client::new("http://localhost:8899");
//!     
//!     // Generate keypair
//!     let keypair = Keypair::new();
//!     
//!     // Get balance
//!     let balance = client.get_balance(&keypair.pubkey()).await?;
//!     println!("Balance: {} LICN", balance.licn());
//!     
//!     Ok(())
//! }
//! ```

pub mod bountyboard;
pub mod client;
pub mod error;
pub mod keypair;
pub mod lichenid;
pub mod lichenswap;
pub mod restrictions;
pub mod sporepay;
pub mod sporevault;
pub mod thalllend;
pub mod transaction;
pub mod types;

// Re-exports for convenience
pub use bountyboard::{
    ApproveWorkParams, BountyBoardBountyInfo, BountyBoardClient, BountyBoardPlatformStats,
    BountyBoardStats, CreateBountyParams, SubmitWorkParams, BOUNTY_STATUS_CANCELLED,
    BOUNTY_STATUS_COMPLETED, BOUNTY_STATUS_OPEN,
};
pub use client::{Client, ClientBuilder, ReadonlyContractResult};
pub use error::{Error, Result};
pub use keypair::{Address, Keypair, PqPublicKey, PqSignature, Pubkey};
pub use lichenid::{
    estimate_lichenid_name_registration_cost, AddSkillParams, ApproveRecoveryParams,
    AttestSkillParams, BidNameAuctionParams, CreateNameAuctionParams, ExecuteRecoveryParams,
    FinalizeNameAuctionParams, LichenIdAchievement, LichenIdAgentConfig, LichenIdAgentDirectory,
    LichenIdAgentDirectoryEntry, LichenIdAgentDirectoryOptions, LichenIdAvailability,
    LichenIdClient, LichenIdContributions, LichenIdDelegateRecord, LichenIdGivenVouch,
    LichenIdIdentitySummary, LichenIdNameAuction, LichenIdNameResolution, LichenIdProfile,
    LichenIdReceivedVouch, LichenIdReputation, LichenIdReputationSummary, LichenIdSkill,
    LichenIdStats, LichenIdVouches, RegisterIdentityParams, RegisterNameParams,
    RevokeAttestationParams, SetAvailabilityAsParams, SetAvailabilityParams, SetDelegateParams,
    SetEndpointAsParams, SetEndpointParams, SetMetadataAsParams, SetMetadataParams,
    SetRateAsParams, SetRateParams, SetRecoveryGuardiansParams, UpdateAgentTypeAsParams,
    LICHENID_DELEGATE_PERM_AGENT_TYPE, LICHENID_DELEGATE_PERM_NAMING,
    LICHENID_DELEGATE_PERM_PROFILE, LICHENID_DELEGATE_PERM_SKILLS,
};
pub use lichenswap::{
    AddLiquidityParams, CreatePoolParams, LichenSwapClient, LichenSwapPoolInfo,
    LichenSwapProtocolFees, LichenSwapStats, LichenSwapSwapStats, LichenSwapTwapCumulatives,
    LichenSwapVolumeTotals, SwapParams, SwapWithDeadlineParams,
};
pub use restrictions::{
    BridgeRouteRestrictionParams, BridgeRouteRestrictionStatus, CodeHashRestrictionParams,
    CodeHashRestrictionStatus, ContractLifecycleRestrictionStatus, ContractRestrictionParams,
    EffectiveRestrictionRecord, ExtendRestrictionParams, GetRestrictionResponse,
    LiftRestrictionParams, MovementRestrictionParams, MovementRestrictionStatus,
    RestrictAccountAssetParams, RestrictAccountParams, RestrictCommonParams, RestrictionAddress,
    RestrictionAsset, RestrictionBuilderBaseParams, RestrictionBuilderInstruction,
    RestrictionGovernanceClient, RestrictionLiftReasonInput, RestrictionListParams,
    RestrictionListResponse, RestrictionModeDetails, RestrictionModeInput, RestrictionReasonInput,
    RestrictionRecord, RestrictionStringOrU64, RestrictionTargetDetails, RestrictionTargetInput,
    RestrictionTargetStatus, ResumeBridgeRouteParams, ResumeContractParams,
    SetFrozenAssetAmountParams, TransferRestrictionParams, TransferRestrictionStatus,
    UnbanCodeHashParams, UnrestrictAccountAssetParams, UnrestrictAccountParams,
    UnsignedRestrictionGovernanceTx,
};
pub use sporepay::{
    CreateStreamParams, CreateStreamWithCliffParams, SporePayClient, SporePayStats, SporePayStream,
    SporePayStreamInfo, TransferStreamParams, WithdrawFromStreamParams,
};
pub use sporevault::{
    SporeVaultClient, SporeVaultStats, SporeVaultStrategyInfo, SporeVaultUserPosition,
    SporeVaultVaultStats,
};
pub use thalllend::{
    LiquidateParams, ThallLendAccountInfo, ThallLendClient, ThallLendInterestRate,
    ThallLendProtocolStats, ThallLendStats,
};
pub use transaction::TransactionBuilder;
pub use types::{Balance, Block, NetworkInfo, Transaction};

// Re-export core types
pub use lichen_core::{
    Account, ContractInstruction, Hash, Instruction, Message, BASE_FEE, CONTRACT_PROGRAM_ID,
    SYSTEM_PROGRAM_ID,
};

/// SDK version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
