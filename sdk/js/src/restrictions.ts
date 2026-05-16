import type { Connection } from './connection.js';
import { PublicKey } from './publickey.js';

const MAX_U64 = (1n << 64n) - 1n;

type RpcConnection = Pick<Connection, 'rpcRequest'>;

export type AddressInput = PublicKey | string;
export type RestrictionAssetInput = PublicKey | string;

export type RestrictionTargetType =
  | 'account'
  | 'account_asset'
  | 'asset'
  | 'contract'
  | 'code_hash'
  | 'bridge_route'
  | 'protocol_module';

export type RestrictionMode =
  | 'outgoing_only'
  | 'incoming_only'
  | 'bidirectional'
  | 'frozen_amount'
  | 'asset_paused'
  | 'execute_blocked'
  | 'state_changing_blocked'
  | 'quarantined'
  | 'deploy_blocked'
  | 'route_paused'
  | 'protocol_paused'
  | 'terminated';

export type RestrictionReason =
  | 'exploit_active'
  | 'stolen_funds'
  | 'bridge_compromise'
  | 'oracle_compromise'
  | 'scam_contract'
  | 'malicious_code_hash'
  | 'sanctions_or_legal_order'
  | 'phishing_or_impersonation'
  | 'custody_incident'
  | 'protocol_bug'
  | 'governance_error_correction'
  | 'false_positive_lift'
  | 'testnet_drill';

export type RestrictionLiftReason =
  | 'incident_resolved'
  | 'false_positive'
  | 'evidence_rejected'
  | 'governance_override'
  | 'expired_cleanup'
  | 'testnet_drill_complete';

export type RestrictionReasonInput = RestrictionReason | number;
export type RestrictionLiftReasonInput = RestrictionLiftReason | number;
export type RestrictionModeInput = RestrictionMode | number;

export const BRIDGE_CHAINS = Object.freeze({
  SOLANA: 'solana',
  ETHEREUM: 'ethereum',
  BSC: 'bsc',
  BNB: 'bnb',
  NEOX: 'neox',
} as const);

export type BridgeChain = typeof BRIDGE_CHAINS[keyof typeof BRIDGE_CHAINS];

export const BRIDGE_ASSETS = Object.freeze({
  SOL: 'sol',
  ETH: 'eth',
  BNB: 'bnb',
  GAS: 'gas',
  NEO: 'neo',
  USDC: 'usdc',
  USDT: 'usdt',
} as const);

export type BridgeAsset = typeof BRIDGE_ASSETS[keyof typeof BRIDGE_ASSETS];

export interface AccountRestrictionTarget {
  type: 'account';
  account: AddressInput;
}

export interface AccountAssetRestrictionTarget {
  type: 'account_asset';
  account: AddressInput;
  asset: RestrictionAssetInput;
}

export interface AssetRestrictionTarget {
  type: 'asset';
  asset: RestrictionAssetInput;
}

export interface ContractRestrictionTarget {
  type: 'contract';
  contract: AddressInput;
}

export interface CodeHashRestrictionTarget {
  type: 'code_hash';
  codeHash?: string;
  code_hash?: string;
}

export interface BridgeRouteRestrictionTarget {
  type: 'bridge_route';
  chain: BridgeChain | string;
  asset: BridgeAsset | string;
}

export interface ProtocolModuleRestrictionTarget {
  type: 'protocol_module';
  module: string | number;
}

export type RestrictionTargetInput =
  | AccountRestrictionTarget
  | AccountAssetRestrictionTarget
  | AssetRestrictionTarget
  | ContractRestrictionTarget
  | CodeHashRestrictionTarget
  | BridgeRouteRestrictionTarget
  | ProtocolModuleRestrictionTarget;

export interface RestrictionTargetDetails {
  type: RestrictionTargetType | string;
  account?: string;
  asset?: string;
  contract?: string;
  code_hash?: string;
  chain?: string;
  chain_id?: string;
  module?: string;
  module_id?: number;
}

export interface RestrictionModeDetails {
  kind: RestrictionMode | string;
  frozen_amount?: number | null;
}

export interface RestrictionRecord {
  id: number;
  status: string;
  target_type: RestrictionTargetType | string;
  target: string;
  target_details: RestrictionTargetDetails;
  mode: RestrictionMode | string;
  mode_details: RestrictionModeDetails;
  frozen_amount?: number | null;
  reason: RestrictionReason | string;
  evidence_hash?: string | null;
  evidence_uri_hash?: string | null;
  proposer: string;
  authority: string;
  approval_authority?: string | null;
  created_slot: number;
  created_epoch: number;
  expires_at_slot?: number | null;
  supersedes?: number | null;
  lifted_by?: string | null;
  lifted_slot?: number | null;
  lift_reason?: RestrictionLiftReason | string | null;
}

export interface EffectiveRestrictionRecord extends RestrictionRecord {
  effective_status: string;
  active: boolean;
}

export interface GetRestrictionResponse {
  id: number;
  slot: number;
  found: boolean;
  restriction: EffectiveRestrictionRecord | null;
}

export interface RestrictionListParams {
  limit?: number | bigint;
  afterId?: number | bigint;
  cursor?: number | bigint | string;
}

export interface RestrictionListResponse {
  restrictions: EffectiveRestrictionRecord[];
  count: number;
  has_more: boolean;
  next_cursor?: string | null;
  slot: number;
  active_only?: boolean;
}

export interface RestrictionTargetStatus {
  slot: number;
  target_type: RestrictionTargetType | string;
  target: string;
  target_details: RestrictionTargetDetails;
  restricted: boolean;
  active: boolean;
  restriction_ids: number[];
  active_restriction_ids: number[];
  restrictions: EffectiveRestrictionRecord[];
  active_restrictions: EffectiveRestrictionRecord[];
}

export interface ContractLifecycleRestrictionStatus {
  contract: string;
  slot: number;
  found: boolean;
  is_executable: boolean;
  lifecycle_status: string;
  lifecycle_updated_slot: number;
  lifecycle_restriction_id?: number | null;
  derived_from_restriction: boolean;
  active: boolean;
  active_restriction_ids: number[];
  active_restrictions: EffectiveRestrictionRecord[];
}

export interface CodeHashRestrictionStatus {
  code_hash: string;
  slot: number;
  blocked: boolean;
  deploy_blocked: boolean;
  active_restriction_ids: number[];
  active_restrictions: RestrictionRecord[];
}

export interface BridgeRouteRestrictionStatus {
  chain: string;
  chain_id: string;
  asset: string;
  slot: number;
  paused: boolean;
  route_paused: boolean;
  active_restriction_ids: number[];
  active_restrictions: RestrictionRecord[];
}

export interface MovementRestrictionStatus {
  operation: 'send' | 'receive';
  account: string;
  asset: string;
  amount: number;
  spendable: number;
  slot: number;
  allowed: boolean;
  blocked: boolean;
  active_restriction_ids: number[];
  active_restrictions: RestrictionRecord[];
}

export interface TransferRestrictionStatus {
  operation: 'transfer';
  from: string;
  to: string;
  asset: string;
  amount: number;
  source_spendable: number;
  recipient_spendable: number;
  slot: number;
  allowed: boolean;
  blocked: boolean;
  send_allowed: boolean;
  receive_allowed: boolean;
  source_blocked: boolean;
  recipient_blocked: boolean;
  source_restriction_ids: number[];
  source_restrictions: RestrictionRecord[];
  recipient_restriction_ids: number[];
  recipient_restrictions: RestrictionRecord[];
  active_restriction_ids: number[];
  active_restrictions: RestrictionRecord[];
}

export interface RestrictionBuilderInstruction {
  program_id: string;
  accounts: string[];
  instruction_type: number;
  governance_action_type?: number | null;
  data_hex: string;
}

export interface UnsignedRestrictionGovernanceTx {
  method: string;
  unsigned: true;
  encoding: 'base64';
  wire_format: string;
  tx_type: 'native';
  transaction_base64: string;
  transaction: string;
  wire_size: number;
  message_hash: string;
  signature_count: number;
  recent_blockhash: string;
  slot?: number | null;
  proposer: string;
  governance_authority: string;
  action_label: 'restrict' | 'lift_restriction' | 'extend_restriction' | string;
  action: Record<string, unknown>;
  instruction: RestrictionBuilderInstruction;
}

export interface RestrictionBuilderBaseParams {
  proposer: AddressInput;
  governanceAuthority: AddressInput;
  recentBlockhash?: string;
}

export interface RestrictCommonParams extends RestrictionBuilderBaseParams {
  reason: RestrictionReasonInput;
  evidenceHash?: string;
  evidenceUriHash?: string;
  expiresAtSlot?: number | bigint;
}

export interface RestrictAccountParams extends RestrictCommonParams {
  account: AddressInput;
  mode?: RestrictionModeInput;
}

export interface UnrestrictAccountParams extends RestrictionBuilderBaseParams {
  account: AddressInput;
  restrictionId?: number | bigint;
  liftReason: RestrictionLiftReasonInput;
}

export interface RestrictAccountAssetParams extends RestrictCommonParams {
  account: AddressInput;
  asset: RestrictionAssetInput;
  mode?: RestrictionModeInput;
}

export interface UnrestrictAccountAssetParams extends RestrictionBuilderBaseParams {
  account: AddressInput;
  asset: RestrictionAssetInput;
  restrictionId?: number | bigint;
  liftReason: RestrictionLiftReasonInput;
}

export interface SetFrozenAssetAmountParams extends RestrictCommonParams {
  account: AddressInput;
  asset: RestrictionAssetInput;
  amount: number | bigint;
}

export interface ContractRestrictionParams extends RestrictCommonParams {
  contract: AddressInput;
}

export interface ResumeContractParams extends RestrictionBuilderBaseParams {
  contract: AddressInput;
  restrictionId?: number | bigint;
  liftReason: RestrictionLiftReasonInput;
}

export interface CodeHashRestrictionParams extends RestrictCommonParams {
  codeHash: string;
}

export interface UnbanCodeHashParams extends RestrictionBuilderBaseParams {
  codeHash: string;
  restrictionId?: number | bigint;
  liftReason: RestrictionLiftReasonInput;
}

export interface BridgeRouteRestrictionParams extends RestrictCommonParams {
  chain: BridgeChain | string;
  asset: BridgeAsset | string;
}

export interface ResumeBridgeRouteParams extends RestrictionBuilderBaseParams {
  chain: BridgeChain | string;
  asset: BridgeAsset | string;
  restrictionId?: number | bigint;
  liftReason: RestrictionLiftReasonInput;
}

export interface ExtendRestrictionParams extends RestrictionBuilderBaseParams {
  restrictionId: number | bigint;
  newExpiresAtSlot?: number | bigint;
  evidenceHash?: string;
}

export interface LiftRestrictionParams extends RestrictionBuilderBaseParams {
  restrictionId: number | bigint;
  liftReason: RestrictionLiftReasonInput;
}

export interface MovementRestrictionParams {
  account: AddressInput;
  asset: RestrictionAssetInput;
  amount?: number | bigint;
}

export interface TransferRestrictionParams {
  from: AddressInput;
  to: AddressInput;
  asset: RestrictionAssetInput;
  amount?: number | bigint;
}

function address(value: AddressInput): string {
  return value instanceof PublicKey ? value.toBase58() : value;
}

function asset(value: RestrictionAssetInput): string {
  return value instanceof PublicKey ? value.toBase58() : value;
}

function u64(value: number | bigint, fieldName: string): number | string {
  const normalized = typeof value === 'bigint'
    ? value
    : Number.isSafeInteger(value) && value >= 0
      ? BigInt(value)
      : null;

  if (normalized === null || normalized < 0n || normalized > MAX_U64) {
    throw new Error(`${fieldName} must be a u64-safe integer value`);
  }

  return typeof value === 'bigint' ? normalized.toString() : value;
}

function optionalU64(value: number | bigint | undefined, fieldName: string): number | string | undefined {
  return value === undefined ? undefined : u64(value, fieldName);
}

function omitUndefined<T extends Record<string, unknown>>(value: T): T {
  for (const key of Object.keys(value)) {
    if (value[key] === undefined) {
      delete value[key];
    }
  }
  return value;
}

function pageParams(params: RestrictionListParams = {}): Record<string, unknown> {
  return omitUndefined({
    limit: optionalU64(params.limit, 'limit'),
    after_id: params.afterId === undefined ? undefined : u64(params.afterId, 'afterId'),
    cursor: params.cursor === undefined || typeof params.cursor === 'string'
      ? params.cursor
      : u64(params.cursor, 'cursor'),
  });
}

function targetParams(target: RestrictionTargetInput): Record<string, unknown> {
  switch (target.type) {
    case 'account':
      return { type: target.type, account: address(target.account) };
    case 'account_asset':
      return { type: target.type, account: address(target.account), asset: asset(target.asset) };
    case 'asset':
      return { type: target.type, asset: asset(target.asset) };
    case 'contract':
      return { type: target.type, contract: address(target.contract) };
    case 'code_hash': {
      const codeHash = target.codeHash ?? target.code_hash;
      if (!codeHash) {
        throw new Error('codeHash is required');
      }
      return { type: target.type, code_hash: codeHash };
    }
    case 'bridge_route':
      return { type: target.type, chain: target.chain, asset: target.asset };
    case 'protocol_module':
      return { type: target.type, module: target.module };
  }
}

function builderBase(params: RestrictionBuilderBaseParams): Record<string, unknown> {
  return omitUndefined({
    proposer: address(params.proposer),
    governance_authority: address(params.governanceAuthority),
    recent_blockhash: params.recentBlockhash,
  });
}

function restrictCommon(params: RestrictCommonParams): Record<string, unknown> {
  return omitUndefined({
    ...builderBase(params),
    reason: params.reason,
    evidence_hash: params.evidenceHash,
    evidence_uri_hash: params.evidenceUriHash,
    expires_at_slot: optionalU64(params.expiresAtSlot, 'expiresAtSlot'),
  });
}

function restrictionIdParam(value: number | bigint | undefined): number | string | undefined {
  return value === undefined ? undefined : u64(value, 'restrictionId');
}

export class RestrictionGovernanceClient {
  constructor(private readonly connection: RpcConnection) {}

  private rpc<T>(method: string, params: unknown[] = []): Promise<T> {
    return this.connection.rpcRequest<T>(method, params);
  }

  async getRestriction(restrictionId: number | bigint): Promise<GetRestrictionResponse> {
    return this.rpc('getRestriction', [u64(restrictionId, 'restrictionId')]);
  }

  async listRestrictions(params: RestrictionListParams = {}): Promise<RestrictionListResponse> {
    return this.rpc('listRestrictions', [pageParams(params)]);
  }

  async listActiveRestrictions(params: RestrictionListParams = {}): Promise<RestrictionListResponse> {
    return this.rpc('listActiveRestrictions', [pageParams(params)]);
  }

  async getRestrictionStatus(target: RestrictionTargetInput): Promise<RestrictionTargetStatus> {
    return this.rpc('getRestrictionStatus', [targetParams(target)]);
  }

  async getAccountRestrictionStatus(account: AddressInput): Promise<RestrictionTargetStatus> {
    return this.rpc('getAccountRestrictionStatus', [address(account)]);
  }

  async getAssetRestrictionStatus(assetId: RestrictionAssetInput): Promise<RestrictionTargetStatus> {
    return this.rpc('getAssetRestrictionStatus', [asset(assetId)]);
  }

  async getAccountAssetRestrictionStatus(
    account: AddressInput,
    assetId: RestrictionAssetInput,
  ): Promise<RestrictionTargetStatus> {
    return this.rpc('getAccountAssetRestrictionStatus', [address(account), asset(assetId)]);
  }

  async getContractLifecycleStatus(contract: AddressInput): Promise<ContractLifecycleRestrictionStatus> {
    return this.rpc('getContractLifecycleStatus', [address(contract)]);
  }

  async getCodeHashRestrictionStatus(codeHash: string): Promise<CodeHashRestrictionStatus> {
    return this.rpc('getCodeHashRestrictionStatus', [codeHash]);
  }

  async getBridgeRouteRestrictionStatus(chain: BridgeChain | string, asset: BridgeAsset | string): Promise<BridgeRouteRestrictionStatus> {
    return this.rpc('getBridgeRouteRestrictionStatus', [chain, asset]);
  }

  async canSend(params: MovementRestrictionParams): Promise<MovementRestrictionStatus> {
    return this.rpc('canSend', [omitUndefined({
      account: address(params.account),
      asset: asset(params.asset),
      amount: optionalU64(params.amount, 'amount'),
    })]);
  }

  async canReceive(params: MovementRestrictionParams): Promise<MovementRestrictionStatus> {
    return this.rpc('canReceive', [omitUndefined({
      account: address(params.account),
      asset: asset(params.asset),
      amount: optionalU64(params.amount, 'amount'),
    })]);
  }

  async canTransfer(params: TransferRestrictionParams): Promise<TransferRestrictionStatus> {
    return this.rpc('canTransfer', [omitUndefined({
      from: address(params.from),
      to: address(params.to),
      asset: asset(params.asset),
      amount: optionalU64(params.amount, 'amount'),
    })]);
  }

  async buildRestrictAccountTx(params: RestrictAccountParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildRestrictAccountTx', [omitUndefined({
      ...restrictCommon(params),
      account: address(params.account),
      mode: params.mode,
    })]);
  }

  async buildUnrestrictAccountTx(params: UnrestrictAccountParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildUnrestrictAccountTx', [omitUndefined({
      ...builderBase(params),
      account: address(params.account),
      restriction_id: restrictionIdParam(params.restrictionId),
      lift_reason: params.liftReason,
    })]);
  }

  async buildRestrictAccountAssetTx(params: RestrictAccountAssetParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildRestrictAccountAssetTx', [omitUndefined({
      ...restrictCommon(params),
      account: address(params.account),
      asset: asset(params.asset),
      mode: params.mode,
    })]);
  }

  async buildUnrestrictAccountAssetTx(params: UnrestrictAccountAssetParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildUnrestrictAccountAssetTx', [omitUndefined({
      ...builderBase(params),
      account: address(params.account),
      asset: asset(params.asset),
      restriction_id: restrictionIdParam(params.restrictionId),
      lift_reason: params.liftReason,
    })]);
  }

  async buildSetFrozenAssetAmountTx(params: SetFrozenAssetAmountParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildSetFrozenAssetAmountTx', [omitUndefined({
      ...restrictCommon(params),
      account: address(params.account),
      asset: asset(params.asset),
      amount: u64(params.amount, 'amount'),
    })]);
  }

  async buildSuspendContractTx(params: ContractRestrictionParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildSuspendContractTx', [omitUndefined({
      ...restrictCommon(params),
      contract: address(params.contract),
    })]);
  }

  async buildResumeContractTx(params: ResumeContractParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildResumeContractTx', [omitUndefined({
      ...builderBase(params),
      contract: address(params.contract),
      restriction_id: restrictionIdParam(params.restrictionId),
      lift_reason: params.liftReason,
    })]);
  }

  async buildQuarantineContractTx(params: ContractRestrictionParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildQuarantineContractTx', [omitUndefined({
      ...restrictCommon(params),
      contract: address(params.contract),
    })]);
  }

  async buildTerminateContractTx(params: ContractRestrictionParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildTerminateContractTx', [omitUndefined({
      ...restrictCommon(params),
      contract: address(params.contract),
    })]);
  }

  async buildBanCodeHashTx(params: CodeHashRestrictionParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildBanCodeHashTx', [omitUndefined({
      ...restrictCommon(params),
      code_hash: params.codeHash,
    })]);
  }

  async buildUnbanCodeHashTx(params: UnbanCodeHashParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildUnbanCodeHashTx', [omitUndefined({
      ...builderBase(params),
      code_hash: params.codeHash,
      restriction_id: restrictionIdParam(params.restrictionId),
      lift_reason: params.liftReason,
    })]);
  }

  async buildPauseBridgeRouteTx(params: BridgeRouteRestrictionParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildPauseBridgeRouteTx', [omitUndefined({
      ...restrictCommon(params),
      chain: params.chain,
      asset: params.asset,
    })]);
  }

  async buildResumeBridgeRouteTx(params: ResumeBridgeRouteParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildResumeBridgeRouteTx', [omitUndefined({
      ...builderBase(params),
      chain: params.chain,
      asset: params.asset,
      restriction_id: restrictionIdParam(params.restrictionId),
      lift_reason: params.liftReason,
    })]);
  }

  async buildExtendRestrictionTx(params: ExtendRestrictionParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildExtendRestrictionTx', [omitUndefined({
      ...builderBase(params),
      restriction_id: u64(params.restrictionId, 'restrictionId'),
      new_expires_at_slot: optionalU64(params.newExpiresAtSlot, 'newExpiresAtSlot'),
      evidence_hash: params.evidenceHash,
    })]);
  }

  async buildLiftRestrictionTx(params: LiftRestrictionParams): Promise<UnsignedRestrictionGovernanceTx> {
    return this.rpc('buildLiftRestrictionTx', [omitUndefined({
      ...builderBase(params),
      restriction_id: u64(params.restrictionId, 'restrictionId'),
      lift_reason: params.liftReason,
    })]);
  }
}
