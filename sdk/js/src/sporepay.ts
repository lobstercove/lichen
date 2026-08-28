import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['SPOREPAY', 'sporepay'];
const STREAM_SIZE = 105;
const STREAM_INFO_SIZE = 113;
const MAX_U64 = (1n << 64n) - 1n;

export interface SporePayStream {
    streamId: bigint;
    sender: string;
    recipient: string;
    totalAmount: bigint;
    withdrawnAmount: bigint;
    startSlot: bigint;
    endSlot: bigint;
    cancelled: boolean;
    createdSlot: bigint;
}

export interface SporePayStreamInfo extends SporePayStream {
    cliffSlot: bigint;
}

export interface SporePayStreamIdPage {
    totalCount: bigint;
    nextCursor: bigint;
    streamIds: bigint[];
}

export interface SporePayStats {
    streamCount: number;
    totalStreamed: number;
    totalWithdrawn: number;
    cancelCount: number;
    escrowLiability: number;
    unpaidLiability: number;
    accountingVersion: number;
    migrationLocked: boolean;
    migrationExpectedStreams: number;
    migrationCursor: number;
    paused: boolean;
}

export interface CreateStreamParams {
    recipient: PublicKey | string;
    totalAmount: number | bigint;
    startSlot: number | bigint;
    endSlot: number | bigint;
}

export interface CreateStreamWithCliffParams extends CreateStreamParams {
    cliffSlot: number | bigint;
}

export interface WithdrawFromStreamParams {
    streamId: number | bigint;
    amount: number | bigint;
}

export interface TransferStreamParams {
    streamId: number | bigint;
    newRecipient: PublicKey | string;
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

function encodeCreateStreamArgs(sender: PublicKey, params: CreateStreamParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x20, 0x08, 0x08, 0x08], [
        sender.toBytes(),
        normalizeAddress(params.recipient).toBytes(),
        u64LE(params.totalAmount, 'totalAmount'),
        u64LE(params.startSlot, 'startSlot'),
        u64LE(params.endSlot, 'endSlot'),
    ]);
}

function encodeCreateStreamWithCliffArgs(sender: PublicKey, params: CreateStreamWithCliffParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x20, 0x08, 0x08, 0x08, 0x08], [
        sender.toBytes(),
        normalizeAddress(params.recipient).toBytes(),
        u64LE(params.totalAmount, 'totalAmount'),
        u64LE(params.startSlot, 'startSlot'),
        u64LE(params.endSlot, 'endSlot'),
        u64LE(params.cliffSlot, 'cliffSlot'),
    ]);
}

function encodeWithdrawArgs(caller: PublicKey, params: WithdrawFromStreamParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x08, 0x08], [
        caller.toBytes(),
        u64LE(params.streamId, 'streamId'),
        u64LE(params.amount, 'amount'),
    ]);
}

function encodeCancelArgs(caller: PublicKey, streamId: number | bigint): Uint8Array {
    return buildLayoutArgs([0x20, 0x08], [
        caller.toBytes(),
        u64LE(streamId, 'streamId'),
    ]);
}

function encodeTransferArgs(caller: PublicKey, params: TransferStreamParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x20, 0x08], [
        caller.toBytes(),
        normalizeAddress(params.newRecipient).toBytes(),
        u64LE(params.streamId, 'streamId'),
    ]);
}

function encodeStreamLookupArgs(streamId: number | bigint): Uint8Array {
    return buildLayoutArgs([0x08], [u64LE(streamId, 'streamId')]);
}

function encodeAddressArgs(address: PublicKey): Uint8Array {
    return buildLayoutArgs([0x20], [address.toBytes()]);
}

function encodeAddressPageArgs(
    address: PublicKey,
    cursor: number | bigint,
    limit: number | bigint,
): Uint8Array {
    return buildLayoutArgs([0x20, 0x08, 0x08], [
        address.toBytes(),
        u64LE(cursor, 'cursor'),
        u64LE(limit, 'limit'),
    ]);
}

function decodeReturnData(returnData: string): Uint8Array {
    return Uint8Array.from(Buffer.from(returnData, 'base64'));
}

function readU64(bytes: Uint8Array, offset: number): bigint {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return view.getBigUint64(offset, true);
}

function ensureReturnCodeZero(result: ReadonlyContractResult, functionName: string): void {
    const code = result.returnCode ?? 0;
    if (code !== 0) {
        throw new Error(result.error ?? `SporePay ${functionName} returned code ${code}`);
    }
    if (result.success === false && result.error) {
        throw new Error(result.error);
    }
}

function decodeStream(streamId: bigint, bytes: Uint8Array): SporePayStream {
    if (bytes.length < STREAM_SIZE) {
        throw new Error('SporePay stream payload was shorter than expected');
    }

    return {
        streamId,
        sender: PublicKey.fromBytes(bytes.slice(0, 32)).toBase58(),
        recipient: PublicKey.fromBytes(bytes.slice(32, 64)).toBase58(),
        totalAmount: readU64(bytes, 64),
        withdrawnAmount: readU64(bytes, 72),
        startSlot: readU64(bytes, 80),
        endSlot: readU64(bytes, 88),
        cancelled: bytes[96] === 1,
        createdSlot: readU64(bytes, 97),
    };
}

function decodeStreamInfo(streamId: bigint, bytes: Uint8Array): SporePayStreamInfo {
    if (bytes.length < STREAM_INFO_SIZE) {
        throw new Error('SporePay stream-info payload was shorter than expected');
    }

    return {
        ...decodeStream(streamId, bytes),
        cliffSlot: readU64(bytes, 105),
    };
}

function decodeStreamIdPage(bytes: Uint8Array): SporePayStreamIdPage {
    if (bytes.length < 24) {
        throw new Error('SporePay stream-ID page payload was shorter than expected');
    }
    const returned = readU64(bytes, 16);
    if (returned > BigInt(Number.MAX_SAFE_INTEGER)
        || bytes.length !== 24 + Number(returned) * 8) {
        throw new Error('SporePay stream-ID page payload length was inconsistent');
    }
    const streamIds: bigint[] = [];
    for (let index = 0; index < Number(returned); index += 1) {
        streamIds.push(readU64(bytes, 24 + index * 8));
    }
    return {
        totalCount: readU64(bytes, 0),
        nextCursor: readU64(bytes, 8),
        streamIds,
    };
}

export class SporePayClient {
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

        throw new Error('Unable to resolve the SporePay program via getSymbolRegistry("SPOREPAY")');
    }

    async getStream(streamId: number | bigint): Promise<SporePayStream | null> {
        const normalizedStreamId = normalizeUnsignedU64(streamId, 'streamId');
        const result = await this.callReadonly('get_stream', encodeStreamLookupArgs(normalizedStreamId));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) {
            return null;
        }
        ensureReturnCodeZero(result, 'get_stream');
        return decodeStream(normalizedStreamId, decodeReturnData(result.returnData));
    }

    async getStreamInfo(streamId: number | bigint): Promise<SporePayStreamInfo | null> {
        const normalizedStreamId = normalizeUnsignedU64(streamId, 'streamId');
        const result = await this.callReadonly('get_stream_info', encodeStreamLookupArgs(normalizedStreamId));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) {
            return null;
        }
        ensureReturnCodeZero(result, 'get_stream_info');
        return decodeStreamInfo(normalizedStreamId, decodeReturnData(result.returnData));
    }

    async getWithdrawable(streamId: number | bigint): Promise<bigint> {
        const result = await this.callReadonly('get_withdrawable', encodeStreamLookupArgs(streamId));
        ensureReturnCodeZero(result, 'get_withdrawable');
        if (!result.returnData) {
            throw new Error('SporePay get_withdrawable did not return a balance');
        }
        return readU64(decodeReturnData(result.returnData), 0);
    }

    async getStats(): Promise<SporePayStats> {
        const stats = await this.connection.getSporePayStats();
        return {
            streamCount: stats.stream_count ?? 0,
            totalStreamed: stats.total_streamed ?? 0,
            totalWithdrawn: stats.total_withdrawn ?? 0,
            cancelCount: stats.cancel_count ?? 0,
            escrowLiability: stats.escrow_liability ?? 0,
            unpaidLiability: stats.unpaid_liability ?? 0,
            accountingVersion: stats.accounting_version ?? 0,
            migrationLocked: Boolean(stats.migration_locked),
            migrationExpectedStreams: stats.migration_expected_streams ?? 0,
            migrationCursor: stats.migration_cursor ?? 0,
            paused: Boolean(stats.paused),
        };
    }

    async getUnpaidPayout(recipient: PublicKey | string): Promise<bigint> {
        const result = await this.callReadonly(
            'get_unpaid_payout',
            encodeAddressArgs(normalizeAddress(recipient)),
        );
        ensureReturnCodeZero(result, 'get_unpaid_payout');
        if (!result.returnData) {
            throw new Error('SporePay get_unpaid_payout did not return a balance');
        }
        return readU64(decodeReturnData(result.returnData), 0);
    }

    async getSenderStreamIds(
        sender: PublicKey | string,
        cursor: number | bigint = 0,
        limit: number | bigint = 64,
    ): Promise<SporePayStreamIdPage> {
        const result = await this.callReadonly(
            'get_sender_stream_ids',
            encodeAddressPageArgs(normalizeAddress(sender), cursor, limit),
        );
        ensureReturnCodeZero(result, 'get_sender_stream_ids');
        if (!result.returnData) {
            throw new Error('SporePay get_sender_stream_ids did not return a page');
        }
        return decodeStreamIdPage(decodeReturnData(result.returnData));
    }

    async getRecipientStreamIds(
        recipient: PublicKey | string,
        cursor: number | bigint = 0,
        limit: number | bigint = 64,
    ): Promise<SporePayStreamIdPage> {
        const result = await this.callReadonly(
            'get_recipient_stream_ids',
            encodeAddressPageArgs(normalizeAddress(recipient), cursor, limit),
        );
        ensureReturnCodeZero(result, 'get_recipient_stream_ids');
        if (!result.returnData) {
            throw new Error('SporePay get_recipient_stream_ids did not return a page');
        }
        return decodeStreamIdPage(decodeReturnData(result.returnData));
    }

    async createStream(sender: Keypair, params: CreateStreamParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeCreateStreamArgs(sender.pubkey(), params);
        return this.connection.callContract(sender, programId, 'create_stream', args);
    }

    async createStreamWithCliff(sender: Keypair, params: CreateStreamWithCliffParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeCreateStreamWithCliffArgs(sender.pubkey(), params);
        return this.connection.callContract(sender, programId, 'create_stream_with_cliff', args);
    }

    async withdrawFromStream(recipient: Keypair, params: WithdrawFromStreamParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeWithdrawArgs(recipient.pubkey(), params);
        return this.connection.callContract(recipient, programId, 'withdraw_from_stream', args);
    }

    async cancelStream(sender: Keypair, streamId: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeCancelArgs(sender.pubkey(), streamId);
        return this.connection.callContract(sender, programId, 'cancel_stream', args);
    }

    async transferStream(recipient: Keypair, params: TransferStreamParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeTransferArgs(recipient.pubkey(), params);
        return this.connection.callContract(recipient, programId, 'transfer_stream', args);
    }

    async claimUnpaidPayout(recipient: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(
            recipient,
            programId,
            'claim_unpaid_payout',
            encodeAddressArgs(recipient.pubkey()),
        );
    }
}
