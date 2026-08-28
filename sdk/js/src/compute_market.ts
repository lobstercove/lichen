import { Connection, ReadonlyContractResult } from './connection.js';
import { Keypair } from './keypair.js';
import { PublicKey } from './publickey.js';

const PROGRAM_SYMBOL_CANDIDATES = ['COMPUTE', 'compute', 'ComputeMarket', 'COMPUTEMARKET', 'compute_market'];
const MAX_U64 = (1n << 64n) - 1n;

export const COMPUTE_JOB_PENDING = 0;
export const COMPUTE_JOB_CLAIMED = 1;
export const COMPUTE_JOB_COMPLETED = 2;
export const COMPUTE_JOB_DISPUTED = 3;
export const COMPUTE_JOB_CANCELLED = 4;
export const COMPUTE_JOB_RESOLVED = 5;
export const COMPUTE_JOB_RELEASED = 6;

export interface ComputeMarketProviderInfo {
    address: PublicKey;
    totalCapacity: bigint;
    pricePerUnit: bigint;
    jobsCompleted: bigint;
    active: boolean;
    registeredSlot: bigint;
}

export interface ComputeMarketProviderCapacity {
    total: bigint;
    reserved: bigint;
    available: bigint;
}

export interface ComputeMarketJobInfo {
    requester: PublicKey;
    computeUnits: bigint;
    maxPrice: bigint;
    codeHash: Uint8Array;
    status: number;
    provider: PublicKey;
    resultHash: Uint8Array;
    createdSlot: bigint;
    completedSlot: bigint;
}

export interface ComputeMarketJobTiming {
    createdSlot: bigint;
    claimDeadline: bigint;
    claimedSlot: bigint;
    completionDeadline: bigint;
    completedSlot: bigint;
    challengeDeadline: bigint;
}

export interface ComputeMarketPlatformStats {
    jobCount: bigint;
    completedCount: bigint;
    paymentVolume: bigint;
    disputeCount: bigint;
}

export interface ComputeMarketAgentControls {
    enabled: boolean;
    routePaused: boolean;
    maxDailyCap: bigint;
    maxPerTaskCap: bigint;
    policyCount: bigint;
    paymentCount: bigint;
    paymentVolume: bigint;
    blockedPaymentCount: bigint;
    blockedPaymentCountSupported: false;
}

export interface ComputeMarketAccountingMigrationStatus {
    expectedJobCount: bigint;
    cursor: bigint;
    reconstructedEscrow: bigint;
    reconstructedUnpaid: bigint;
    accountingVersion: bigint;
    locked: boolean;
}

export interface ComputeMarketAccountingHealth {
    accountingVersion: bigint;
    migrationLocked: boolean;
    escrowLiability: bigint;
    unpaidLiability: bigint;
    platformFees: bigint;
    totalLiability: bigint;
    custodyBalance: bigint;
    solvent: boolean;
}

export interface ComputeMarketAgentPolicy {
    policyVersion: bigint;
    dailyCap: bigint;
    perTaskCap: bigint;
    policyHash: Uint8Array;
    createdSlot: bigint;
    updatedSlot: bigint;
    active: boolean;
}

export interface SubmitComputeJobParams {
    computeUnits: number | bigint;
    maxPrice: number | bigint;
    codeHash: Uint8Array;
    /** Native LICN attached to escrow. Use 0 for an allowance-based token market. */
    paymentValue?: number | bigint;
}

export interface SubmitAgentComputeJobParams extends SubmitComputeJobParams {
    actionHash: Uint8Array;
}

function normalizeAddress(value: PublicKey | string): PublicKey {
    return value instanceof PublicKey ? value : new PublicKey(value);
}

function normalizeU64(value: number | bigint, fieldName: string): bigint {
    const result = typeof value === 'bigint'
        ? value
        : Number.isSafeInteger(value) && value >= 0 ? BigInt(value) : null;
    if (result === null || result < 0n || result > MAX_U64) {
        throw new Error(`${fieldName} must be a u64-safe integer value`);
    }
    return result;
}

function u64LE(value: number | bigint, fieldName: string): Uint8Array {
    const bytes = new Uint8Array(8);
    new DataView(bytes.buffer).setBigUint64(0, normalizeU64(value, fieldName), true);
    return bytes;
}

function hash32(value: Uint8Array, fieldName: string): Uint8Array {
    if (value.length !== 32) {
        throw new Error(`${fieldName} must be exactly 32 bytes`);
    }
    if (value.every(byte => byte === 0)) {
        throw new Error(`${fieldName} must not be the zero hash`);
    }
    return value;
}

function requireLength(data: Uint8Array, expected: number, functionName: string): Uint8Array {
    if (data.length !== expected) {
        throw new Error(`Compute Market ${functionName} payload must be exactly ${expected} bytes`);
    }
    return data;
}

function layout(types: number[], chunks: Uint8Array[]): Uint8Array {
    const header = Uint8Array.from([0xAB, ...types]);
    const output = new Uint8Array(header.length + chunks.reduce((total, chunk) => total + chunk.length, 0));
    output.set(header);
    let offset = header.length;
    for (const chunk of chunks) {
        output.set(chunk, offset);
        offset += chunk.length;
    }
    return output;
}

function addressArgs(...addresses: Array<PublicKey | string>): Uint8Array {
    return layout(addresses.map(() => 0x20), addresses.map(address => normalizeAddress(address).toBytes()));
}

function idArgs(id: number | bigint, fieldName = 'jobId'): Uint8Array {
    return layout([0x08], [u64LE(id, fieldName)]);
}

function bytes(result: ReadonlyContractResult, functionName: string): Uint8Array {
    const code = result.returnCode ?? 0;
    if (code !== 0 || result.success === false || !result.returnData) {
        throw new Error(result.error ?? `Compute Market ${functionName} returned code ${code}`);
    }
    return Uint8Array.from(Buffer.from(result.returnData, 'base64'));
}

function readU64(data: Uint8Array, offset: number): bigint {
    if (data.length < offset + 8) {
        throw new Error('Compute Market return payload was shorter than expected');
    }
    return new DataView(data.buffer, data.byteOffset, data.byteLength).getBigUint64(offset, true);
}

function decodeProvider(data: Uint8Array): ComputeMarketProviderInfo {
    requireLength(data, 65, 'get_provider_info');
    return {
        address: new PublicKey(data.slice(0, 32)),
        totalCapacity: readU64(data, 32),
        pricePerUnit: readU64(data, 40),
        jobsCompleted: readU64(data, 48),
        active: data[56] === 1,
        registeredSlot: readU64(data, 57),
    };
}

function decodeJob(data: Uint8Array): ComputeMarketJobInfo {
    requireLength(data, 161, 'get_job');
    return {
        requester: new PublicKey(data.slice(0, 32)),
        computeUnits: readU64(data, 32),
        maxPrice: readU64(data, 40),
        codeHash: data.slice(48, 80),
        status: data[80],
        provider: new PublicKey(data.slice(81, 113)),
        resultHash: data.slice(113, 145),
        createdSlot: readU64(data, 145),
        completedSlot: readU64(data, 153),
    };
}

export class ComputeMarketClient {
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
            } catch { /* try next alias */ }
        }
        throw new Error('Unable to resolve the Compute Market program via getSymbolRegistry("COMPUTE")');
    }

    private async readonly(functionName: string, args: Uint8Array = new Uint8Array()): Promise<ReadonlyContractResult> {
        return this.connection.callReadonlyContract(await this.getProgramId(), functionName, args);
    }

    private async write(caller: Keypair, functionName: string, args: Uint8Array, value: number | bigint = 0): Promise<string> {
        return this.connection.callContract(caller, await this.getProgramId(), functionName, args, value);
    }

    async getJob(jobId: number | bigint): Promise<ComputeMarketJobInfo | null> {
        const result = await this.readonly('get_job', idArgs(jobId));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        return decodeJob(bytes(result, 'get_job'));
    }

    async getJobCount(): Promise<bigint> {
        const data = requireLength(bytes(await this.readonly('get_job_count'), 'get_job_count'), 8, 'get_job_count');
        return readU64(data, 0);
    }

    async getProvider(provider: PublicKey | string): Promise<ComputeMarketProviderInfo | null> {
        const result = await this.readonly('get_provider_info', addressArgs(provider));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        return decodeProvider(bytes(result, 'get_provider_info'));
    }

    async getProviderCapacity(provider: PublicKey | string): Promise<ComputeMarketProviderCapacity | null> {
        const result = await this.readonly('get_provider_capacity', addressArgs(provider));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const data = requireLength(bytes(result, 'get_provider_capacity'), 24, 'get_provider_capacity');
        return { total: readU64(data, 0), reserved: readU64(data, 8), available: readU64(data, 16) };
    }

    async getJobTiming(jobId: number | bigint): Promise<ComputeMarketJobTiming | null> {
        const result = await this.readonly('get_job_timing', idArgs(jobId));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const data = requireLength(bytes(result, 'get_job_timing'), 48, 'get_job_timing');
        return {
            createdSlot: readU64(data, 0), claimDeadline: readU64(data, 8), claimedSlot: readU64(data, 16),
            completionDeadline: readU64(data, 24), completedSlot: readU64(data, 32), challengeDeadline: readU64(data, 40),
        };
    }

    async getPlatformStats(): Promise<ComputeMarketPlatformStats> {
        const data = requireLength(bytes(await this.readonly('get_platform_stats'), 'get_platform_stats'), 32, 'get_platform_stats');
        return { jobCount: readU64(data, 0), completedCount: readU64(data, 8), paymentVolume: readU64(data, 16), disputeCount: readU64(data, 24) };
    }

    private async getAmount(functionName: string, args: Uint8Array): Promise<bigint> {
        return readU64(requireLength(bytes(await this.readonly(functionName, args), functionName), 8, functionName), 0);
    }

    async getEscrow(jobId: number | bigint): Promise<bigint> { return this.getAmount('get_escrow', idArgs(jobId)); }
    async getPlatformFees(token: PublicKey | string): Promise<bigint> { return this.getAmount('get_platform_fees', addressArgs(token)); }
    async getUnpaidPayout(token: PublicKey | string, recipient: PublicKey | string): Promise<bigint> {
        return this.getAmount('get_unpaid_payout', addressArgs(token, recipient));
    }
    async getAgentSpendWindow(agent: PublicKey | string, window: number | bigint): Promise<bigint> {
        return this.getAmount('get_agent_spend_window', layout([0x20, 0x08], [normalizeAddress(agent).toBytes(), u64LE(window, 'window')]));
    }

    async getAgentJobAction(jobId: number | bigint): Promise<Uint8Array | null> {
        const result = await this.readonly('get_agent_job_action', idArgs(jobId));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const data = bytes(result, 'get_agent_job_action');
        if (data.length !== 32) throw new Error('Compute Market action hash must be 32 bytes');
        return data;
    }

    async getAgentControls(): Promise<ComputeMarketAgentControls> {
        const data = requireLength(bytes(await this.readonly('get_agent_compute_controls'), 'get_agent_compute_controls'), 50, 'get_agent_compute_controls');
        return {
            enabled: data[0] === 1, routePaused: data[1] === 1, maxDailyCap: readU64(data, 2),
            maxPerTaskCap: readU64(data, 10), policyCount: readU64(data, 18), paymentCount: readU64(data, 26),
            paymentVolume: readU64(data, 34), blockedPaymentCount: readU64(data, 42),
            blockedPaymentCountSupported: false,
        };
    }

    async getAgentPolicy(agent: PublicKey | string): Promise<ComputeMarketAgentPolicy | null> {
        const result = await this.readonly('get_agent_spending_policy', addressArgs(agent));
        if ((result.returnCode ?? 0) === 1 || !result.returnData) return null;
        const data = requireLength(bytes(result, 'get_agent_spending_policy'), 73, 'get_agent_spending_policy');
        return {
            policyVersion: readU64(data, 0), dailyCap: readU64(data, 8), perTaskCap: readU64(data, 16),
            policyHash: data.slice(24, 56), createdSlot: readU64(data, 56), updatedSlot: readU64(data, 64), active: data[72] === 1,
        };
    }

    async getAccountingMigrationStatus(): Promise<ComputeMarketAccountingMigrationStatus> {
        const data = requireLength(bytes(await this.readonly('get_accounting_migration_status'), 'get_accounting_migration_status'), 48, 'get_accounting_migration_status');
        return {
            expectedJobCount: readU64(data, 0), cursor: readU64(data, 8),
            reconstructedEscrow: readU64(data, 16), reconstructedUnpaid: readU64(data, 24),
            accountingVersion: readU64(data, 32), locked: readU64(data, 40) === 1n,
        };
    }

    async getAccountingHealth(): Promise<ComputeMarketAccountingHealth> {
        const data = requireLength(bytes(await this.readonly('get_accounting_health'), 'get_accounting_health'), 64, 'get_accounting_health');
        return {
            accountingVersion: readU64(data, 0), migrationLocked: readU64(data, 8) === 1n,
            escrowLiability: readU64(data, 16), unpaidLiability: readU64(data, 24),
            platformFees: readU64(data, 32), totalLiability: readU64(data, 40),
            custodyBalance: readU64(data, 48), solvent: readU64(data, 56) === 1n,
        };
    }

    async registerProvider(provider: Keypair, capacity: number | bigint, pricePerUnit: number | bigint): Promise<string> {
        return this.write(provider, 'register_provider', layout([0x20, 0x08, 0x08], [provider.pubkey().toBytes(), u64LE(capacity, 'capacity'), u64LE(pricePerUnit, 'pricePerUnit')]));
    }
    async updateProvider(provider: Keypair, capacity: number | bigint, pricePerUnit: number | bigint): Promise<string> {
        return this.write(provider, 'update_provider', layout([0x20, 0x08, 0x08], [provider.pubkey().toBytes(), u64LE(capacity, 'capacity'), u64LE(pricePerUnit, 'pricePerUnit')]));
    }
    async deactivateProvider(provider: Keypair): Promise<string> { return this.write(provider, 'deactivate_provider', addressArgs(provider.pubkey())); }
    async reactivateProvider(provider: Keypair): Promise<string> { return this.write(provider, 'reactivate_provider', addressArgs(provider.pubkey())); }

    async submitJob(requester: Keypair, params: SubmitComputeJobParams): Promise<string> {
        const maxPrice = normalizeU64(params.maxPrice, 'maxPrice');
        const args = layout([0x20, 0x08, 0x08, 0x20], [requester.pubkey().toBytes(), u64LE(params.computeUnits, 'computeUnits'), u64LE(maxPrice, 'maxPrice'), hash32(params.codeHash, 'codeHash')]);
        return this.write(requester, 'submit_job', args, params.paymentValue ?? maxPrice);
    }
    async createJob(requester: Keypair, params: SubmitComputeJobParams): Promise<string> { return this.submitJobNamed(requester, params, 'create_job'); }
    private async submitJobNamed(requester: Keypair, params: SubmitComputeJobParams, name: string): Promise<string> {
        const maxPrice = normalizeU64(params.maxPrice, 'maxPrice');
        return this.write(requester, name, layout([0x20, 0x08, 0x08, 0x20], [requester.pubkey().toBytes(), u64LE(params.computeUnits, 'computeUnits'), u64LE(maxPrice, 'maxPrice'), hash32(params.codeHash, 'codeHash')]), params.paymentValue ?? maxPrice);
    }
    async claimJob(provider: Keypair, jobId: number | bigint): Promise<string> { return this.write(provider, 'claim_job', layout([0x20, 0x08], [provider.pubkey().toBytes(), u64LE(jobId, 'jobId')])); }
    async acceptJob(provider: Keypair, jobId: number | bigint): Promise<string> { return this.write(provider, 'accept_job', layout([0x20, 0x08], [provider.pubkey().toBytes(), u64LE(jobId, 'jobId')])); }
    async completeJob(provider: Keypair, jobId: number | bigint, resultHash: Uint8Array): Promise<string> { return this.completeNamed(provider, jobId, resultHash, 'complete_job'); }
    async submitResult(provider: Keypair, jobId: number | bigint, resultHash: Uint8Array): Promise<string> { return this.completeNamed(provider, jobId, resultHash, 'submit_result'); }
    private async completeNamed(provider: Keypair, jobId: number | bigint, resultHash: Uint8Array, name: string): Promise<string> {
        return this.write(provider, name, layout([0x20, 0x08, 0x20], [provider.pubkey().toBytes(), u64LE(jobId, 'jobId'), hash32(resultHash, 'resultHash')]));
    }
    async disputeJob(requester: Keypair, jobId: number | bigint): Promise<string> { return this.write(requester, 'dispute_job', layout([0x20, 0x08], [requester.pubkey().toBytes(), u64LE(jobId, 'jobId')])); }
    async cancelJob(requester: Keypair, jobId: number | bigint): Promise<string> { return this.write(requester, 'cancel_job', layout([0x20, 0x08], [requester.pubkey().toBytes(), u64LE(jobId, 'jobId')])); }
    async releasePayment(caller: Keypair, jobId: number | bigint): Promise<string> { return this.write(caller, 'release_payment', idArgs(jobId)); }
    async confirmResult(caller: Keypair, jobId: number | bigint): Promise<string> { return this.write(caller, 'confirm_result', idArgs(jobId)); }
    async resolveDispute(arbitrator: Keypair, jobId: number | bigint, providerShareBps: number | bigint): Promise<string> {
        return this.write(arbitrator, 'resolve_dispute', layout([0x20, 0x08, 0x08], [arbitrator.pubkey().toBytes(), u64LE(jobId, 'jobId'), u64LE(providerShareBps, 'providerShareBps')]));
    }
    async claimUnpaidPayout(recipient: Keypair, token: PublicKey | string): Promise<string> { return this.write(recipient, 'claim_unpaid_payout', addressArgs(recipient.pubkey(), token)); }

    async setAgentPolicy(agent: Keypair, dailyCap: number | bigint, perTaskCap: number | bigint, policyHash: Uint8Array, policyVersion: number | bigint): Promise<string> {
        return this.write(agent, 'set_agent_spending_policy', layout([0x20, 0x08, 0x08, 0x20, 0x08], [agent.pubkey().toBytes(), u64LE(dailyCap, 'dailyCap'), u64LE(perTaskCap, 'perTaskCap'), hash32(policyHash, 'policyHash'), u64LE(policyVersion, 'policyVersion')]));
    }
    async disableAgentPolicy(agent: Keypair): Promise<string> { return this.write(agent, 'disable_agent_spending_policy', addressArgs(agent.pubkey())); }
    async submitAgentJob(agent: Keypair, params: SubmitAgentComputeJobParams): Promise<string> {
        const maxPrice = normalizeU64(params.maxPrice, 'maxPrice');
        return this.write(agent, 'submit_agent_job', layout([0x20, 0x08, 0x08, 0x20, 0x20], [agent.pubkey().toBytes(), u64LE(params.computeUnits, 'computeUnits'), u64LE(maxPrice, 'maxPrice'), hash32(params.codeHash, 'codeHash'), hash32(params.actionHash, 'actionHash')]), params.paymentValue ?? maxPrice);
    }

    async initialize(admin: Keypair): Promise<string> { return this.write(admin, 'initialize', addressArgs(admin.pubkey())); }
    async setClaimTimeout(admin: Keypair, slots: number | bigint): Promise<string> { return this.adminU64(admin, 'set_claim_timeout', slots); }
    async setCompleteTimeout(admin: Keypair, slots: number | bigint): Promise<string> { return this.adminU64(admin, 'set_complete_timeout', slots); }
    async setChallengePeriod(admin: Keypair, slots: number | bigint): Promise<string> { return this.adminU64(admin, 'set_challenge_period', slots); }
    async setPlatformFee(admin: Keypair, feeBps: number | bigint): Promise<string> { return this.adminU64(admin, 'set_platform_fee', feeBps); }
    private async adminU64(admin: Keypair, name: string, value: number | bigint): Promise<string> { return this.write(admin, name, layout([0x20, 0x08], [admin.pubkey().toBytes(), u64LE(value, 'value')])); }
    async addArbitrator(admin: Keypair, arbitrator: PublicKey | string): Promise<string> { return this.adminAddress(admin, 'add_arbitrator', arbitrator); }
    async removeArbitrator(admin: Keypair, arbitrator: PublicKey | string): Promise<string> { return this.adminAddress(admin, 'remove_arbitrator', arbitrator); }
    async setTokenAddress(admin: Keypair, token: PublicKey | string): Promise<string> { return this.adminAddress(admin, 'set_token_address', token); }
    async setFeeTreasury(admin: Keypair, treasury: PublicKey | string): Promise<string> { return this.adminAddress(admin, 'set_fee_treasury', treasury); }
    async setLichenIdAddress(admin: Keypair, contract: PublicKey | string): Promise<string> { return this.adminAddress(admin, 'set_lichenid_address', contract); }
    private async adminAddress(admin: Keypair, name: string, address: PublicKey | string): Promise<string> { return this.write(admin, name, addressArgs(admin.pubkey(), address)); }
    async setIdentityAdmin(admin: Keypair): Promise<string> { return this.write(admin, 'set_identity_admin', addressArgs(admin.pubkey())); }
    async setIdentityGate(admin: Keypair, minReputation: number | bigint): Promise<string> { return this.adminU64(admin, 'set_identity_gate', minReputation); }
    async setAgentControls(admin: Keypair, enabled: boolean, routePaused: boolean, maxDailyCap: number | bigint, maxPerTaskCap: number | bigint): Promise<string> {
        return this.write(admin, 'set_agent_compute_controls', layout([0x20, 0x08, 0x08, 0x08, 0x08], [admin.pubkey().toBytes(), u64LE(enabled ? 1 : 0, 'enabled'), u64LE(routePaused ? 1 : 0, 'routePaused'), u64LE(maxDailyCap, 'maxDailyCap'), u64LE(maxPerTaskCap, 'maxPerTaskCap')]));
    }
    async pause(admin: Keypair): Promise<string> { return this.write(admin, 'pause', addressArgs(admin.pubkey())); }
    async unpause(admin: Keypair): Promise<string> { return this.write(admin, 'unpause', addressArgs(admin.pubkey())); }
    async withdrawPlatformFees(admin: Keypair, token: PublicKey | string, amount: number | bigint): Promise<string> {
        return this.write(admin, 'withdraw_platform_fees', layout([0x20, 0x20, 0x08], [admin.pubkey().toBytes(), normalizeAddress(token).toBytes(), u64LE(amount, 'amount')]));
    }
    async beginAccountingV3Migration(admin: Keypair, expectedJobCount: number | bigint): Promise<string> {
        return this.write(admin, 'begin_accounting_v3_migration', layout([0x20, 0x08], [admin.pubkey().toBytes(), u64LE(expectedJobCount, 'expectedJobCount')]));
    }
    async migrateAccountingV3Job(caller: Keypair, jobId: number | bigint): Promise<string> {
        return this.write(caller, 'migrate_accounting_v3_job', idArgs(jobId));
    }
    async completeAccountingV3Migration(admin: Keypair, expectedEscrow: number | bigint, expectedUnpaid: number | bigint, expectedPlatformFees: number | bigint, expectedTotalLiability: number | bigint): Promise<string> {
        return this.write(admin, 'complete_accounting_v3_migration', layout(
            [0x20, 0x08, 0x08, 0x08, 0x08],
            [admin.pubkey().toBytes(), u64LE(expectedEscrow, 'expectedEscrow'), u64LE(expectedUnpaid, 'expectedUnpaid'), u64LE(expectedPlatformFees, 'expectedPlatformFees'), u64LE(expectedTotalLiability, 'expectedTotalLiability')],
        ));
    }
}
