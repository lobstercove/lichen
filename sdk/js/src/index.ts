// Lichen JavaScript/TypeScript SDK
// Official SDK for interacting with Lichen blockchain

export { PublicKey } from './publickey.js';
export { PublicKey as Address } from './publickey.js';
export { Keypair } from './keypair.js';
export { Connection } from './connection.js';
export {
  LichenIdClient,
  LICHEN_ID_DELEGATE_PERMISSIONS,
  estimateLichenIdNameRegistrationCost,
} from './lichenid.js';
export { ThallLendClient } from './thalllend.js';
export { LichenSwapClient } from './lichenswap.js';
export { SporePayClient } from './sporepay.js';
export { SporePumpClient, SPOREPUMP_CREATION_FEE } from './sporepump.js';
export { SporeVaultClient } from './sporevault.js';
export { BountyBoardClient } from './bountyboard.js';
export { ComputeMarketClient } from './compute_market.js';
export {
  BRIDGE_ASSETS,
  BRIDGE_CHAINS,
  RestrictionGovernanceClient,
} from './restrictions.js';
export {
  ML_DSA_65_PUBLIC_KEY_BYTES,
  ML_DSA_65_SIGNATURE_BYTES,
  PQ_SCHEME_ML_DSA_65,
  PqPublicKey,
  PqSignature,
} from './pq.js';
export {
  Transaction,
  TransactionBuilder,
  Instruction,
  Message,
} from './transaction.js';

export type {
  Balance,
  Account,
  AccountTxCount,
  Block,
  Validator,
  NetworkInfo,
  ChainStatus,
  Metrics,
  ProofStep,
  ContractListOptions,
  ContractListResponse,
  ContractSummary,
  DeployContractResult,
  ReadonlyContractResult,
  TransactionHistoryResponse,
  TransactionProof,
} from './connection.js';
export type {
  AddSkillParams,
  ApproveRecoveryParams,
  AttestSkillParams,
  BidNameAuctionParams,
  LichenIdAgentDirectory,
  LichenIdAgentDirectoryEntry,
  LichenIdAgentDirectoryOptions,
  LichenIdAvailabilityInput,
  LichenIdAvailabilityStatus,
  LichenIdDelegatePermissionKey,
  LichenIdDelegateRecord,
  LichenIdGivenVouch,
  LichenIdMetadata,
  LichenIdNameAuction,
  LichenIdNameResolution,
  LichenIdProfile,
  LichenIdReputation,
  LichenIdReceivedVouch,
  LichenIdSkill,
  LichenIdStats,
  LichenIdVouches,
  CreateNameAuctionParams,
  ExecuteRecoveryParams,
  FinalizeNameAuctionParams,
  RegisterIdentityParams,
  RegisterNameParams,
  RevokeAttestationParams,
  SetAvailabilityParams,
  SetAvailabilityAsParams,
  SetDelegateParams,
  SetEndpointParams,
  SetEndpointAsParams,
  SetMetadataParams,
  SetMetadataAsParams,
  SetRateParams,
  SetRateAsParams,
  SetRecoveryGuardiansParams,
  UpdateAgentTypeAsParams,
} from './lichenid.js';
export type {
  LiquidateParams,
  ThallLendAccountInfo,
  ThallLendInterestRate,
  ThallLendMarketStatus,
  ThallLendProtocolStats,
  ThallLendRateModel,
  ThallLendStats,
} from './thalllend.js';
export type {
  AddLiquidityParams,
  CreatePoolParams,
  LichenSwapPoolInfo,
  LichenSwapProtocolFees,
  LichenSwapSwapStats,
  LichenSwapTwapCumulatives,
  LichenSwapVolumeTotals,
  SwapParams,
  SwapWithDeadlineParams,
} from './lichenswap.js';
export type {
  CreateStreamParams,
  CreateStreamWithCliffParams,
  SporePayStats,
  SporePayStream,
  SporePayStreamIdPage,
  SporePayStreamInfo,
  TransferStreamParams,
  WithdrawFromStreamParams,
} from './sporepay.js';
export type {
  CreateSporePumpTokenParams,
  SporePumpCustodyStatus,
  SporePumpGraduationConfig,
  SporePumpGraduationInfo,
  SporePumpGraduationStatus,
  SporePumpPlatformStats,
  SporePumpTokenInfo,
  SporePumpTokenMetadata,
} from './sporepump.js';
export type {
  SporeVaultStats,
  SporeVaultStatus,
  SporeVaultStrategyInfo,
  SporeVaultUserPosition,
  SporeVaultVaultStats,
} from './sporevault.js';
export type {
  ApproveWorkParams,
  BountyBoardAccountingHealth,
  BountyBoardAdminTransition,
  BountyBoardAccountingMigrationStatus,
  BountyBoardBountyInfo,
  BountyBoardPlatformStats,
  BountyBoardSubmission,
  BountyBoardStats,
  BountyBoardTerms,
  CreateBountyParams,
  SubmitWorkParams,
} from './bountyboard.js';
export type {
  ComputeMarketAccountingHealth,
  ComputeMarketAccountingMigrationStatus,
  ComputeMarketAgentControls,
  ComputeMarketAgentPolicy,
  ComputeMarketJobInfo,
  ComputeMarketJobTiming,
  ComputeMarketPlatformStats,
  ComputeMarketProviderCapacity,
  ComputeMarketProviderInfo,
  SubmitAgentComputeJobParams,
  SubmitComputeJobParams,
} from './compute_market.js';
export type {
  AccountAssetRestrictionTarget,
  AccountRestrictionTarget,
  AddressInput,
  AssetRestrictionTarget,
  BridgeAsset,
  BridgeChain,
  BridgeRouteRestrictionParams,
  BridgeRouteRestrictionStatus,
  BridgeRouteRestrictionTarget,
  CodeHashRestrictionParams,
  CodeHashRestrictionStatus,
  CodeHashRestrictionTarget,
  ContractLifecycleRestrictionStatus,
  ContractRestrictionParams,
  ContractRestrictionTarget,
  EffectiveRestrictionRecord,
  ExtendRestrictionParams,
  GetRestrictionResponse,
  LiftRestrictionParams,
  MovementRestrictionParams,
  MovementRestrictionStatus,
  ProtocolModuleRestrictionTarget,
  RestrictAccountAssetParams,
  RestrictAccountParams,
  RestrictionAssetInput,
  RestrictionBuilderBaseParams,
  RestrictionBuilderInstruction,
  RestrictionLiftReason,
  RestrictionLiftReasonInput,
  RestrictionListParams,
  RestrictionListResponse,
  RestrictionMode,
  RestrictionModeDetails,
  RestrictionModeInput,
  RestrictionReason,
  RestrictionReasonInput,
  RestrictionRecord,
  RestrictionTargetDetails,
  RestrictionTargetInput,
  RestrictionTargetStatus,
  RestrictionTargetType,
  ResumeBridgeRouteParams,
  ResumeContractParams,
  SetFrozenAssetAmountParams,
  TransferRestrictionParams,
  TransferRestrictionStatus,
  UnbanCodeHashParams,
  UnrestrictAccountAssetParams,
  UnrestrictAccountParams,
  UnsignedRestrictionGovernanceTx,
} from './restrictions.js';
export {
  BOUNTY_STATUS_OPEN,
  BOUNTY_STATUS_COMPLETED,
  BOUNTY_STATUS_CANCELLED,
} from './bountyboard.js';
export {
  COMPUTE_JOB_PENDING,
  COMPUTE_JOB_CLAIMED,
  COMPUTE_JOB_COMPLETED,
  COMPUTE_JOB_DISPUTED,
  COMPUTE_JOB_CANCELLED,
  COMPUTE_JOB_RESOLVED,
  COMPUTE_JOB_RELEASED,
} from './compute_market.js';


/**
 * SDK version
 */
export const VERSION = '1.0.6';

/**
 * Default RPC URL (override with LICHEN_RPC_URL env var)
 */
export const DEFAULT_RPC_URL = (typeof process !== 'undefined' && process.env?.LICHEN_RPC_URL) || 'http://localhost:8899';

/**
 * Default WebSocket URL (override with LICHEN_WS_URL env var)
 */
export const DEFAULT_WS_URL = (typeof process !== 'undefined' && process.env?.LICHEN_WS_URL) || 'ws://localhost:8900';
