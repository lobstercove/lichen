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

export interface SporeVaultStatus {
    accountingVersion: bigint;
    paused: boolean;
    licnConfigPresent: boolean;
    licnConfigValid: boolean;
    nativeLicn: boolean;
    thallLendConfigPresent: boolean;
    thallLendConfigValid: boolean;
    strategyRegistryValid: boolean;
    idleAssets: bigint;
    lendingAssets: bigint;
    totalAssets: bigint;
    totalShares: bigint;
    protocolFees: bigint;
    realLiquidCustody: bigint;
    custodyQueryOk: boolean;
    liquidCustodyCoversAccounting: boolean;
    depositFeeBps: bigint;
    withdrawalFeeBps: bigint;
    depositCap: bigint;
    riskTier: bigint;
    performanceFeePercent: bigint;
    managementFeeBps: bigint;
    targetSlotsPerYear: bigint;
}

export interface SporeVaultStats {
    totalAssets: number;
    totalShares: number;
    strategyCount: number;
    totalEarned: number;
    feesEarned: number;
    protocolFees: number;
    idleAssets: number;
    lendingAssets: number;
    accountingVersion: number;
    depositFeeBps: number;
    withdrawalFeeBps: number;
    depositCap: number;
    riskTier: number;
    activeLendingStrategies: number;
    lendingStrategyRows: number;
    strategyRegistryBounded: boolean;
    strategyRegistryValid: boolean;
    totalStrategyAllocation: number;
    nativeLicn: boolean;
    thallLendConfigValid: boolean;
    componentsMatchTotal: boolean;
    shareStateConsistent: boolean;
    liquidCustodyCoversAccounting: boolean;
    paused: boolean;
    operational: boolean;
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

function encodeAdminU64Args(admin: PublicKey, value: number | bigint, fieldName: string): Uint8Array {
    return buildLayoutArgs([0x20, 0x08], [admin.toBytes(), u64LE(value, fieldName)]);
}

function encodeAdminU8Args(admin: PublicKey, value: number, fieldName: string): Uint8Array {
    if (!Number.isInteger(value) || value < 0 || value > 0xff) {
        throw new Error(`${fieldName} must be a u8 integer value`);
    }
    return buildLayoutArgs([0x20, 0x01], [admin.toBytes(), Uint8Array.of(value)]);
}

function encodeAdminStrategyArgs(admin: PublicKey, strategyType: number, allocation: number | bigint): Uint8Array {
    if (!Number.isInteger(strategyType) || strategyType < 0 || strategyType > 0xff) {
        throw new Error('strategyType must be a u8 integer value');
    }
    return buildLayoutArgs(
        [0x20, 0x01, 0x08],
        [admin.toBytes(), Uint8Array.of(strategyType), u64LE(allocation, 'allocation')],
    );
}

function encodeAdminIndexValueArgs(admin: PublicKey, index: number | bigint, value: number | bigint): Uint8Array {
    return buildLayoutArgs(
        [0x20, 0x08, 0x08],
        [admin.toBytes(), u64LE(index, 'index'), u64LE(value, 'value')],
    );
}

function encodeAdminAddressArgs(admin: PublicKey, address: PublicKey): Uint8Array {
    return buildLayoutArgs([0x20, 0x20], [admin.toBytes(), address.toBytes()]);
}

function encodeProtocolAddressArgs(admin: PublicKey, thallLend: PublicKey, lichenSwap: PublicKey): Uint8Array {
    return buildLayoutArgs(
        [0x20, 0x20, 0x20],
        [admin.toBytes(), thallLend.toBytes(), lichenSwap.toBytes()],
    );
}

function encodeLegacyStrategyRetirementArgs(
    admin: PublicKey,
    index: number | bigint,
    expectedType: number,
    expectedAllocation: number | bigint,
    expectedDeployed: number | bigint,
): Uint8Array {
    if (!Number.isInteger(expectedType) || expectedType < 0 || expectedType > 0xff) {
        throw new Error('expectedType must be a u8 integer value');
    }
    return buildLayoutArgs(
        [0x20, 0x08, 0x01, 0x08, 0x08],
        [
            admin.toBytes(),
            u64LE(index, 'index'),
            Uint8Array.of(expectedType),
            u64LE(expectedAllocation, 'expectedAllocation'),
            u64LE(expectedDeployed, 'expectedDeployed'),
        ],
    );
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

function decodeVaultStatus(result: ReadonlyContractResult): SporeVaultStatus {
    ensureReadonlySuccess(result, 'get_vault_status');
    if (!result.returnData) {
        throw new Error('SporeVault get_vault_status did not return status data');
    }
    const bytes = decodeReturnData(result.returnData);
    if (bytes.length < 23 * 8) {
        throw new Error('SporeVault get_vault_status payload was shorter than expected');
    }
    const value = (index: number): bigint => readU64(bytes, index * 8);
    return {
        accountingVersion: value(0),
        paused: value(1) !== 0n,
        licnConfigPresent: value(2) !== 0n,
        licnConfigValid: value(3) !== 0n,
        nativeLicn: value(4) !== 0n,
        thallLendConfigPresent: value(5) !== 0n,
        thallLendConfigValid: value(6) !== 0n,
        strategyRegistryValid: value(7) !== 0n,
        idleAssets: value(8),
        lendingAssets: value(9),
        totalAssets: value(10),
        totalShares: value(11),
        protocolFees: value(12),
        realLiquidCustody: value(13),
        custodyQueryOk: value(14) !== 0n,
        liquidCustodyCoversAccounting: value(15) !== 0n,
        depositFeeBps: value(16),
        withdrawalFeeBps: value(17),
        depositCap: value(18),
        riskTier: value(19),
        performanceFeePercent: value(20),
        managementFeeBps: value(21),
        targetSlotsPerYear: value(22),
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

    async getVaultStatus(): Promise<SporeVaultStatus> {
        return decodeVaultStatus(await this.callReadonly('get_vault_status'));
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
            idleAssets: stats.idle_assets ?? 0,
            lendingAssets: stats.lending_assets ?? 0,
            accountingVersion: stats.accounting_version ?? 0,
            depositFeeBps: stats.deposit_fee_bps ?? 0,
            withdrawalFeeBps: stats.withdrawal_fee_bps ?? 0,
            depositCap: stats.deposit_cap ?? 0,
            riskTier: stats.risk_tier ?? 0,
            activeLendingStrategies: stats.active_lending_strategies ?? 0,
            lendingStrategyRows: stats.lending_strategy_rows ?? 0,
            strategyRegistryBounded: Boolean(stats.strategy_registry_bounded),
            strategyRegistryValid: Boolean(stats.strategy_registry_valid),
            totalStrategyAllocation: stats.total_strategy_allocation ?? 0,
            nativeLicn: Boolean(stats.native_licn),
            thallLendConfigValid: Boolean(stats.thalllend_config_valid),
            componentsMatchTotal: Boolean(stats.components_match_total),
            shareStateConsistent: Boolean(stats.share_state_consistent),
            liquidCustodyCoversAccounting: Boolean(stats.liquid_custody_covers_accounting),
            paused: Boolean(stats.paused),
            operational: Boolean(stats.operational),
        };
    }

    async deposit(depositor: Keypair, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(depositor.pubkey(), amount);
        return this.connection.callContract(depositor, programId, 'deposit', args, normalizeUnsignedU64(amount, 'amount'));
    }

    async depositMt20(depositor: Keypair, amount: number | bigint): Promise<string> {
        const programId = await this.getProgramId();
        const args = encodeUserAmountArgs(depositor.pubkey(), amount);
        return this.connection.callContract(depositor, programId, 'deposit', args);
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

    async rebalance(caller: Keypair): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(caller, programId, 'rebalance');
    }

    private async callAdmin(admin: Keypair, functionName: string, args: Uint8Array): Promise<string> {
        const programId = await this.getProgramId();
        return this.connection.callContract(admin, programId, functionName, args);
    }

    async pause(admin: Keypair): Promise<string> {
        return this.callAdmin(admin, 'cv_pause', buildLayoutArgs([0x20], [admin.pubkey().toBytes()]));
    }

    async unpause(admin: Keypair): Promise<string> {
        return this.callAdmin(admin, 'cv_unpause', buildLayoutArgs([0x20], [admin.pubkey().toBytes()]));
    }

    async setDepositFee(admin: Keypair, feeBps: number | bigint): Promise<string> {
        return this.callAdmin(admin, 'set_deposit_fee', encodeAdminU64Args(admin.pubkey(), feeBps, 'feeBps'));
    }

    async setWithdrawalFee(admin: Keypair, feeBps: number | bigint): Promise<string> {
        return this.callAdmin(admin, 'set_withdrawal_fee', encodeAdminU64Args(admin.pubkey(), feeBps, 'feeBps'));
    }

    async setDepositCap(admin: Keypair, cap: number | bigint): Promise<string> {
        return this.callAdmin(admin, 'set_deposit_cap', encodeAdminU64Args(admin.pubkey(), cap, 'cap'));
    }

    async setRiskTier(admin: Keypair, tier: number): Promise<string> {
        return this.callAdmin(admin, 'set_risk_tier', encodeAdminU8Args(admin.pubkey(), tier, 'tier'));
    }

    async addStrategy(admin: Keypair, strategyType: number, allocationPercent: number | bigint): Promise<string> {
        return this.callAdmin(
            admin,
            'add_strategy',
            encodeAdminStrategyArgs(admin.pubkey(), strategyType, allocationPercent),
        );
    }

    async removeStrategy(admin: Keypair, index: number | bigint): Promise<string> {
        return this.callAdmin(admin, 'remove_strategy', encodeAdminU64Args(admin.pubkey(), index, 'index'));
    }

    async updateStrategyAllocation(
        admin: Keypair,
        index: number | bigint,
        allocationPercent: number | bigint,
    ): Promise<string> {
        return this.callAdmin(
            admin,
            'update_strategy_allocation',
            encodeAdminIndexValueArgs(admin.pubkey(), index, allocationPercent),
        );
    }

    async withdrawProtocolFees(admin: Keypair): Promise<string> {
        return this.callAdmin(
            admin,
            'withdraw_protocol_fees',
            buildLayoutArgs([0x20], [admin.pubkey().toBytes()]),
        );
    }

    async setProtocolAddresses(
        admin: Keypair,
        thallLend: PublicKey | string,
        lichenSwap: PublicKey | string = new PublicKey(new Uint8Array(32)),
    ): Promise<string> {
        return this.callAdmin(
            admin,
            'set_protocol_addresses',
            encodeProtocolAddressArgs(admin.pubkey(), normalizeAddress(thallLend), normalizeAddress(lichenSwap)),
        );
    }

    async setLicnToken(admin: Keypair, token: PublicKey | string): Promise<string> {
        return this.callAdmin(
            admin,
            'set_licn_token',
            encodeAdminAddressArgs(admin.pubkey(), normalizeAddress(token)),
        );
    }

    async migrateAccountingV2(
        admin: Keypair,
        expectedIdleAssets: number | bigint,
        expectedLendingAssets: number | bigint,
    ): Promise<string> {
        return this.callAdmin(
            admin,
            'migrate_accounting_v2',
            encodeAdminIndexValueArgs(admin.pubkey(), expectedIdleAssets, expectedLendingAssets),
        );
    }

    async retireLegacyStrategy(
        admin: Keypair,
        index: number | bigint,
        expectedType: number,
        expectedAllocation: number | bigint,
        expectedDeployed: number | bigint,
    ): Promise<string> {
        return this.callAdmin(
            admin,
            'retire_legacy_strategy',
            encodeLegacyStrategyRetirementArgs(
                admin.pubkey(),
                index,
                expectedType,
                expectedAllocation,
                expectedDeployed,
            ),
        );
    }
}
