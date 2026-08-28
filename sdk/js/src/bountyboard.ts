import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['BOUNTY', 'bounty', 'BountyBoard', 'BOUNTYBOARD', 'bountyboard'];
const MAX_U64 = (1n << 64n) - 1n;
const BOUNTY_DATA_SIZE = 91;
const PLATFORM_STATS_SIZE = 32;
const SUBMISSION_DATA_SIZE = 72;
const BOUNTY_TERMS_SIZE = 64;
const ACCOUNTING_MIGRATION_STATUS_SIZE = 40;
const ACCOUNTING_HEALTH_SIZE = 56;
const ADMIN_TRANSITION_SIZE = 64;

// Bounty status constants
export const BOUNTY_STATUS_OPEN = 0;
export const BOUNTY_STATUS_COMPLETED = 1;
export const BOUNTY_STATUS_CANCELLED = 2;

export interface BountyBoardBountyInfo {
    creator: PublicKey;
    titleHash: Uint8Array;
    rewardAmount: bigint;
    deadlineSlot: bigint;
    status: number;
    submissionCount: number;
    createdSlot: bigint;
    approvedIdx: number;
}

export interface BountyBoardPlatformStats {
    bountyCount: bigint;
    completedCount: bigint;
    rewardVolume: bigint;
    cancelCount: bigint;
}

export interface BountyBoardSubmission {
    worker: PublicKey;
    proofHash: Uint8Array;
    submittedSlot: bigint;
}

export interface BountyBoardTerms {
    rewardToken: PublicKey;
    platformFeeBps: bigint;
    grossReward: bigint;
    workerNet: bigint;
    platformFee: bigint;
}

export interface BountyBoardAccountingMigrationStatus {
    expectedBountyCount: bigint;
    cursor: bigint;
    reconstructedEscrow: bigint;
    accountingVersion: bigint;
    locked: boolean;
}

export interface BountyBoardAccountingHealth {
    accountingVersion: bigint;
    migrationLocked: boolean;
    escrowLiability: bigint;
    platformFees: bigint;
    totalLiability: bigint;
    custodyBalance: bigint;
    solvent: boolean;
}

export interface BountyBoardAdminTransition {
    currentAdmin: PublicKey;
    pendingAdmin: PublicKey | null;
}

export interface BountyBoardStats {
    bountyCount: bigint;
    completedCount: bigint;
    totalRewardVolume: bigint;
    cancelCount: bigint;
    paused: boolean;
}

export interface CreateBountyParams {
    titleHash: Uint8Array;
    rewardAmount: number | bigint;
    deadlineSlot: number | bigint;
    paymentValue?: number | bigint;
}

export interface SubmitWorkParams {
    bountyId: number | bigint;
    proofHash: Uint8Array;
}

export interface ApproveWorkParams {
    bountyId: number | bigint;
    submissionIdx: number;
}

function normalizeAddress(value: PublicKey | string): PublicKey {
    return value instanceof PublicKey ? value : new PublicKey(value);
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

function rpcU64(exact: unknown, fallback: unknown, fieldName: string): bigint {
    if (typeof exact === 'string' && /^\d+$/.test(exact)) {
        return normalizeUnsignedU64(BigInt(exact), fieldName);
    }
    if (typeof fallback === 'number' || typeof fallback === 'bigint') {
        return normalizeUnsignedU64(fallback, fieldName);
    }
    return 0n;
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

function decodeReturnData(returnData: string): Uint8Array {
    return Uint8Array.from(Buffer.from(returnData, 'base64'));
}

function readU64(bytes: Uint8Array, offset: number): bigint {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return view.getBigUint64(offset, true);
}

function readFlag(bytes: Uint8Array, offset: number, fieldName: string): boolean {
    const value = readU64(bytes, offset);
    if (value !== 0n && value !== 1n) {
        throw new Error(`BountyBoard ${fieldName} must be encoded as 0 or 1`);
    }
    return value === 1n;
}

function ensureReadonlySuccess(
    result: ReadonlyContractResult,
    functionName: string,
    allowedReturnCodes: number[] = [0],
): void {
    const code = result.returnCode ?? 0;
    if (!allowedReturnCodes.includes(code)) {
        throw new Error(result.error ?? `BountyBoard ${functionName} returned code ${code}`);
    }
    if (result.success === false) {
        throw new Error(result.error ?? `BountyBoard ${functionName} failed`);
    }
}

function ensureBytes32(value: Uint8Array, fieldName: string): Uint8Array {
    if (value.length !== 32) {
        throw new Error(`${fieldName} must be exactly 32 bytes`);
    }
    if (value.every((byte) => byte === 0)) {
        throw new Error(`${fieldName} must not be the zero hash`);
    }
    return value;
}

// --- Encoding helpers ---

function encodeCreateBountyArgs(creator: PublicKey, titleHash: Uint8Array, rewardAmount: bigint, deadlineSlot: bigint): Uint8Array {
    return buildLayoutArgs(
        [0x20, 0x20, 0x08, 0x08],
        [creator.toBytes(), ensureBytes32(titleHash, 'titleHash'), u64LE(rewardAmount, 'rewardAmount'), u64LE(deadlineSlot, 'deadlineSlot')],
    );
}

function encodeSubmitWorkArgs(bountyId: bigint, worker: PublicKey, proofHash: Uint8Array): Uint8Array {
    return buildLayoutArgs(
        [0x08, 0x20, 0x20],
        [u64LE(bountyId, 'bountyId'), worker.toBytes(), ensureBytes32(proofHash, 'proofHash')],
    );
}

function encodeApproveWorkArgs(caller: PublicKey, bountyId: bigint, submissionIdx: number): Uint8Array {
    if (submissionIdx < 0 || submissionIdx > 255) {
        throw new Error('submissionIdx must be 0-255');
    }
    return buildLayoutArgs(
        [0x20, 0x08, 0x01],
        [caller.toBytes(), u64LE(bountyId, 'bountyId'), Uint8Array.from([submissionIdx])],
    );
}

function encodeCancelBountyArgs(caller: PublicKey, bountyId: bigint): Uint8Array {
    return buildLayoutArgs(
        [0x20, 0x08],
        [caller.toBytes(), u64LE(bountyId, 'bountyId')],
    );
}

function encodeBountyIdArgs(bountyId: bigint): Uint8Array {
    return buildLayoutArgs([0x08], [u64LE(bountyId, 'bountyId')]);
}

function encodeSubmissionArgs(bountyId: bigint, submissionIdx: number): Uint8Array {
    if (!Number.isInteger(submissionIdx) || submissionIdx < 0 || submissionIdx > 255) {
        throw new Error('submissionIdx must be 0-255');
    }
    return buildLayoutArgs([0x08, 0x01], [u64LE(bountyId, 'bountyId'), Uint8Array.from([submissionIdx])]);
}

function encodeUpdateWorkArgs(bountyId: bigint, submissionIdx: number, worker: PublicKey, proofHash: Uint8Array): Uint8Array {
    if (!Number.isInteger(submissionIdx) || submissionIdx < 0 || submissionIdx > 255) {
        throw new Error('submissionIdx must be 0-255');
    }
    return buildLayoutArgs(
        [0x08, 0x01, 0x20, 0x20],
        [u64LE(bountyId, 'bountyId'), Uint8Array.from([submissionIdx]), worker.toBytes(), ensureBytes32(proofHash, 'proofHash')],
    );
}

function encodeAddressArgs(address: PublicKey | string): Uint8Array {
    return buildLayoutArgs([0x20], [normalizeAddress(address).toBytes()]);
}

function encodeCallerAddressArgs(caller: PublicKey, address: PublicKey | string): Uint8Array {
    return buildLayoutArgs([0x20, 0x20], [caller.toBytes(), normalizeAddress(address).toBytes()]);
}

function encodeCallerAddressAmountArgs(caller: PublicKey, address: PublicKey | string, amount: number | bigint): Uint8Array {
    return buildLayoutArgs(
        [0x20, 0x20, 0x08],
        [caller.toBytes(), normalizeAddress(address).toBytes(), u64LE(amount, 'amount')],
    );
}

function encodeCallerU64Args(caller: PublicKey, value: number | bigint, fieldName: string): Uint8Array {
    return buildLayoutArgs(
        [0x20, 0x08],
        [caller.toBytes(), u64LE(value, fieldName)],
    );
}

function encodeMigrationCompletionArgs(
    caller: PublicKey,
    expectedEscrow: number | bigint,
    expectedPlatformFees: number | bigint,
    expectedTotalLiability: number | bigint,
): Uint8Array {
    return buildLayoutArgs(
        [0x20, 0x08, 0x08, 0x08],
        [
            caller.toBytes(),
            u64LE(expectedEscrow, 'expectedEscrow'),
            u64LE(expectedPlatformFees, 'expectedPlatformFees'),
            u64LE(expectedTotalLiability, 'expectedTotalLiability'),
        ],
    );
}

// --- Decoding helpers ---

function decodeBountyInfo(result: ReadonlyContractResult): BountyBoardBountyInfo {
    ensureReadonlySuccess(result, 'get_bounty');
    if (!result.returnData) {
        throw new Error('BountyBoard get_bounty did not return bounty data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length !== BOUNTY_DATA_SIZE) {
        throw new Error('BountyBoard get_bounty payload must be exactly 91 bytes');
    }
    return {
        creator: new PublicKey(bytes.slice(0, 32)),
        titleHash: bytes.slice(32, 64),
        rewardAmount: readU64(bytes, 64),
        deadlineSlot: readU64(bytes, 72),
        status: bytes[80],
        submissionCount: bytes[81],
        createdSlot: readU64(bytes, 82),
        approvedIdx: bytes[90],
    };
}

function decodePlatformStats(result: ReadonlyContractResult): BountyBoardPlatformStats {
    ensureReadonlySuccess(result, 'get_platform_stats');
    if (!result.returnData) {
        throw new Error('BountyBoard get_platform_stats did not return stats data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length !== PLATFORM_STATS_SIZE) {
        throw new Error('BountyBoard get_platform_stats payload must be exactly 32 bytes');
    }
    return {
        bountyCount: readU64(bytes, 0),
        completedCount: readU64(bytes, 8),
        rewardVolume: readU64(bytes, 16),
        cancelCount: readU64(bytes, 24),
    };
}

function decodeSubmission(result: ReadonlyContractResult): BountyBoardSubmission {
    ensureReadonlySuccess(result, 'get_submission');
    if (!result.returnData) {
        throw new Error('BountyBoard get_submission did not return submission data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length !== SUBMISSION_DATA_SIZE) {
        throw new Error('BountyBoard get_submission payload must be exactly 72 bytes');
    }
    return {
        worker: new PublicKey(bytes.slice(0, 32)),
        proofHash: bytes.slice(32, 64),
        submittedSlot: readU64(bytes, 64),
    };
}

function decodeBountyTerms(result: ReadonlyContractResult): BountyBoardTerms {
    ensureReadonlySuccess(result, 'get_bounty_terms');
    if (!result.returnData) {
        throw new Error('BountyBoard get_bounty_terms did not return terms data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length !== BOUNTY_TERMS_SIZE) {
        throw new Error('BountyBoard get_bounty_terms payload must be exactly 64 bytes');
    }
    return {
        rewardToken: new PublicKey(bytes.slice(0, 32)),
        platformFeeBps: readU64(bytes, 32),
        grossReward: readU64(bytes, 40),
        workerNet: readU64(bytes, 48),
        platformFee: readU64(bytes, 56),
    };
}

function decodeAccountingMigrationStatus(
    result: ReadonlyContractResult,
): BountyBoardAccountingMigrationStatus {
    ensureReadonlySuccess(result, 'get_accounting_migration_status');
    if (!result.returnData) {
        throw new Error('BountyBoard get_accounting_migration_status did not return data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length !== ACCOUNTING_MIGRATION_STATUS_SIZE) {
        throw new Error('BountyBoard accounting migration status must be exactly 40 bytes');
    }
    return {
        expectedBountyCount: readU64(bytes, 0),
        cursor: readU64(bytes, 8),
        reconstructedEscrow: readU64(bytes, 16),
        accountingVersion: readU64(bytes, 24),
        locked: readFlag(bytes, 32, 'migration lock'),
    };
}

function decodeAccountingHealth(result: ReadonlyContractResult): BountyBoardAccountingHealth {
    ensureReadonlySuccess(result, 'get_accounting_health');
    if (!result.returnData) {
        throw new Error('BountyBoard get_accounting_health did not return data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length !== ACCOUNTING_HEALTH_SIZE) {
        throw new Error('BountyBoard accounting health must be exactly 56 bytes');
    }
    return {
        accountingVersion: readU64(bytes, 0),
        migrationLocked: readFlag(bytes, 8, 'migration lock'),
        escrowLiability: readU64(bytes, 16),
        platformFees: readU64(bytes, 24),
        totalLiability: readU64(bytes, 32),
        custodyBalance: readU64(bytes, 40),
        solvent: readFlag(bytes, 48, 'solvent flag'),
    };
}

function decodeAdminTransition(result: ReadonlyContractResult): BountyBoardAdminTransition {
    ensureReadonlySuccess(result, 'get_admin_transition');
    if (!result.returnData) {
        throw new Error('BountyBoard get_admin_transition did not return data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length !== ADMIN_TRANSITION_SIZE) {
        throw new Error('BountyBoard admin transition must be exactly 64 bytes');
    }
    const pending = bytes.slice(32, 64);
    return {
        currentAdmin: new PublicKey(bytes.slice(0, 32)),
        pendingAdmin: pending.every((byte) => byte === 0) ? null : new PublicKey(pending),
    };
}

export class BountyBoardClient {
    private resolvedProgram?: PublicKey;

    constructor(
        private readonly connection: Connection,
        programId?: PublicKey,
    ) {
        this.resolvedProgram = programId;
    }

    private async callReadonly(functionName: string, args: Uint8Array = new Uint8Array()): Promise<ReadonlyContractResult> {
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

        throw new Error('Unable to resolve the BountyBoard program via getSymbolRegistry("BOUNTY")');
    }

    // --- Read methods ---

    async getBounty(bountyId: number | bigint): Promise<BountyBoardBountyInfo | null> {
        const result = await this.callReadonly('get_bounty', encodeBountyIdArgs(normalizeUnsignedU64(bountyId, 'bountyId')));
        if ((result.returnCode ?? 0) === 1) {
            return null;
        }
        return decodeBountyInfo(result);
    }

    async getBountyCount(): Promise<bigint> {
        const result = await this.callReadonly('get_bounty_count_exact');
        ensureReadonlySuccess(result, 'get_bounty_count_exact');
        if (!result.returnData) {
            throw new Error('BountyBoard get_bounty_count_exact did not return data');
        }
        const bytes = decodeReturnData(result.returnData);
        if (bytes.length !== 8) {
            throw new Error('BountyBoard get_bounty_count_exact payload must be exactly 8 bytes');
        }
        return readU64(bytes, 0);
    }

    async getPlatformStats(): Promise<BountyBoardPlatformStats> {
        return decodePlatformStats(await this.callReadonly('get_platform_stats'));
    }

    async getSubmission(bountyId: number | bigint, submissionIdx: number): Promise<BountyBoardSubmission | null> {
        const result = await this.callReadonly(
            'get_submission',
            encodeSubmissionArgs(normalizeUnsignedU64(bountyId, 'bountyId'), submissionIdx),
        );
        if ((result.returnCode ?? 0) === 1) {
            return null;
        }
        return decodeSubmission(result);
    }

    async getBountyTerms(bountyId: number | bigint): Promise<BountyBoardTerms | null> {
        const result = await this.callReadonly(
            'get_bounty_terms',
            encodeBountyIdArgs(normalizeUnsignedU64(bountyId, 'bountyId')),
        );
        if ((result.returnCode ?? 0) === 1) {
            return null;
        }
        return decodeBountyTerms(result);
    }

    async getPlatformFees(token: PublicKey | string): Promise<bigint> {
        const result = await this.callReadonly('get_platform_fees', encodeAddressArgs(token));
        ensureReadonlySuccess(result, 'get_platform_fees');
        if (!result.returnData) {
            throw new Error('BountyBoard get_platform_fees did not return data');
        }
        const bytes = decodeReturnData(result.returnData);
        if (bytes.length !== 8) {
            throw new Error('BountyBoard get_platform_fees payload must be exactly 8 bytes');
        }
        return readU64(bytes, 0);
    }

    async getAccountingMigrationStatus(): Promise<BountyBoardAccountingMigrationStatus> {
        return decodeAccountingMigrationStatus(await this.callReadonly('get_accounting_migration_status'));
    }

    async getAccountingHealth(): Promise<BountyBoardAccountingHealth> {
        return decodeAccountingHealth(await this.callReadonly('get_accounting_health'));
    }

    async getAdminTransition(): Promise<BountyBoardAdminTransition> {
        return decodeAdminTransition(await this.callReadonly('get_admin_transition'));
    }

    async getStats(): Promise<BountyBoardStats> {
        const stats = await this.connection.getBountyBoardStats();
        return {
            bountyCount: rpcU64(stats.bounty_count_exact, stats.bounty_count, 'bountyCount'),
            completedCount: rpcU64(stats.completed_count_exact, stats.completed_count, 'completedCount'),
            totalRewardVolume: rpcU64(
                stats.reward_volume_raw_exact,
                stats.reward_volume ?? stats.total_reward_volume,
                'totalRewardVolume',
            ),
            cancelCount: rpcU64(stats.cancel_count_exact, stats.cancel_count, 'cancelCount'),
            paused: Boolean(stats.paused),
        };
    }

    // --- Write methods ---

    async createBounty(creator: Keypair, params: CreateBountyParams): Promise<string> {
        const programId = await this.getProgramId();
        const rewardAmount = normalizeUnsignedU64(params.rewardAmount, 'rewardAmount');
        const deadlineSlot = normalizeUnsignedU64(params.deadlineSlot, 'deadlineSlot');
        const paymentValue = params.paymentValue === undefined
            ? rewardAmount
            : normalizeUnsignedU64(params.paymentValue, 'paymentValue');
        const args = encodeCreateBountyArgs(creator.pubkey(), params.titleHash, rewardAmount, deadlineSlot);
        return this.connection.callContract(creator, programId, 'create_bounty', args, paymentValue);
    }

    async submitWork(worker: Keypair, params: SubmitWorkParams): Promise<string> {
        const programId = await this.getProgramId();
        const bountyId = normalizeUnsignedU64(params.bountyId, 'bountyId');
        const args = encodeSubmitWorkArgs(bountyId, worker.pubkey(), params.proofHash);
        return this.connection.callContract(worker, programId, 'submit_work', args);
    }

    async approveWork(creator: Keypair, params: ApproveWorkParams): Promise<string> {
        const programId = await this.getProgramId();
        const bountyId = normalizeUnsignedU64(params.bountyId, 'bountyId');
        const args = encodeApproveWorkArgs(creator.pubkey(), bountyId, params.submissionIdx);
        return this.connection.callContract(creator, programId, 'approve_work', args);
    }

    async cancelBounty(creator: Keypair, bountyId: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const normalizedId = normalizeUnsignedU64(bountyId, 'bountyId');
        const args = encodeCancelBountyArgs(creator.pubkey(), normalizedId);
        return this.connection.callContract(creator, programId, 'cancel_bounty', args);
    }

    async updateWork(worker: Keypair, bountyId: number | bigint, submissionIdx: number, proofHash: Uint8Array): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUpdateWorkArgs(
            normalizeUnsignedU64(bountyId, 'bountyId'),
            submissionIdx,
            worker.pubkey(),
            proofHash,
        );
        return this.connection.callContract(worker, programId, 'update_work', args);
    }

    async initialize(admin: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(admin, programId, 'initialize', encodeAddressArgs(admin.pubkey()));
    }

    async setIdentityAdmin(admin: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(admin, programId, 'set_identity_admin', encodeAddressArgs(admin.pubkey()));
    }

    async proposeAdmin(admin: Keypair, newAdmin: PublicKey | string): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'propose_admin',
            encodeCallerAddressArgs(admin.pubkey(), newAdmin),
        );
    }

    async acceptAdmin(pendingAdmin: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            pendingAdmin,
            programId,
            'accept_admin',
            encodeAddressArgs(pendingAdmin.pubkey()),
        );
    }

    async cancelAdminProposal(admin: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'cancel_admin_proposal',
            encodeAddressArgs(admin.pubkey()),
        );
    }

    async setLichenIdAddress(admin: Keypair, lichenId: PublicKey | string): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'set_lichenid_address',
            encodeCallerAddressArgs(admin.pubkey(), lichenId),
        );
    }

    async setIdentityGate(admin: Keypair, minReputation: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'set_identity_gate',
            encodeCallerU64Args(admin.pubkey(), minReputation, 'minReputation'),
        );
    }

    async setTokenAddress(admin: Keypair, token: PublicKey | string): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'set_token_address',
            encodeCallerAddressArgs(admin.pubkey(), token),
        );
    }

    async setPlatformFee(admin: Keypair, feeBps: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'set_platform_fee',
            encodeCallerU64Args(admin.pubkey(), feeBps, 'feeBps'),
        );
    }

    async pause(admin: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(admin, programId, 'bb_pause', encodeAddressArgs(admin.pubkey()));
    }

    async unpause(admin: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(admin, programId, 'bb_unpause', encodeAddressArgs(admin.pubkey()));
    }

    async setFeeTreasury(admin: Keypair, treasury: PublicKey | string): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'set_fee_treasury',
            encodeCallerAddressArgs(admin.pubkey(), treasury),
        );
    }

    async withdrawPlatformFees(admin: Keypair, token: PublicKey | string, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'withdraw_platform_fees',
            encodeCallerAddressAmountArgs(admin.pubkey(), token, amount),
        );
    }

    async migrateBountyToken(
        admin: Keypair,
        bountyId: number | bigint,
        token: PublicKey | string,
    ): Promise<string> {
        const programId = await this.getProgramId();
        const args = buildLayoutArgs(
            [0x20, 0x08, 0x20],
            [
                admin.pubkey().toBytes(),
                u64LE(bountyId, 'bountyId'),
                normalizeAddress(token).toBytes(),
            ],
        );
        return this.connection.callContract(admin, programId, 'migrate_bounty_token', args);
    }

    async beginAccountingV2Migration(admin: Keypair, expectedBountyCount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'begin_accounting_v2_migration',
            encodeCallerU64Args(admin.pubkey(), expectedBountyCount, 'expectedBountyCount'),
        );
    }

    async migrateAccountingV2Bounty(caller: Keypair, bountyId: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            caller,
            programId,
            'migrate_accounting_v2_bounty',
            encodeBountyIdArgs(normalizeUnsignedU64(bountyId, 'bountyId')),
        );
    }

    async completeAccountingV2Migration(
        admin: Keypair,
        expectedEscrow: number | bigint,
        expectedPlatformFees: number | bigint,
        expectedTotalLiability: number | bigint,
    ): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            admin,
            programId,
            'complete_accounting_v2_migration',
            encodeMigrationCompletionArgs(
                admin.pubkey(),
                expectedEscrow,
                expectedPlatformFees,
                expectedTotalLiability,
            ),
        );
    }
}
