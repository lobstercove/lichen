import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PREMIUM_NAME_MIN_LENGTH = 3;
const PREMIUM_NAME_MAX_LENGTH = 4;
const DIRECT_NAME_MIN_LENGTH = 5;
const MAX_NAME_LENGTH = 32;
const MAX_SKILL_NAME_BYTES = 32;
const MAX_ENDPOINT_BYTES = 255;
const MAX_METADATA_BYTES = 1024;
const RECOVERY_GUARDIAN_COUNT = 5;
const MAX_U64 = (1n << 64n) - 1n;
const SPORES_PER_LICN = 1_000_000_000;
const PROGRAM_SYMBOL_CANDIDATES = ['YID', 'yid', 'LICHENID'];
const textEncoder = new TextEncoder();

export const LICHEN_ID_DELEGATE_PERMISSIONS = {
    PROFILE: 0b0000_0001,
    AGENT_TYPE: 0b0000_0010,
    SKILLS: 0b0000_0100,
    NAMING: 0b0000_1000,
} as const;

const AVAILABILITY_BY_NAME = {
    offline: 0,
    available: 1,
    busy: 2,
    online: 1,
} as const;

export type LichenIdAvailabilityStatus = 0 | 1 | 2;
export type LichenIdAvailabilityInput = LichenIdAvailabilityStatus | keyof typeof AVAILABILITY_BY_NAME;
export type LichenIdDelegatePermissionKey = keyof typeof LICHEN_ID_DELEGATE_PERMISSIONS;
export type LichenIdMetadata = Record<string, unknown> | unknown[] | string;

export interface LichenIdReputation {
    address: string;
    score: number;
    tier: number;
    tier_name: string;
}

export interface LichenIdNameResolution {
    name: string;
    owner: string;
    registered_slot: number;
    expiry_slot: number;
}

export interface LichenIdSkill {
    index: number;
    name: string;
    proficiency: number;
    attestation_count: number;
    timestamp: number;
}

export interface LichenIdReceivedVouch {
    voucher: string;
    voucher_name: string | null;
    timestamp: number;
}

export interface LichenIdGivenVouch {
    vouchee: string;
    vouchee_name: string | null;
    timestamp: number;
}

export interface LichenIdVouches {
    received: LichenIdReceivedVouch[];
    given: LichenIdGivenVouch[];
}

export interface LichenIdNameAuction {
    name: string;
    active: boolean;
    start_slot: number;
    end_slot: number;
    reserve_bid: number;
    highest_bid: number;
    highest_bidder: string;
    current_slot: number;
    ended: boolean;
}

export interface LichenIdAgentDirectoryEntry {
    address: string;
    name: string;
    licn_name: string | null;
    agent_type: number;
    agent_type_name: string;
    reputation: number;
    trust_tier: number;
    trust_tier_name: string;
    availability: number;
    available: boolean;
    rate: number;
    endpoint: string | null;
    skill_count: number;
    vouch_count: number;
    created_at: number;
    updated_at: number;
}

export interface LichenIdAgentDirectory {
    agents: LichenIdAgentDirectoryEntry[];
    count: number;
    total: number;
}

export interface LichenIdStats {
    total_identities: number;
    total_names: number;
    total_skills: number;
    total_vouches: number;
    total_attestations: number;
    tier_distribution: Record<string, number>;
}

export interface LichenIdDelegateRecord {
    owner: string;
    delegate: string;
    permissions: number;
    expiresAtMs: number;
    createdAtMs: number;
    active: boolean;
    canProfile: boolean;
    canAgentType: boolean;
    canSkills: boolean;
    canNaming: boolean;
}

export interface LichenIdProfile {
    identity: {
        address: string;
        owner: string;
        name: string;
        agent_type: number;
        agent_type_name: string;
        reputation: number;
        created_at: number;
        updated_at: number;
        skill_count: number;
        vouch_count: number;
        is_active: boolean;
    };
    licn_name: string | null;
    reputation: {
        score: number;
        tier: number;
        tier_name: string;
    };
    skills: LichenIdSkill[];
    vouches: LichenIdVouches;
    achievements: Array<Record<string, unknown>>;
    agent: {
        endpoint: string | null;
        metadata: LichenIdMetadata | null;
        availability: number;
        availability_name: string;
        rate: number;
    };
    contributions: Record<string, number>;
}

export interface RegisterIdentityParams {
    agentType: number;
    name: string;
}

export interface RegisterNameParams {
    name: string;
    durationYears?: number;
    valueSpores?: number | bigint;
}

export interface AddSkillParams {
    name: string;
    proficiency?: number;
}

export interface SetEndpointParams {
    url: string;
}

export interface SetMetadataParams {
    metadata: LichenIdMetadata;
}

export interface SetRateParams {
    rateSpores: number | bigint;
}

export interface SetAvailabilityParams {
    status: LichenIdAvailabilityInput;
}

export interface SetDelegateParams {
    delegate: PublicKey | string;
    permissions: number;
    expiresAtMs: number | bigint;
}

export interface SetEndpointAsParams {
    owner: PublicKey | string;
    url: string;
}

export interface SetMetadataAsParams {
    owner: PublicKey | string;
    metadata: LichenIdMetadata;
}

export interface SetAvailabilityAsParams {
    owner: PublicKey | string;
    status: LichenIdAvailabilityInput;
}

export interface SetRateAsParams {
    owner: PublicKey | string;
    rateSpores: number | bigint;
}

export interface UpdateAgentTypeAsParams {
    owner: PublicKey | string;
    agentType: number;
}

export interface SetRecoveryGuardiansParams {
    guardians: Array<PublicKey | string>;
}

export interface ApproveRecoveryParams {
    target: PublicKey | string;
    newOwner: PublicKey | string;
}

export interface ExecuteRecoveryParams {
    target: PublicKey | string;
    newOwner: PublicKey | string;
}

export interface AttestSkillParams {
    identity: PublicKey | string;
    name: string;
    level?: number;
}

export interface RevokeAttestationParams {
    identity: PublicKey | string;
    name: string;
}

export interface CreateNameAuctionParams {
    name: string;
    reserveBidSpores: number | bigint;
    endSlot: number | bigint;
}

export interface BidNameAuctionParams {
    name: string;
    bidAmountSpores: number | bigint;
}

export interface FinalizeNameAuctionParams {
    name: string;
    durationYears?: number;
}

export interface LichenIdAgentDirectoryOptions {
    type?: number;
    available?: boolean;
    min_reputation?: number;
    limit?: number;
    offset?: number;
}

function normalizeAddress(value: PublicKey | string): PublicKey {
    return value instanceof PublicKey ? value : new PublicKey(value);
}

function normalizeNameLabel(name: string): string {
    return name.trim().toLowerCase().replace(/\.lichen$/, '');
}

function hasValidNameCharacters(label: string): boolean {
    return !label.startsWith('-')
        && !label.endsWith('-')
        && !label.includes('--')
        && /^[a-z0-9-]+$/.test(label);
}

function validateLookupName(name: string): string {
    const label = normalizeNameLabel(name);
    if (!label) {
        throw new Error('Name cannot be empty');
    }
    if (label.length > MAX_NAME_LENGTH) {
        throw new Error('LichenID names must be at most 32 characters');
    }
    if (!hasValidNameCharacters(label)) {
        throw new Error('LichenID names must use lowercase a-z, 0-9, and internal hyphens only');
    }
    return label;
}

function validateDirectRegistrationName(name: string): string {
    const label = validateLookupName(name);
    if (label.length < DIRECT_NAME_MIN_LENGTH) {
        throw new Error('Direct registerName supports 5-32 character labels; 3-4 character names are auction-only');
    }
    return label;
}

function validateAuctionName(name: string): string {
    const label = validateLookupName(name);
    if (label.length < PREMIUM_NAME_MIN_LENGTH || label.length > PREMIUM_NAME_MAX_LENGTH) {
        throw new Error('Name auction helpers support 3-4 character premium labels only');
    }
    return label;
}

function normalizeDurationYears(value: number | undefined): number {
    return Math.max(1, Math.min(10, Math.trunc(value ?? 1) || 1));
}

function validateSkillName(name: string): string {
    const skillName = name.trim();
    if (!skillName) {
        throw new Error('Skill name cannot be empty');
    }
    if (textEncoder.encode(skillName).length > MAX_SKILL_NAME_BYTES) {
        throw new Error('Skill names must be at most 32 bytes');
    }
    return skillName;
}

function normalizeEndpointUrl(url: string): string {
    const value = url.trim();
    if (!value) {
        throw new Error('Endpoint URL cannot be empty');
    }
    if (textEncoder.encode(value).length > MAX_ENDPOINT_BYTES) {
        throw new Error('Endpoint URL must be at most 255 bytes');
    }
    return value;
}

function normalizeMetadata(metadata: LichenIdMetadata): string {
    const serialized = typeof metadata === 'string' ? metadata.trim() : JSON.stringify(metadata);
    if (!serialized) {
        throw new Error('Metadata cannot be empty');
    }
    if (textEncoder.encode(serialized).length > MAX_METADATA_BYTES) {
        throw new Error('Metadata must be at most 1024 bytes');
    }
    return serialized;
}

function normalizeAvailabilityStatus(status: LichenIdAvailabilityInput): LichenIdAvailabilityStatus {
    if (typeof status === 'number') {
        if (status === 0 || status === 1 || status === 2) {
            return status;
        }
    } else {
        const normalized = AVAILABILITY_BY_NAME[status.toLowerCase() as keyof typeof AVAILABILITY_BY_NAME];
        if (normalized !== undefined) {
            return normalized;
        }
    }

    throw new Error('Availability must be one of offline, available, busy, or the numeric values 0-2');
}

function normalizeDelegatePermissions(permissions: number): number {
    if (!Number.isInteger(permissions) || permissions <= 0 || permissions > 0x0F) {
        throw new Error('Delegate permissions must be a non-zero bitmask using PROFILE, AGENT_TYPE, SKILLS, and NAMING');
    }
    return permissions;
}

function normalizeUnsignedU64(value: number | bigint, fieldName: string): bigint {
    const normalized = typeof value === 'bigint'
        ? value
        : Number.isSafeInteger(value) && value >= 0
            ? BigInt(value)
            : null;

    if (normalized === null || normalized < 0n || normalized > MAX_U64) {
        throw new Error(`${fieldName} must be a u64-safe integer value`);
    }

    return normalized;
}

function validateAttestationLevel(level: number | undefined): number {
    const normalized = Math.trunc(level ?? 3);
    if (normalized < 1 || normalized > 5) {
        throw new Error('Attestation level must be between 1 and 5');
    }
    return normalized;
}

function normalizeRecoveryGuardians(owner: PublicKey, guardians: Array<PublicKey | string>): PublicKey[] {
    if (guardians.length !== RECOVERY_GUARDIAN_COUNT) {
        throw new Error('Recovery helpers require exactly 5 guardian addresses');
    }

    const normalized = guardians.map(normalizeAddress);
    const unique = new Set(normalized.map((guardian) => guardian.toBase58()));
    if (unique.size !== RECOVERY_GUARDIAN_COUNT) {
        throw new Error('Recovery guardians must be unique');
    }
    if (normalized.some((guardian) => guardian.equals(owner))) {
        throw new Error('Recovery guardians cannot include the owner');
    }
    return normalized;
}

function padBytes(bytes: Uint8Array, size: number): Uint8Array {
    const out = new Uint8Array(size);
    out.set(bytes.subarray(0, size));
    return out;
}

function u8(value: number): Uint8Array {
    return Uint8Array.from([value & 0xFF]);
}

function u32LE(value: number): Uint8Array {
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, value, true);
    return out;
}

function u64LE(value: number | bigint, fieldName: string): Uint8Array {
    const out = new Uint8Array(8);
    new DataView(out.buffer).setBigUint64(0, normalizeUnsignedU64(value, fieldName), true);
    return out;
}

function buildLayoutArgs(layout: number[], chunks: Uint8Array[]): Uint8Array {
    const header = Uint8Array.from([0xAB, ...layout]);
    const total = chunks.reduce((sum, chunk) => sum + chunk.length, header.length);
    const out = new Uint8Array(total);
    out.set(header, 0);
    let offset = header.length;
    for (const chunk of chunks) {
        out.set(chunk, offset);
        offset += chunk.length;
    }
    return out;
}

function encodeRegisterIdentityArgs(owner: PublicKey, params: RegisterIdentityParams): Uint8Array {
    const nameBytes = textEncoder.encode(params.name);
    return buildLayoutArgs([0x20, 0x01, 0x40, 0x04], [
        owner.toBytes(),
        u8(params.agentType),
        padBytes(nameBytes, 64),
        u32LE(nameBytes.length),
    ]);
}

function encodeNameDurationArgs(owner: PublicKey, name: string, durationYears: number): Uint8Array {
    const nameBytes = textEncoder.encode(name);
    return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [
        owner.toBytes(),
        padBytes(nameBytes, 32),
        u32LE(nameBytes.length),
        u8(durationYears),
    ]);
}

function encodeAddSkillArgs(owner: PublicKey, params: AddSkillParams): Uint8Array {
    const skillName = validateSkillName(params.name);
    const proficiency = Math.max(0, Math.min(100, Math.trunc(params.proficiency ?? 50) || 0));
    const nameBytes = textEncoder.encode(skillName);
    return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [
        owner.toBytes(),
        padBytes(nameBytes, 32),
        u32LE(nameBytes.length),
        u8(proficiency),
    ]);
}

function encodeVouchArgs(owner: PublicKey, vouchee: PublicKey): Uint8Array {
    return buildLayoutArgs([0x20, 0x20], [owner.toBytes(), vouchee.toBytes()]);
}

function encodeEndpointArgs(owner: PublicKey, url: string): Uint8Array {
    const endpoint = normalizeEndpointUrl(url);
    const urlBytes = textEncoder.encode(endpoint);
    const stride = Math.max(32, urlBytes.length);
    return buildLayoutArgs([0x20, stride, 0x04], [
        owner.toBytes(),
        padBytes(urlBytes, stride),
        u32LE(urlBytes.length),
    ]);
}

function encodeMetadataArgs(owner: PublicKey, metadata: LichenIdMetadata): Uint8Array {
    const serialized = normalizeMetadata(metadata);
    const metadataBytes = textEncoder.encode(serialized);
    const stride = Math.max(32, metadataBytes.length);
    return buildLayoutArgs([0x20, stride, 0x04], [
        owner.toBytes(),
        padBytes(metadataBytes, stride),
        u32LE(metadataBytes.length),
    ]);
}

function encodeRateArgs(owner: PublicKey, rateSpores: number | bigint): Uint8Array {
    return buildLayoutArgs([0x20, 0x08], [
        owner.toBytes(),
        u64LE(rateSpores, 'rateSpores'),
    ]);
}

function encodeAvailabilityArgs(owner: PublicKey, status: LichenIdAvailabilityInput): Uint8Array {
    return buildLayoutArgs([0x20, 0x01], [
        owner.toBytes(),
        u8(normalizeAvailabilityStatus(status)),
    ]);
}

function encodeSetDelegateArgs(owner: PublicKey, params: SetDelegateParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x20, 0x01, 0x08], [
        owner.toBytes(),
        normalizeAddress(params.delegate).toBytes(),
        u8(normalizeDelegatePermissions(params.permissions)),
        u64LE(params.expiresAtMs, 'expiresAtMs'),
    ]);
}

function encodeDelegateLookupArgs(owner: PublicKey, delegate: PublicKey): Uint8Array {
    return buildLayoutArgs([0x20, 0x20], [owner.toBytes(), delegate.toBytes()]);
}

function encodeDelegatedEndpointArgs(delegate: PublicKey, params: SetEndpointAsParams): Uint8Array {
    const owner = normalizeAddress(params.owner);
    const endpoint = normalizeEndpointUrl(params.url);
    const urlBytes = textEncoder.encode(endpoint);
    const stride = Math.max(32, urlBytes.length);
    return buildLayoutArgs([0x20, 0x20, stride, 0x04], [
        delegate.toBytes(),
        owner.toBytes(),
        padBytes(urlBytes, stride),
        u32LE(urlBytes.length),
    ]);
}

function encodeDelegatedMetadataArgs(delegate: PublicKey, params: SetMetadataAsParams): Uint8Array {
    const owner = normalizeAddress(params.owner);
    const serialized = normalizeMetadata(params.metadata);
    const metadataBytes = textEncoder.encode(serialized);
    const stride = Math.max(32, metadataBytes.length);
    return buildLayoutArgs([0x20, 0x20, stride, 0x04], [
        delegate.toBytes(),
        owner.toBytes(),
        padBytes(metadataBytes, stride),
        u32LE(metadataBytes.length),
    ]);
}

function encodeDelegatedAvailabilityArgs(delegate: PublicKey, params: SetAvailabilityAsParams): Uint8Array {
    const owner = normalizeAddress(params.owner);
    return buildLayoutArgs([0x20, 0x20, 0x01], [
        delegate.toBytes(),
        owner.toBytes(),
        u8(normalizeAvailabilityStatus(params.status)),
    ]);
}

function encodeDelegatedRateArgs(delegate: PublicKey, params: SetRateAsParams): Uint8Array {
    const owner = normalizeAddress(params.owner);
    return buildLayoutArgs([0x20, 0x20, 0x08], [
        delegate.toBytes(),
        owner.toBytes(),
        u64LE(params.rateSpores, 'rateSpores'),
    ]);
}

function encodeUpdateAgentTypeAsArgs(delegate: PublicKey, params: UpdateAgentTypeAsParams): Uint8Array {
    const owner = normalizeAddress(params.owner);
    return buildLayoutArgs([0x20, 0x20, 0x01], [
        delegate.toBytes(),
        owner.toBytes(),
        u8(params.agentType),
    ]);
}

function encodeRecoveryGuardiansArgs(owner: PublicKey, params: SetRecoveryGuardiansParams): Uint8Array {
    const guardians = normalizeRecoveryGuardians(owner, params.guardians);
    return buildLayoutArgs([0x20, 0x20, 0x20, 0x20, 0x20, 0x20], [
        owner.toBytes(),
        guardians[0].toBytes(),
        guardians[1].toBytes(),
        guardians[2].toBytes(),
        guardians[3].toBytes(),
        guardians[4].toBytes(),
    ]);
}

function encodeRecoveryActionArgs(caller: PublicKey, params: ApproveRecoveryParams | ExecuteRecoveryParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x20, 0x20], [
        caller.toBytes(),
        normalizeAddress(params.target).toBytes(),
        normalizeAddress(params.newOwner).toBytes(),
    ]);
}

function encodeAttestSkillArgs(attester: PublicKey, params: AttestSkillParams): Uint8Array {
    const skillName = validateSkillName(params.name);
    const nameBytes = textEncoder.encode(skillName);
    return buildLayoutArgs([0x20, 0x20, 0x20, 0x04, 0x01], [
        attester.toBytes(),
        normalizeAddress(params.identity).toBytes(),
        padBytes(nameBytes, 32),
        u32LE(nameBytes.length),
        u8(validateAttestationLevel(params.level)),
    ]);
}

function encodeGetAttestationsArgs(identity: PublicKey, skillName: string): Uint8Array {
    const normalized = validateSkillName(skillName);
    const nameBytes = textEncoder.encode(normalized);
    return buildLayoutArgs([0x20, 0x20, 0x04], [
        identity.toBytes(),
        padBytes(nameBytes, 32),
        u32LE(nameBytes.length),
    ]);
}

function encodeRevokeAttestationArgs(attester: PublicKey, params: RevokeAttestationParams): Uint8Array {
    const skillName = validateSkillName(params.name);
    const nameBytes = textEncoder.encode(skillName);
    return buildLayoutArgs([0x20, 0x20, 0x20, 0x04], [
        attester.toBytes(),
        normalizeAddress(params.identity).toBytes(),
        padBytes(nameBytes, 32),
        u32LE(nameBytes.length),
    ]);
}

function encodeCreateNameAuctionArgs(owner: PublicKey, params: CreateNameAuctionParams): Uint8Array {
    const label = validateAuctionName(params.name);
    const nameBytes = textEncoder.encode(label);
    return buildLayoutArgs([0x20, 0x20, 0x04, 0x08, 0x08], [
        owner.toBytes(),
        padBytes(nameBytes, 32),
        u32LE(nameBytes.length),
        u64LE(params.reserveBidSpores, 'reserveBidSpores'),
        u64LE(params.endSlot, 'endSlot'),
    ]);
}

function encodeBidNameAuctionArgs(owner: PublicKey, params: BidNameAuctionParams): Uint8Array {
    const label = validateAuctionName(params.name);
    const nameBytes = textEncoder.encode(label);
    return buildLayoutArgs([0x20, 0x20, 0x04, 0x08], [
        owner.toBytes(),
        padBytes(nameBytes, 32),
        u32LE(nameBytes.length),
        u64LE(params.bidAmountSpores, 'bidAmountSpores'),
    ]);
}

function decodeReturnData(returnData: string): Uint8Array {
    return Uint8Array.from(Buffer.from(returnData, 'base64'));
}

function readU64(bytes: Uint8Array, offset: number): bigint {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return view.getBigUint64(offset, true);
}

function readSafeU64(bytes: Uint8Array, offset: number, fieldName: string): number {
    const value = readU64(bytes, offset);
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new Error(`${fieldName} exceeds JavaScript's safe integer range`);
    }
    return Number(value);
}

function ensureReturnCodeZero(result: ReadonlyContractResult, functionName: string): void {
    const code = result.returnCode ?? 0;
    if (code !== 0) {
        throw new Error(result.error ?? `LichenID ${functionName} returned code ${code}`);
    }
    if (result.success === false && result.error) {
        throw new Error(result.error);
    }
}

function decodeDelegateRecord(owner: PublicKey, delegate: PublicKey, bytes: Uint8Array): LichenIdDelegateRecord {
    if (bytes.length < 17) {
        throw new Error('Delegate record payload was shorter than expected');
    }

    const permissions = bytes[0];
    const expiresAtMs = readSafeU64(bytes, 1, 'expiresAtMs');
    const createdAtMs = readSafeU64(bytes, 9, 'createdAtMs');

    return {
        owner: owner.toBase58(),
        delegate: delegate.toBase58(),
        permissions,
        expiresAtMs,
        createdAtMs,
        active: Date.now() < expiresAtMs,
        canProfile: (permissions & LICHEN_ID_DELEGATE_PERMISSIONS.PROFILE) !== 0,
        canAgentType: (permissions & LICHEN_ID_DELEGATE_PERMISSIONS.AGENT_TYPE) !== 0,
        canSkills: (permissions & LICHEN_ID_DELEGATE_PERMISSIONS.SKILLS) !== 0,
        canNaming: (permissions & LICHEN_ID_DELEGATE_PERMISSIONS.NAMING) !== 0,
    };
}

function registrationCostPerYearLicn(name: string): number {
    const label = normalizeNameLabel(name);
    if (label.length <= 3) return 500;
    if (label.length === 4) return 100;
    return 20;
}

export function estimateLichenIdNameRegistrationCost(name: string, durationYears: number = 1): bigint {
    const years = Math.max(1, Math.min(10, Math.trunc(durationYears) || 1));
    return BigInt(registrationCostPerYearLicn(name)) * BigInt(years) * BigInt(SPORES_PER_LICN);
}

export class LichenIdClient {
    private resolvedProgram?: PublicKey;

    constructor(
        private readonly connection: Connection,
        programId?: PublicKey,
    ) {
        this.resolvedProgram = programId;
    }

    private async callReadonly(functionName: string, args: Uint8Array): Promise<ReadonlyContractResult> {
        const programId = await this.getProgramId();
        return this.connection.callReadonlyContract(programId, functionName, args);
    }

    async getProgramId(): Promise<PublicKey> {
        if (this.resolvedProgram) {
            return this.resolvedProgram;
        }

        for (const symbol of PROGRAM_SYMBOL_CANDIDATES) {
            try {
                const entry = await this.connection.getSymbolRegistry(symbol);
                if (entry?.program) {
                    this.resolvedProgram = new PublicKey(entry.program);
                    return this.resolvedProgram;
                }
            } catch {
                // Try the next known registry alias.
            }
        }

        throw new Error('Unable to resolve the LichenID program via getSymbolRegistry("YID")');
    }

    async getProfile(address: PublicKey | string): Promise<LichenIdProfile | null> {
        return this.connection.getLichenIdProfile(normalizeAddress(address));
    }

    async getReputation(address: PublicKey | string): Promise<LichenIdReputation | null> {
        return this.connection.getLichenIdReputation(normalizeAddress(address));
    }

    async getSkills(address: PublicKey | string): Promise<LichenIdSkill[]> {
        return this.connection.getLichenIdSkills(normalizeAddress(address));
    }

    async getVouches(address: PublicKey | string): Promise<LichenIdVouches> {
        return this.connection.getLichenIdVouches(normalizeAddress(address));
    }

    async resolveName(name: string): Promise<LichenIdNameResolution | null> {
        const label = validateLookupName(name);
        return this.connection.resolveLichenName(`${label}.lichen`);
    }

    async getMetadata(address: PublicKey | string): Promise<LichenIdMetadata | null> {
        const profile = await this.getProfile(address);
        return profile?.agent?.metadata ?? null;
    }

    async getDelegate(owner: PublicKey | string, delegate: PublicKey | string): Promise<LichenIdDelegateRecord | null> {
        const ownerAddress = normalizeAddress(owner);
        const delegateAddress = normalizeAddress(delegate);
        const result = await this.callReadonly('get_delegate', encodeDelegateLookupArgs(ownerAddress, delegateAddress));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) {
            return null;
        }
        ensureReturnCodeZero(result, 'get_delegate');
        return decodeDelegateRecord(ownerAddress, delegateAddress, decodeReturnData(result.returnData));
    }

    async getAttestations(identity: PublicKey | string, skillName: string): Promise<number> {
        const result = await this.callReadonly('get_attestations', encodeGetAttestationsArgs(normalizeAddress(identity), skillName));
        ensureReturnCodeZero(result, 'get_attestations');
        if (!result.returnData) {
            throw new Error('LichenID get_attestations did not return attestation data');
        }
        return readSafeU64(decodeReturnData(result.returnData), 0, 'attestationCount');
    }

    async getNameAuction(name: string): Promise<LichenIdNameAuction | null> {
        return this.connection.getNameAuction(validateLookupName(name));
    }

    async getAgentDirectory(options: LichenIdAgentDirectoryOptions = {}): Promise<LichenIdAgentDirectory> {
        return this.connection.getLichenIdAgentDirectory(options);
    }

    async getStats(): Promise<LichenIdStats> {
        return this.connection.getLichenIdStats();
    }

    async registerIdentity(owner: Keypair, params: RegisterIdentityParams): Promise<string> {
        const name = params.name.trim();
        if (!name) {
            throw new Error('Identity name cannot be empty');
        }

        const args = encodeRegisterIdentityArgs(owner.pubkey(), {
            agentType: params.agentType,
            name,
        });
        const programId = await this.getProgramId();
        return this.connection.callContract(owner, programId, 'register_identity', args);
    }

    async registerName(owner: Keypair, params: RegisterNameParams): Promise<string> {
        const durationYears = normalizeDurationYears(params.durationYears);
        const label = validateDirectRegistrationName(params.name);
        const value = params.valueSpores ?? estimateLichenIdNameRegistrationCost(label, durationYears);
        const args = encodeNameDurationArgs(owner.pubkey(), label, durationYears);
        const programId = await this.getProgramId();
        return this.connection.callContract(owner, programId, 'register_name', args, value);
    }

    async addSkill(owner: Keypair, params: AddSkillParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeAddSkillArgs(owner.pubkey(), params);
        return this.connection.callContract(owner, programId, 'add_skill', args);
    }

    async vouch(owner: Keypair, vouchee: PublicKey | string): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeVouchArgs(owner.pubkey(), normalizeAddress(vouchee));
        return this.connection.callContract(owner, programId, 'vouch', args);
    }

    async setEndpoint(owner: Keypair, params: SetEndpointParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeEndpointArgs(owner.pubkey(), params.url);
        return this.connection.callContract(owner, programId, 'set_endpoint', args);
    }

    async setMetadata(owner: Keypair, params: SetMetadataParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeMetadataArgs(owner.pubkey(), params.metadata);
        return this.connection.callContract(owner, programId, 'set_metadata', args);
    }

    async setRate(owner: Keypair, params: SetRateParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeRateArgs(owner.pubkey(), params.rateSpores);
        return this.connection.callContract(owner, programId, 'set_rate', args);
    }

    async setAvailability(owner: Keypair, params: SetAvailabilityParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeAvailabilityArgs(owner.pubkey(), params.status);
        return this.connection.callContract(owner, programId, 'set_availability', args);
    }

    async setDelegate(owner: Keypair, params: SetDelegateParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeSetDelegateArgs(owner.pubkey(), params);
        return this.connection.callContract(owner, programId, 'set_delegate', args);
    }

    async revokeDelegate(owner: Keypair, delegate: PublicKey | string): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDelegateLookupArgs(owner.pubkey(), normalizeAddress(delegate));
        return this.connection.callContract(owner, programId, 'revoke_delegate', args);
    }

    async setEndpointAs(delegate: Keypair, params: SetEndpointAsParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDelegatedEndpointArgs(delegate.pubkey(), params);
        return this.connection.callContract(delegate, programId, 'set_endpoint_as', args);
    }

    async setMetadataAs(delegate: Keypair, params: SetMetadataAsParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDelegatedMetadataArgs(delegate.pubkey(), params);
        return this.connection.callContract(delegate, programId, 'set_metadata_as', args);
    }

    async setAvailabilityAs(delegate: Keypair, params: SetAvailabilityAsParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDelegatedAvailabilityArgs(delegate.pubkey(), params);
        return this.connection.callContract(delegate, programId, 'set_availability_as', args);
    }

    async setRateAs(delegate: Keypair, params: SetRateAsParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDelegatedRateArgs(delegate.pubkey(), params);
        return this.connection.callContract(delegate, programId, 'set_rate_as', args);
    }

    async updateAgentTypeAs(delegate: Keypair, params: UpdateAgentTypeAsParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUpdateAgentTypeAsArgs(delegate.pubkey(), params);
        return this.connection.callContract(delegate, programId, 'update_agent_type_as', args);
    }

    async setRecoveryGuardians(owner: Keypair, params: SetRecoveryGuardiansParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeRecoveryGuardiansArgs(owner.pubkey(), params);
        return this.connection.callContract(owner, programId, 'set_recovery_guardians', args);
    }

    async approveRecovery(guardian: Keypair, params: ApproveRecoveryParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeRecoveryActionArgs(guardian.pubkey(), params);
        return this.connection.callContract(guardian, programId, 'approve_recovery', args);
    }

    async executeRecovery(guardian: Keypair, params: ExecuteRecoveryParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeRecoveryActionArgs(guardian.pubkey(), params);
        return this.connection.callContract(guardian, programId, 'execute_recovery', args);
    }

    async attestSkill(attester: Keypair, params: AttestSkillParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeAttestSkillArgs(attester.pubkey(), params);
        return this.connection.callContract(attester, programId, 'attest_skill', args);
    }

    async revokeAttestation(attester: Keypair, params: RevokeAttestationParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeRevokeAttestationArgs(attester.pubkey(), params);
        return this.connection.callContract(attester, programId, 'revoke_attestation', args);
    }

    async createNameAuction(owner: Keypair, params: CreateNameAuctionParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeCreateNameAuctionArgs(owner.pubkey(), params);
        return this.connection.callContract(owner, programId, 'create_name_auction', args);
    }

    async bidNameAuction(owner: Keypair, params: BidNameAuctionParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeBidNameAuctionArgs(owner.pubkey(), params);
        return this.connection.callContract(owner, programId, 'bid_name_auction', args, params.bidAmountSpores);
    }

    async finalizeNameAuction(owner: Keypair, params: FinalizeNameAuctionParams): Promise<string> {
        const durationYears = normalizeDurationYears(params.durationYears);
        const label = validateAuctionName(params.name);
        const programId = await this.getProgramId();
        const args = encodeNameDurationArgs(owner.pubkey(), label, durationYears);
        return this.connection.callContract(owner, programId, 'finalize_name_auction', args);
    }
}