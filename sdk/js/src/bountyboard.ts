import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['BOUNTY', 'bounty', 'BountyBoard', 'BOUNTYBOARD', 'bountyboard'];
const MAX_U64 = (1n << 64n) - 1n;
const BOUNTY_DATA_SIZE = 91;
const PLATFORM_STATS_SIZE = 32;

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

export interface BountyBoardStats {
    bountyCount: number;
    completedCount: number;
    totalRewardVolume: number;
    cancelCount: number;
    paused: boolean;
}

export interface CreateBountyParams {
    titleHash: Uint8Array;
    rewardAmount: number | bigint;
    deadlineSlot: number | bigint;
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

function ensureReadonlySuccess(
    result: ReadonlyContractResult,
    functionName: string,
    allowedReturnCodes: number[] = [0],
): void {
    const code = result.returnCode ?? 0;
    if (!allowedReturnCodes.includes(code)) {
        throw new Error(result.error ?? `BountyBoard ${functionName} returned code ${code}`);
    }
    if (result.success === false && result.error) {
        throw new Error(result.error);
    }
}

function ensureBytes32(value: Uint8Array, fieldName: string): Uint8Array {
    if (value.length !== 32) {
        throw new Error(`${fieldName} must be exactly 32 bytes`);
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

// --- Decoding helpers ---

function decodeBountyInfo(result: ReadonlyContractResult): BountyBoardBountyInfo {
    ensureReadonlySuccess(result, 'get_bounty');
    if (!result.returnData) {
        throw new Error('BountyBoard get_bounty did not return bounty data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < BOUNTY_DATA_SIZE) {
        throw new Error('BountyBoard get_bounty payload was shorter than expected');
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
    if (bytes.length < PLATFORM_STATS_SIZE) {
        throw new Error('BountyBoard get_platform_stats payload was shorter than expected');
    }
    return {
        bountyCount: readU64(bytes, 0),
        completedCount: readU64(bytes, 8),
        rewardVolume: readU64(bytes, 16),
        cancelCount: readU64(bytes, 24),
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
        if ((result.returnCode ?? 0) === 1 || !result.returnData) {
            return null;
        }
        return decodeBountyInfo(result);
    }

    async getBountyCount(): Promise<bigint> {
        const result = await this.callReadonly('get_bounty_count');
        ensureReadonlySuccess(result, 'get_bounty_count');
        if (!result.returnData) {
            return 0n;
        }
        const bytes = decodeReturnData(result.returnData);
        if (bytes.length < 8) {
            return 0n;
        }
        return readU64(bytes, 0);
    }

    async getPlatformStats(): Promise<BountyBoardPlatformStats> {
        return decodePlatformStats(await this.callReadonly('get_platform_stats'));
    }

    async getStats(): Promise<BountyBoardStats> {
        const stats = await this.connection.getBountyBoardStats();
        return {
            bountyCount: stats.bounty_count ?? 0,
            completedCount: stats.completed_count ?? 0,
            totalRewardVolume: stats.total_reward_volume ?? 0,
            cancelCount: stats.cancel_count ?? 0,
            paused: Boolean(stats.paused),
        };
    }

    // --- Write methods ---

    async createBounty(creator: Keypair, params: CreateBountyParams): Promise<string> {
        const programId = await this.getProgramId();
        const rewardAmount = normalizeUnsignedU64(params.rewardAmount, 'rewardAmount');
        const deadlineSlot = normalizeUnsignedU64(params.deadlineSlot, 'deadlineSlot');
        const args = encodeCreateBountyArgs(creator.pubkey(), params.titleHash, rewardAmount, deadlineSlot);
        return this.connection.callContract(creator, programId, 'create_bounty', args, rewardAmount);
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
}
