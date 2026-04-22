import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['LICHENSWAP', 'lichenswap'];
const MAX_U64 = (1n << 64n) - 1n;

export interface LichenSwapPoolInfo {
    reserveA: bigint;
    reserveB: bigint;
    totalLiquidity: bigint;
}

export interface LichenSwapVolumeTotals {
    volumeA: bigint;
    volumeB: bigint;
}

export interface LichenSwapProtocolFees {
    feesA: bigint;
    feesB: bigint;
}

export interface LichenSwapTwapCumulatives {
    cumulativePriceA: bigint;
    cumulativePriceB: bigint;
    lastUpdatedAt: bigint;
}

export interface LichenSwapSwapStats {
    swapCount: bigint;
    volumeA: bigint;
    volumeB: bigint;
    poolCount: bigint;
    totalLiquidity: bigint;
}

export interface LichenSwapStats {
    swapCount: number;
    volumeA: number;
    volumeB: number;
    paused: boolean;
}

export interface CreatePoolParams {
    tokenA: PublicKey | string;
    tokenB: PublicKey | string;
}

export interface AddLiquidityParams {
    amountA: number | bigint;
    amountB: number | bigint;
    minLiquidity?: number | bigint;
    valueSpores?: number | bigint;
}

export interface SwapParams {
    amountIn: number | bigint;
    minAmountOut?: number | bigint;
    valueSpores?: number | bigint;
}

export interface SwapWithDeadlineParams extends SwapParams {
    deadline: number | bigint;
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

function addUnsignedU64(left: number | bigint, right: number | bigint, fieldName: string): bigint {
    const sum = normalizeUnsignedU64(left, fieldName) + normalizeUnsignedU64(right, fieldName);
    if (sum > MAX_U64) {
        throw new Error(`${fieldName} must be a u64-safe integer value`);
    }
    return sum;
}

function u32LE(value: number): Uint8Array {
    if (!Number.isInteger(value) || value < 0 || value > 0xFFFF_FFFF) {
        throw new Error('u32 values must fit within 0 to 4,294,967,295');
    }
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

function encodeCreatePoolArgs(params: CreatePoolParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x20], [
        normalizeAddress(params.tokenA).toBytes(),
        normalizeAddress(params.tokenB).toBytes(),
    ]);
}

function encodeAddLiquidityArgs(provider: PublicKey, params: AddLiquidityParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x08, 0x08, 0x08], [
        provider.toBytes(),
        u64LE(params.amountA, 'amountA'),
        u64LE(params.amountB, 'amountB'),
        u64LE(params.minLiquidity ?? 0, 'minLiquidity'),
    ]);
}

function encodeSwapArgs(params: SwapParams, aToB: boolean): Uint8Array {
    return buildLayoutArgs([0x08, 0x08, 0x04], [
        u64LE(params.amountIn, 'amountIn'),
        u64LE(params.minAmountOut ?? 0, 'minAmountOut'),
        u32LE(aToB ? 1 : 0),
    ]);
}

function encodeDirectionalSwapArgs(params: SwapParams): Uint8Array {
    return buildLayoutArgs([0x08, 0x08], [
        u64LE(params.amountIn, 'amountIn'),
        u64LE(params.minAmountOut ?? 0, 'minAmountOut'),
    ]);
}

function encodeDirectionalSwapWithDeadlineArgs(params: SwapWithDeadlineParams): Uint8Array {
    return buildLayoutArgs([0x08, 0x08, 0x08], [
        u64LE(params.amountIn, 'amountIn'),
        u64LE(params.minAmountOut ?? 0, 'minAmountOut'),
        u64LE(params.deadline, 'deadline'),
    ]);
}

function encodeQuoteArgs(amountIn: number | bigint, aToB: boolean): Uint8Array {
    return buildLayoutArgs([0x08, 0x04], [
        u64LE(amountIn, 'amountIn'),
        u32LE(aToB ? 1 : 0),
    ]);
}

function encodeLiquidityBalanceArgs(provider: PublicKey): Uint8Array {
    return buildLayoutArgs([0x20], [provider.toBytes()]);
}

function encodeAmountArgs(amount: number | bigint, fieldName: string): Uint8Array {
    return buildLayoutArgs([0x08], [u64LE(amount, fieldName)]);
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
        throw new Error(result.error ?? `LichenSwap ${functionName} returned code ${code}`);
    }
    if (result.success === false && result.error) {
        throw new Error(result.error);
    }
}

function decodeU64Return(result: ReadonlyContractResult, functionName: string): bigint {
    ensureReadonlySuccess(result, functionName);
    if (!result.returnData) {
        throw new Error(`LichenSwap ${functionName} did not return payload data`);
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 8) {
        throw new Error(`LichenSwap ${functionName} payload was shorter than expected`);
    }
    return readU64(bytes, 0);
}

function decodePoolInfo(result: ReadonlyContractResult): LichenSwapPoolInfo {
    ensureReadonlySuccess(result, 'get_pool_info', [0, 1]);
    if (!result.returnData) {
        throw new Error('LichenSwap get_pool_info did not return pool data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 24) {
        throw new Error('LichenSwap get_pool_info payload was shorter than expected');
    }
    return {
        reserveA: readU64(bytes, 0),
        reserveB: readU64(bytes, 8),
        totalLiquidity: readU64(bytes, 16),
    };
}

function decodeVolumeTotals(result: ReadonlyContractResult, functionName: string): LichenSwapVolumeTotals {
    ensureReadonlySuccess(result, functionName);
    if (!result.returnData) {
        throw new Error(`LichenSwap ${functionName} did not return volume data`);
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 16) {
        throw new Error(`LichenSwap ${functionName} payload was shorter than expected`);
    }
    return {
        volumeA: readU64(bytes, 0),
        volumeB: readU64(bytes, 8),
    };
}

function decodeProtocolFees(result: ReadonlyContractResult): LichenSwapProtocolFees {
    ensureReadonlySuccess(result, 'get_protocol_fees');
    if (!result.returnData) {
        throw new Error('LichenSwap get_protocol_fees did not return fee data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 16) {
        throw new Error('LichenSwap get_protocol_fees payload was shorter than expected');
    }
    return {
        feesA: readU64(bytes, 0),
        feesB: readU64(bytes, 8),
    };
}

function decodeTwapCumulatives(result: ReadonlyContractResult): LichenSwapTwapCumulatives {
    ensureReadonlySuccess(result, 'get_twap_cumulatives');
    if (!result.returnData) {
        throw new Error('LichenSwap get_twap_cumulatives did not return TWAP data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 24) {
        throw new Error('LichenSwap get_twap_cumulatives payload was shorter than expected');
    }
    return {
        cumulativePriceA: readU64(bytes, 0),
        cumulativePriceB: readU64(bytes, 8),
        lastUpdatedAt: readU64(bytes, 16),
    };
}

function decodeSwapStats(result: ReadonlyContractResult): LichenSwapSwapStats {
    ensureReadonlySuccess(result, 'get_swap_stats');
    if (!result.returnData) {
        throw new Error('LichenSwap get_swap_stats did not return stats data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 40) {
        throw new Error('LichenSwap get_swap_stats payload was shorter than expected');
    }
    return {
        swapCount: readU64(bytes, 0),
        volumeA: readU64(bytes, 8),
        volumeB: readU64(bytes, 16),
        poolCount: readU64(bytes, 24),
        totalLiquidity: readU64(bytes, 32),
    };
}

export class LichenSwapClient {
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

        throw new Error('Unable to resolve the LichenSwap program via getSymbolRegistry("LICHENSWAP")');
    }

    async getPoolInfo(): Promise<LichenSwapPoolInfo> {
        return decodePoolInfo(await this.callReadonly('get_pool_info'));
    }

    async getQuote(amountIn: number | bigint, aToB: boolean = true): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_quote', encodeQuoteArgs(amountIn, aToB)), 'get_quote');
    }

    async getLiquidityBalance(provider: PublicKey | string): Promise<bigint> {
        return decodeU64Return(
            await this.callReadonly('get_liquidity_balance', encodeLiquidityBalanceArgs(normalizeAddress(provider))),
            'get_liquidity_balance',
        );
    }

    async getTotalLiquidity(): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_total_liquidity'), 'get_total_liquidity');
    }

    async getFlashLoanFee(amount: number | bigint): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_flash_loan_fee', encodeAmountArgs(amount, 'amount')), 'get_flash_loan_fee');
    }

    async getTwapCumulatives(): Promise<LichenSwapTwapCumulatives> {
        return decodeTwapCumulatives(await this.callReadonly('get_twap_cumulatives'));
    }

    async getTwapSnapshotCount(): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_twap_snapshot_count'), 'get_twap_snapshot_count');
    }

    async getProtocolFees(): Promise<LichenSwapProtocolFees> {
        return decodeProtocolFees(await this.callReadonly('get_protocol_fees'));
    }

    async getPoolCount(): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_pool_count'), 'get_pool_count');
    }

    async getSwapCount(): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_swap_count'), 'get_swap_count');
    }

    async getTotalVolume(): Promise<LichenSwapVolumeTotals> {
        return decodeVolumeTotals(await this.callReadonly('get_total_volume'), 'get_total_volume');
    }

    async getSwapStats(): Promise<LichenSwapSwapStats> {
        return decodeSwapStats(await this.callReadonly('get_swap_stats'));
    }

    async getStats(): Promise<LichenSwapStats> {
        const stats = await this.connection.getLichenSwapStats();
        return {
            swapCount: stats.swap_count ?? 0,
            volumeA: stats.volume_a ?? 0,
            volumeB: stats.volume_b ?? 0,
            paused: Boolean(stats.paused),
        };
    }

    async createPool(owner: Keypair, params: CreatePoolParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeCreatePoolArgs(params);
        return this.connection.callContract(owner, programId, 'create_pool', args);
    }

    async addLiquidity(provider: Keypair, params: AddLiquidityParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeAddLiquidityArgs(provider.pubkey(), params);
        const value = params.valueSpores ?? addUnsignedU64(params.amountA, params.amountB, 'valueSpores');
        return this.connection.callContract(provider, programId, 'add_liquidity', args, value);
    }

    async swap(provider: Keypair, params: SwapParams, aToB: boolean = true): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeSwapArgs(params, aToB);
        const value = params.valueSpores ?? normalizeUnsignedU64(params.amountIn, 'valueSpores');
        return this.connection.callContract(provider, programId, 'swap', args, value);
    }

    async swapAToB(provider: Keypair, params: SwapParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDirectionalSwapArgs(params);
        const value = params.valueSpores ?? normalizeUnsignedU64(params.amountIn, 'valueSpores');
        return this.connection.callContract(provider, programId, 'swap_a_for_b', args, value);
    }

    async swapBToA(provider: Keypair, params: SwapParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDirectionalSwapArgs(params);
        const value = params.valueSpores ?? normalizeUnsignedU64(params.amountIn, 'valueSpores');
        return this.connection.callContract(provider, programId, 'swap_b_for_a', args, value);
    }

    async swapAToBWithDeadline(provider: Keypair, params: SwapWithDeadlineParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDirectionalSwapWithDeadlineArgs(params);
        const value = params.valueSpores ?? normalizeUnsignedU64(params.amountIn, 'valueSpores');
        return this.connection.callContract(provider, programId, 'swap_a_for_b_with_deadline', args, value);
    }

    async swapBToAWithDeadline(provider: Keypair, params: SwapWithDeadlineParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeDirectionalSwapWithDeadlineArgs(params);
        const value = params.valueSpores ?? normalizeUnsignedU64(params.amountIn, 'valueSpores');
        return this.connection.callContract(provider, programId, 'swap_b_for_a_with_deadline', args, value);
    }
}