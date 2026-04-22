import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['LEND', 'lend', 'THALLLEND', 'thalllend'];
const MAX_U64 = (1n << 64n) - 1n;

export interface ThallLendAccountInfo {
    deposit: bigint;
    borrow: bigint;
    healthFactorBps: bigint;
}

export interface ThallLendProtocolStats {
    totalDeposits: bigint;
    totalBorrows: bigint;
    utilizationPct: bigint;
    reserves: bigint;
}

export interface ThallLendInterestRate {
    ratePerSlot: bigint;
    utilizationPct: bigint;
    totalAvailable: bigint;
}

export interface ThallLendStats {
    totalDeposits: number;
    totalBorrows: number;
    reserves: number;
    depositCount: number;
    borrowCount: number;
    liquidationCount: number;
    paused: boolean;
}

export interface LiquidateParams {
    borrower: PublicKey | string;
    repayAmount: number | bigint;
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

function encodeUserAmountArgs(user: PublicKey, amount: number | bigint): Uint8Array {
    return buildLayoutArgs([0x20, 0x08], [
        user.toBytes(),
        u64LE(amount, 'amount'),
    ]);
}

function encodeUserLookupArgs(user: PublicKey | string): Uint8Array {
    return buildLayoutArgs([0x20], [normalizeAddress(user).toBytes()]);
}

function encodeLiquidateArgs(liquidator: PublicKey, params: LiquidateParams): Uint8Array {
    return buildLayoutArgs([0x20, 0x20, 0x08], [
        liquidator.toBytes(),
        normalizeAddress(params.borrower).toBytes(),
        u64LE(params.repayAmount, 'repayAmount'),
    ]);
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
        throw new Error(result.error ?? `ThallLend ${functionName} returned code ${code}`);
    }
    if (result.success === false && result.error) {
        throw new Error(result.error);
    }
}

function decodeU64Return(result: ReadonlyContractResult, functionName: string): bigint {
    ensureReadonlySuccess(result, functionName);
    if (!result.returnData) {
        throw new Error(`ThallLend ${functionName} did not return payload data`);
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 8) {
        throw new Error(`ThallLend ${functionName} payload was shorter than expected`);
    }
    return readU64(bytes, 0);
}

function decodeAccountInfo(result: ReadonlyContractResult): ThallLendAccountInfo {
    ensureReadonlySuccess(result, 'get_account_info');
    if (!result.returnData) {
        throw new Error('ThallLend get_account_info did not return account data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 24) {
        throw new Error('ThallLend get_account_info payload was shorter than expected');
    }
    return {
        deposit: readU64(bytes, 0),
        borrow: readU64(bytes, 8),
        healthFactorBps: readU64(bytes, 16),
    };
}

function decodeProtocolStats(result: ReadonlyContractResult): ThallLendProtocolStats {
    ensureReadonlySuccess(result, 'get_protocol_stats');
    if (!result.returnData) {
        throw new Error('ThallLend get_protocol_stats did not return stats data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 32) {
        throw new Error('ThallLend get_protocol_stats payload was shorter than expected');
    }
    return {
        totalDeposits: readU64(bytes, 0),
        totalBorrows: readU64(bytes, 8),
        utilizationPct: readU64(bytes, 16),
        reserves: readU64(bytes, 24),
    };
}

function decodeInterestRate(result: ReadonlyContractResult): ThallLendInterestRate {
    ensureReadonlySuccess(result, 'get_interest_rate');
    if (!result.returnData) {
        throw new Error('ThallLend get_interest_rate did not return rate data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 24) {
        throw new Error('ThallLend get_interest_rate payload was shorter than expected');
    }
    return {
        ratePerSlot: readU64(bytes, 0),
        utilizationPct: readU64(bytes, 8),
        totalAvailable: readU64(bytes, 16),
    };
}

export class ThallLendClient {
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

        throw new Error('Unable to resolve the ThallLend program via getSymbolRegistry("LEND")');
    }

    async getAccountInfo(user: PublicKey | string): Promise<ThallLendAccountInfo> {
        return decodeAccountInfo(await this.callReadonly('get_account_info', encodeUserLookupArgs(user)));
    }

    async getProtocolStats(): Promise<ThallLendProtocolStats> {
        return decodeProtocolStats(await this.callReadonly('get_protocol_stats'));
    }

    async getInterestRate(): Promise<ThallLendInterestRate> {
        return decodeInterestRate(await this.callReadonly('get_interest_rate'));
    }

    async getDepositCount(): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_deposit_count'), 'get_deposit_count');
    }

    async getBorrowCount(): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_borrow_count'), 'get_borrow_count');
    }

    async getLiquidationCount(): Promise<bigint> {
        return decodeU64Return(await this.callReadonly('get_liquidation_count'), 'get_liquidation_count');
    }

    async getStats(): Promise<ThallLendStats> {
        const stats = await this.connection.getThallLendStats();
        return {
            totalDeposits: stats.total_deposits ?? 0,
            totalBorrows: stats.total_borrows ?? 0,
            reserves: stats.reserves ?? 0,
            depositCount: stats.deposit_count ?? 0,
            borrowCount: stats.borrow_count ?? 0,
            liquidationCount: stats.liquidation_count ?? 0,
            paused: Boolean(stats.paused),
        };
    }

    async deposit(depositor: Keypair, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(depositor.pubkey(), amount);
        return this.connection.callContract(depositor, programId, 'deposit', args, normalizeUnsignedU64(amount, 'amount'));
    }

    async withdraw(depositor: Keypair, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(depositor.pubkey(), amount);
        return this.connection.callContract(depositor, programId, 'withdraw', args);
    }

    async borrow(borrower: Keypair, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(borrower.pubkey(), amount);
        return this.connection.callContract(borrower, programId, 'borrow', args);
    }

    async repay(borrower: Keypair, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(borrower.pubkey(), amount);
        return this.connection.callContract(borrower, programId, 'repay', args, normalizeUnsignedU64(amount, 'amount'));
    }

    async liquidate(liquidator: Keypair, params: LiquidateParams): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeLiquidateArgs(liquidator.pubkey(), params);
        return this.connection.callContract(
            liquidator,
            programId,
            'liquidate',
            args,
            normalizeUnsignedU64(params.repayAmount, 'repayAmount'),
        );
    }
}