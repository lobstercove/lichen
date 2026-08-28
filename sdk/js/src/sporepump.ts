import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['SPOREPUMP', 'sporepump'];
export const SPOREPUMP_CREATION_FEE = 10_000_000_000n;
const MAX_U64 = (1n << 64n) - 1n;
const PLATFORM_STATS_SIZE = 88;
const GRADUATION_STATUS_SIZE = 113;
const ACCOUNTING_MIGRATION_TOKEN_SIZE = 73;

export interface SporePumpTokenInfo {
    supplySold: bigint;
    licnRaised: bigint;
    currentPrice: bigint;
    marketCap: bigint;
    graduated: boolean;
}

export interface SporePumpTokenMetadata {
    name: string;
    symbol: string;
}

export interface SporePumpPlatformStats {
    tokenCount: bigint;
    platformFees: bigint;
    curveReserve: bigint;
    creatorLiability: bigint;
    cumulativeGraduationRevenue: bigint;
    graduatedCount: bigint;
    accountingVersion: bigint;
    migrationExpected: bigint;
    migrationCursor: bigint;
    migrationLocked: boolean;
    creatorRoyaltyBps: bigint;
}

export interface SporePumpCustodyStatus {
    balance: bigint;
    obligations: bigint;
    recoverableSurplus: bigint;
}

export interface SporePumpAccountingMigrationToken {
    creator: string;
    supplySold: bigint;
    licnRaised: bigint;
    maxSupply: bigint;
    createdSlot: bigint;
    lifecycleState: number;
    creatorRoyalty: bigint;
}

export interface SporePumpGraduationStatus {
    state: number;
    eligibilitySlot: bigint;
    migrationBoundarySlot: bigint;
    candidate: string | null;
    pairId: bigint;
    poolId: bigint;
    forwardRouteId: bigint;
    reverseRouteId: bigint;
    positionId: bigint;
    licnLiquidity: bigint;
    tokenLiquidity: bigint;
    protocolTokenInventory: bigint;
}

export interface SporePumpGraduationInfo {
    cumulativeRevenue: bigint;
    dexCoreConfigured: boolean;
    dexAmmConfigured: boolean;
    dexRouterConfigured: boolean;
    tokenTemplateConfigured: boolean;
    governanceConfigured: boolean;
    accountingReady: boolean;
    tickSize: bigint;
    lotSize: bigint;
    minimumOrder: bigint;
    ammFeeTier: bigint;
}

export interface CreateSporePumpTokenParams {
    name: string;
    symbol: string;
}

export interface SporePumpGraduationConfig {
    router: PublicKey | string;
    tokenTemplateHash: PublicKey | string;
    tickSize: number | bigint;
    lotSize: number | bigint;
    minimumOrder: number | bigint;
    ammFeeTier: number;
}

function normalizeAddress(value: PublicKey | string): PublicKey {
    return value instanceof PublicKey ? value : new PublicKey(value);
}

function normalizeU64(value: number | bigint, fieldName: string): bigint {
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
    new DataView(out.buffer).setBigUint64(0, normalizeU64(value, fieldName), true);
    return out;
}

function u32LE(value: number, fieldName: string): Uint8Array {
    if (!Number.isInteger(value) || value < 0 || value > 0xFFFF_FFFF) {
        throw new Error(`${fieldName} must be a u32 integer`);
    }
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, value, true);
    return out;
}

function layoutArgs(layout: number[], chunks: Uint8Array[]): Uint8Array {
    if (layout.some((size) => !Number.isInteger(size) || size < 0 || size > 255)) {
        throw new Error('SporePump ABI stride exceeded one byte');
    }
    const header = Uint8Array.from([0xAB, ...layout]);
    const out = new Uint8Array(header.length + chunks.reduce((sum, chunk) => sum + chunk.length, 0));
    out.set(header);
    let offset = header.length;
    for (const chunk of chunks) {
        out.set(chunk, offset);
        offset += chunk.length;
    }
    return out;
}

function fixedArgs(...chunks: Uint8Array[]): Uint8Array {
    return layoutArgs(chunks.map((chunk) => chunk.length), chunks);
}

function decodeReturnData(result: ReadonlyContractResult, functionName: string): Uint8Array {
    if (result.success === false) {
        throw new Error(result.error ?? `SporePump ${functionName} failed`);
    }
    if (!result.returnData) {
        throw new Error(`SporePump ${functionName} did not return payload data`);
    }
    return Uint8Array.from(Buffer.from(result.returnData, 'base64'));
}

function ensureCodeSuccess(result: ReadonlyContractResult, functionName: string): Uint8Array {
    const code = result.returnCode ?? 0;
    if (code !== 0 || result.success === false) {
        throw new Error(result.error ?? `SporePump ${functionName} returned code ${code}`);
    }
    return decodeReturnData(result, functionName);
}

function readU64(bytes: Uint8Array, offset = 0): bigint {
    if (offset < 0 || bytes.length < offset + 8) {
        throw new Error('SporePump payload was shorter than expected');
    }
    return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getBigUint64(offset, true);
}

function decodeU64Value(result: ReadonlyContractResult, functionName: string): bigint {
    const bytes = decodeReturnData(result, functionName);
    if (bytes.length !== 8) {
        throw new Error(`SporePump ${functionName} returned a non-u64 payload`);
    }
    return readU64(bytes);
}

function validateMetadata(params: CreateSporePumpTokenParams): { name: Uint8Array; symbol: Uint8Array } {
    const encoder = new TextEncoder();
    const nameValue = params.name.trim();
    const symbolValue = params.symbol.trim().toUpperCase();
    const name = encoder.encode(nameValue);
    const symbol = encoder.encode(symbolValue);
    if (!nameValue || name.length > 64 || /[\u0000-\u001f\u007f]/u.test(nameValue)) {
        throw new Error('name must be 1-64 UTF-8 bytes without control characters');
    }
    if (!/^[A-Z][A-Z0-9]{1,11}$/u.test(symbolValue)) {
        throw new Error('symbol must contain 2-12 ASCII alphanumeric characters and start with a letter');
    }
    return { name, symbol };
}

function metadataArgs(creator: PublicKey, params: CreateSporePumpTokenParams): Uint8Array {
    const { name, symbol } = validateMetadata(params);
    const nameStride = Math.max(32, name.length);
    const symbolStride = Math.max(32, symbol.length);
    const paddedName = new Uint8Array(nameStride);
    const paddedSymbol = new Uint8Array(symbolStride);
    paddedName.set(name);
    paddedSymbol.set(symbol);
    return layoutArgs([32, nameStride, 4, symbolStride, 4, 8], [
        creator.toBytes(),
        paddedName,
        u32LE(name.length, 'name length'),
        paddedSymbol,
        u32LE(symbol.length, 'symbol length'),
        u64LE(SPOREPUMP_CREATION_FEE, 'creation fee'),
    ]);
}

export class SporePumpClient {
    private resolvedProgram?: PublicKey;

    constructor(private readonly connection: Connection, programId?: PublicKey) {
        this.resolvedProgram = programId;
    }

    async getProgramId(): Promise<PublicKey> {
        if (this.resolvedProgram) return this.resolvedProgram;
        for (const symbol of PROGRAM_SYMBOL_CANDIDATES) {
            try {
                const entry = await this.connection.getSymbolRegistry(symbol);
                if (entry?.program) {
                    this.resolvedProgram = new PublicKey(entry.program);
                    return this.resolvedProgram;
                }
            } catch {
                // Try the canonical lowercase alias.
            }
        }
        throw new Error('Unable to resolve SporePump via getSymbolRegistry("SPOREPUMP")');
    }

    private async readonly(
        functionName: string,
        args: Uint8Array = new Uint8Array(),
    ): Promise<ReadonlyContractResult> {
        return this.connection.callReadonlyContract(await this.getProgramId(), functionName, args);
    }

    private async write(
        signer: Keypair,
        functionName: string,
        args: Uint8Array,
        value: number | bigint = 0n,
    ): Promise<string> {
        return this.connection.callContract(
            signer,
            await this.getProgramId(),
            functionName,
            args,
            normalizeU64(value, 'value'),
        );
    }

    async getTokenInfo(tokenId: number | bigint): Promise<SporePumpTokenInfo | null> {
        const result = await this.readonly('get_token_info', fixedArgs(u64LE(tokenId, 'tokenId')));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const bytes = ensureCodeSuccess(result, 'get_token_info');
        if (bytes.length !== 33 || bytes[32] > 1) {
            throw new Error('SporePump get_token_info returned a malformed 33-byte payload');
        }
        return {
            supplySold: readU64(bytes, 0),
            licnRaised: readU64(bytes, 8),
            currentPrice: readU64(bytes, 16),
            marketCap: readU64(bytes, 24),
            graduated: bytes[32] === 1,
        };
    }

    async getTokenMetadata(tokenId: number | bigint): Promise<SporePumpTokenMetadata | null> {
        const result = await this.readonly('get_token_metadata', fixedArgs(u64LE(tokenId, 'tokenId')));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const bytes = ensureCodeSuccess(result, 'get_token_metadata');
        if (bytes.length < 4) throw new Error('SporePump metadata payload was malformed');
        const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        const nameLength = view.getUint16(0, true);
        const symbolOffset = 2 + nameLength;
        if (bytes.length < symbolOffset + 2) throw new Error('SporePump metadata name length was invalid');
        const symbolLength = view.getUint16(symbolOffset, true);
        if (bytes.length !== symbolOffset + 2 + symbolLength) {
            throw new Error('SporePump metadata symbol length was invalid');
        }
        const decoder = new TextDecoder('utf-8', { fatal: true });
        return {
            name: decoder.decode(bytes.slice(2, symbolOffset)),
            symbol: decoder.decode(bytes.slice(symbolOffset + 2)),
        };
    }

    async getBuyQuote(tokenId: number | bigint, licnAmount: number | bigint): Promise<bigint> {
        return decodeU64Value(
            await this.readonly('get_buy_quote', fixedArgs(
                u64LE(tokenId, 'tokenId'),
                u64LE(licnAmount, 'licnAmount'),
            )),
            'get_buy_quote',
        );
    }

    async getSellQuote(tokenId: number | bigint, tokenAmount: number | bigint): Promise<bigint> {
        return decodeU64Value(
            await this.readonly('get_sell_quote', fixedArgs(
                u64LE(tokenId, 'tokenId'),
                u64LE(tokenAmount, 'tokenAmount'),
            )),
            'get_sell_quote',
        );
    }

    async getTokenCount(): Promise<bigint> {
        return decodeU64Value(await this.readonly('get_token_count'), 'get_token_count');
    }

    async getCreatorRoyaltyBalance(tokenId: number | bigint, creator: PublicKey | string): Promise<bigint> {
        return decodeU64Value(
            await this.readonly('get_creator_royalty_balance', fixedArgs(
                u64LE(tokenId, 'tokenId'),
                normalizeAddress(creator).toBytes(),
            )),
            'get_creator_royalty_balance',
        );
    }

    async getPlatformStats(): Promise<SporePumpPlatformStats> {
        const bytes = ensureCodeSuccess(await this.readonly('get_platform_stats'), 'get_platform_stats');
        if (bytes.length !== PLATFORM_STATS_SIZE) throw new Error('SporePump platform stats payload was malformed');
        const migrationLocked = readU64(bytes, 72);
        const creatorRoyaltyBps = readU64(bytes, 80);
        if (migrationLocked > 1n || creatorRoyaltyBps > 1_000n) {
            throw new Error('SporePump platform stats contains invalid control values');
        }
        return {
            tokenCount: readU64(bytes, 0),
            platformFees: readU64(bytes, 8),
            curveReserve: readU64(bytes, 16),
            creatorLiability: readU64(bytes, 24),
            cumulativeGraduationRevenue: readU64(bytes, 32),
            graduatedCount: readU64(bytes, 40),
            accountingVersion: readU64(bytes, 48),
            migrationExpected: readU64(bytes, 56),
            migrationCursor: readU64(bytes, 64),
            migrationLocked: migrationLocked === 1n,
            creatorRoyaltyBps,
        };
    }

    async getCustodyStatus(): Promise<SporePumpCustodyStatus> {
        const bytes = ensureCodeSuccess(await this.readonly('get_custody_status'), 'get_custody_status');
        if (bytes.length !== 24) throw new Error('SporePump custody payload was malformed');
        return { balance: readU64(bytes), obligations: readU64(bytes, 8), recoverableSurplus: readU64(bytes, 16) };
    }

    async getAccountingMigrationToken(
        tokenId: number | bigint,
    ): Promise<SporePumpAccountingMigrationToken | null> {
        const result = await this.readonly(
            'get_accounting_migration_token',
            fixedArgs(u64LE(tokenId, 'tokenId')),
        );
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const bytes = ensureCodeSuccess(result, 'get_accounting_migration_token');
        if (bytes.length !== ACCOUNTING_MIGRATION_TOKEN_SIZE
            || ![0, 1, 3].includes(bytes[64])
            || bytes.slice(0, 32).every((byte) => byte === 0)) {
            throw new Error('SporePump accounting-migration token payload was malformed');
        }
        const supplySold = readU64(bytes, 32);
        const maxSupply = readU64(bytes, 48);
        if (supplySold > maxSupply) {
            throw new Error('SporePump accounting-migration token supply exceeds its cap');
        }
        return {
            creator: PublicKey.fromBytes(bytes.slice(0, 32)).toBase58(),
            supplySold,
            licnRaised: readU64(bytes, 40),
            maxSupply,
            createdSlot: readU64(bytes, 56),
            lifecycleState: bytes[64],
            creatorRoyalty: readU64(bytes, 65),
        };
    }

    async getGraduationStatus(tokenId: number | bigint): Promise<SporePumpGraduationStatus | null> {
        const result = await this.readonly('get_graduation_status', fixedArgs(u64LE(tokenId, 'tokenId')));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const bytes = ensureCodeSuccess(result, 'get_graduation_status');
        if (bytes.length !== GRADUATION_STATUS_SIZE || bytes[0] > 3) {
            throw new Error('SporePump graduation status payload was malformed');
        }
        const candidateBytes = bytes.slice(17, 49);
        return {
            state: bytes[0],
            eligibilitySlot: readU64(bytes, 1),
            migrationBoundarySlot: readU64(bytes, 9),
            candidate: candidateBytes.some((byte) => byte !== 0)
                ? PublicKey.fromBytes(candidateBytes).toBase58()
                : null,
            pairId: readU64(bytes, 49),
            poolId: readU64(bytes, 57),
            forwardRouteId: readU64(bytes, 65),
            reverseRouteId: readU64(bytes, 73),
            positionId: readU64(bytes, 81),
            licnLiquidity: readU64(bytes, 89),
            tokenLiquidity: readU64(bytes, 97),
            protocolTokenInventory: readU64(bytes, 105),
        };
    }

    async getGraduationInfo(): Promise<SporePumpGraduationInfo> {
        const bytes = ensureCodeSuccess(await this.readonly('get_graduation_info'), 'get_graduation_info');
        if (bytes.length !== 46 || bytes.slice(8, 14).some((flag) => flag > 1)) {
            throw new Error('SporePump graduation info payload was malformed');
        }
        return {
            cumulativeRevenue: readU64(bytes, 0),
            dexCoreConfigured: bytes[8] === 1,
            dexAmmConfigured: bytes[9] === 1,
            dexRouterConfigured: bytes[10] === 1,
            tokenTemplateConfigured: bytes[11] === 1,
            governanceConfigured: bytes[12] === 1,
            accountingReady: bytes[13] === 1,
            tickSize: readU64(bytes, 14),
            lotSize: readU64(bytes, 22),
            minimumOrder: readU64(bytes, 30),
            ammFeeTier: readU64(bytes, 38),
        };
    }

    async createToken(creator: Keypair, metadata?: CreateSporePumpTokenParams): Promise<string> {
        const creatorKey = creator.pubkey();
        const args = metadata
            ? metadataArgs(creatorKey, metadata)
            : fixedArgs(creatorKey.toBytes(), u64LE(SPOREPUMP_CREATION_FEE, 'creation fee'));
        return this.write(
            creator,
            metadata ? 'create_token_with_metadata' : 'create_token',
            args,
            SPOREPUMP_CREATION_FEE,
        );
    }

    async buy(
        buyer: Keypair,
        tokenId: number | bigint,
        licnAmount: number | bigint,
        minimumTokensOut: number | bigint,
    ): Promise<string> {
        const amount = normalizeU64(licnAmount, 'licnAmount');
        return this.write(buyer, 'buy_with_min_output', fixedArgs(
            buyer.pubkey().toBytes(),
            u64LE(tokenId, 'tokenId'),
            u64LE(amount, 'licnAmount'),
            u64LE(minimumTokensOut, 'minimumTokensOut'),
        ), amount);
    }

    async sell(
        seller: Keypair,
        tokenId: number | bigint,
        tokenAmount: number | bigint,
        minimumLicnOut: number | bigint,
    ): Promise<string> {
        return this.write(seller, 'sell_with_min_output', fixedArgs(
            seller.pubkey().toBytes(),
            u64LE(tokenId, 'tokenId'),
            u64LE(tokenAmount, 'tokenAmount'),
            u64LE(minimumLicnOut, 'minimumLicnOut'),
        ));
    }

    async claimCreatorRoyalty(creator: Keypair, tokenId: number | bigint, amount: number | bigint): Promise<string> {
        return this.write(creator, 'claim_creator_royalty', fixedArgs(
            creator.pubkey().toBytes(),
            u64LE(tokenId, 'tokenId'),
            u64LE(amount, 'amount'),
        ));
    }

    private adminU64(admin: Keypair, functionName: string, value: number | bigint): Promise<string> {
        return this.write(admin, functionName, fixedArgs(admin.pubkey().toBytes(), u64LE(value, 'value')));
    }

    pause(admin: Keypair): Promise<string> { return this.write(admin, 'pause', fixedArgs(admin.pubkey().toBytes())); }
    unpause(admin: Keypair): Promise<string> { return this.write(admin, 'unpause', fixedArgs(admin.pubkey().toBytes())); }
    freezeToken(admin: Keypair, tokenId: number | bigint): Promise<string> { return this.adminU64(admin, 'freeze_token', tokenId); }
    unfreezeToken(admin: Keypair, tokenId: number | bigint): Promise<string> { return this.adminU64(admin, 'unfreeze_token', tokenId); }
    setBuyCooldown(admin: Keypair, slots: number | bigint): Promise<string> { return this.adminU64(admin, 'set_buy_cooldown', slots); }
    setSellCooldown(admin: Keypair, slots: number | bigint): Promise<string> { return this.adminU64(admin, 'set_sell_cooldown', slots); }
    setMaxBuy(admin: Keypair, amount: number | bigint): Promise<string> { return this.adminU64(admin, 'set_max_buy', amount); }
    setCreatorRoyalty(admin: Keypair, basisPoints: number | bigint): Promise<string> { return this.adminU64(admin, 'set_creator_royalty', basisPoints); }
    withdrawFees(admin: Keypair, amount: number | bigint): Promise<string> { return this.adminU64(admin, 'withdraw_fees', amount); }
    recoverCustodySurplus(admin: Keypair, amount: number | bigint): Promise<string> { return this.adminU64(admin, 'recover_custody_surplus', amount); }
    beginAccountingV3Migration(admin: Keypair, expectedTokens: number | bigint): Promise<string> { return this.adminU64(admin, 'begin_accounting_v3_migration', expectedTokens); }
    completeAccountingV3Migration(admin: Keypair): Promise<string> { return this.write(admin, 'complete_accounting_v3_migration', fixedArgs(admin.pubkey().toBytes())); }

    migrateAccountingV3Token(keeper: Keypair, tokenId: number | bigint): Promise<string> {
        return this.write(keeper, 'migrate_accounting_v3_token', fixedArgs(u64LE(tokenId, 'tokenId')));
    }

    proposeAdmin(admin: Keypair, nextAdmin: PublicKey | string): Promise<string> {
        return this.write(admin, 'propose_admin', fixedArgs(admin.pubkey().toBytes(), normalizeAddress(nextAdmin).toBytes()));
    }

    acceptAdmin(nextAdmin: Keypair): Promise<string> {
        return this.write(nextAdmin, 'accept_admin', fixedArgs(nextAdmin.pubkey().toBytes()));
    }

    setDexAddresses(admin: Keypair, core: PublicKey | string, amm: PublicKey | string): Promise<string> {
        return this.write(admin, 'set_dex_addresses', fixedArgs(
            admin.pubkey().toBytes(), normalizeAddress(core).toBytes(), normalizeAddress(amm).toBytes(),
        ));
    }

    setGraduationGovernance(admin: Keypair, governance: PublicKey | string): Promise<string> {
        return this.write(admin, 'set_graduation_governance', fixedArgs(
            admin.pubkey().toBytes(), normalizeAddress(governance).toBytes(),
        ));
    }

    setGraduationConfig(governance: Keypair, config: SporePumpGraduationConfig): Promise<string> {
        return this.write(governance, 'set_graduation_config', fixedArgs(
            governance.pubkey().toBytes(),
            normalizeAddress(config.router).toBytes(),
            normalizeAddress(config.tokenTemplateHash).toBytes(),
            u64LE(config.tickSize, 'tickSize'),
            u64LE(config.lotSize, 'lotSize'),
            u64LE(config.minimumOrder, 'minimumOrder'),
            u32LE(config.ammFeeTier, 'ammFeeTier'),
        ));
    }

    beginGraduation(keeper: Keypair, tokenId: number | bigint, candidate: PublicKey | string): Promise<string> {
        return this.write(keeper, 'begin_migration', fixedArgs(
            keeper.pubkey().toBytes(), u64LE(tokenId, 'tokenId'), normalizeAddress(candidate).toBytes(),
        ));
    }

    abortGraduation(keeper: Keypair, tokenId: number | bigint): Promise<string> {
        return this.adminU64(keeper, 'abort_migration', tokenId);
    }

    finalizeGraduation(keeper: Keypair, tokenId: number | bigint): Promise<string> {
        return this.adminU64(keeper, 'finalize_migration', tokenId);
    }
}
