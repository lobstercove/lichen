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
export { SporeVaultClient } from './sporevault.js';
export { BountyBoardClient } from './bountyboard.js';
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
  Block,
  Validator,
  NetworkInfo,
  ChainStatus,
  Metrics,
  ProofStep,
  ReadonlyContractResult,
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
  ThallLendProtocolStats,
  ThallLendStats,
} from './thalllend.js';
export type {
  AddLiquidityParams,
  CreatePoolParams,
  LichenSwapPoolInfo,
  LichenSwapProtocolFees,
  LichenSwapStats,
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
  SporePayStreamInfo,
  TransferStreamParams,
  WithdrawFromStreamParams,
} from './sporepay.js';
export type {
  SporeVaultStats,
  SporeVaultStrategyInfo,
  SporeVaultUserPosition,
  SporeVaultVaultStats,
} from './sporevault.js';
export type {
  ApproveWorkParams,
  BountyBoardBountyInfo,
  BountyBoardPlatformStats,
  BountyBoardStats,
  CreateBountyParams,
  SubmitWorkParams,
} from './bountyboard.js';
export {
  BOUNTY_STATUS_OPEN,
  BOUNTY_STATUS_COMPLETED,
  BOUNTY_STATUS_CANCELLED,
} from './bountyboard.js';


/**
 * SDK version
 */
export const VERSION = '1.0.2';

/**
 * Default RPC URL (override with LICHEN_RPC_URL env var)
 */
export const DEFAULT_RPC_URL = (typeof process !== 'undefined' && process.env?.LICHEN_RPC_URL) || 'http://localhost:8899';

/**
 * Default WebSocket URL (override with LICHEN_WS_URL env var)
 */
export const DEFAULT_WS_URL = (typeof process !== 'undefined' && process.env?.LICHEN_WS_URL) || 'ws://localhost:8900';
