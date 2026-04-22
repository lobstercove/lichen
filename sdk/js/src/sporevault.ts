import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['SPOREVAULT', 'sporevault', 'SporeVault', 'VAULT', 'vault'];
const MAX_U64 = (1n << 64n) - 1n;

export interface SporeVaultVaultStats {
    totalAssets: bigint;
    totalShares: bigint;
    sharePriceE9: bigint;
    strategyCount: bigint;
    totalEarned: bigint;
    feesEarned: bigint;
}

export interface SporeVaultUserPosition {
    shares: bigint;
    estimatedValue: bigint;
}

export interface SporeVaultStrategyInfo {
    strategyType: bigint;
    allocationPercent: bigint;
    deployedAmount: bigint;
}

export interface SporeVaultStats {
    totalAssets: number;
    totalShares: number;
    strategyCount: number;
    totalEarned: number;
    feesEarned: number;
    protocolFees: number;
    paused: boolean;
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

function encodeIndexArgs(index: number | bigint): Uint8Array {
    return buildLayoutArgs([0x08], [u64LE(index, 'index')]);
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
        throw new Error(result.error ?? `SporeVault ${functionName} returned code ${code}`);
    }
    if (result.success === false && result.error) {
        throw new Error(result.error);
    }
}

function decodeVaultStats(result: ReadonlyContractResult): SporeVaultVaultStats {
    ensureReadonlySuccess(result, 'get_vault_stats');
    if (!result.returnData) {
        throw new Error('SporeVault get_vault_stats did not return vault data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 48) {
        throw new Error('SporeVault get_vault_stats payload was shorter than expected');
    }
    return {
        totalAssets: readU64(bytes, 0),
        totalShares: readU64(bytes, 8),
        sharePriceE9: readU64(bytes, 16),
        strategyCount: readU64(bytes, 24),
        totalEarned: readU64(bytes, 32),
        feesEarned: readU64(bytes, 40),
    };
}

function decodeUserPosition(result: ReadonlyContractResult): SporeVaultUserPosition {
    ensureReadonlySuccess(result, 'get_user_position');
    if (!result.returnData) {
        throw new Error('SporeVault get_user_position did not return user data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 16) {
        throw new Error('SporeVault get_user_position payload was shorter than expected');
    }
    return {
        shares: readU64(bytes, 0),
        estimatedValue: readU64(bytes, 8),
    };
}

function decodeStrategyInfo(result: ReadonlyContractResult): SporeVaultStrategyInfo {
    ensureReadonlySuccess(result, 'get_strategy_info');
    if (!result.returnData) {
        throw new Error('SporeVault get_strategy_info did not return strategy data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 24) {
        throw new Error('SporeVault get_strategy_info payload was shorter than expected');
    }
    return {
        strategyType: readU64(bytes, 0),
        allocationPercent: readU64(bytes, 8),
        deployedAmount: readU64(bytes, 16),
    };
}

export class SporeVaultClient {
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

        throw new Error('Unable to resolve the SporeVault program via getSymbolRegistry("SPOREVAULT")');
    }

    async getVaultStats(): Promise<SporeVaultVaultStats> {
        return decodeVaultStats(await this.callReadonly('get_vault_stats'));
    }

    async getUserPosition(user: PublicKey | string): Promise<SporeVaultUserPosition> {
        return decodeUserPosition(await this.callReadonly('get_user_position', encodeUserLookupArgs(user)));
    }

    async getStrategyInfo(index: number | bigint): Promise<SporeVaultStrategyInfo | null> {
        const result = await this.callReadonly('get_strategy_info', encodeIndexArgs(index));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) {
            return null;
        }
        return decodeStrategyInfo(result);
    }

    async getStats(): Promise<SporeVaultStats> {
        const stats = await this.connection.getSporeVaultStats();
        return {
            totalAssets: stats.total_assets ?? 0,
            totalShares: stats.total_shares ?? 0,
            strategyCount: stats.strategy_count ?? 0,
            totalEarned: stats.total_earned ?? 0,
            feesEarned: stats.fees_earned ?? 0,
            protocolFees: stats.protocol_fees ?? 0,
            paused: Boolean(stats.paused),
        };
    }

    async deposit(depositor: Keypair, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(depositor.pubkey(), amount);
        return this.connection.callContract(depositor, programId, 'deposit', args, normalizeUnsignedU64(amount, 'amount'));
    }

    async withdraw(depositor: Keypair, sharesToBurn: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(depositor.pubkey(), sharesToBurn);
        return this.connection.callContract(depositor, programId, 'withdraw', args);
    }

    async harvest(caller: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(caller, programId, 'harvest');
    }
}