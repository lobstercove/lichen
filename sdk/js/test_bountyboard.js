import assert from 'node:assert/strict';

import { BountyBoardClient, Keypair, PublicKey } from './dist/index.js';

function encoded(values, trailing = false) {
    const bytes = new Uint8Array(values.length * 8 + (trailing ? 1 : 0));
    const view = new DataView(bytes.buffer);
    values.forEach((value, index) => view.setBigUint64(index * 8, BigInt(value), true));
    return { success: true, returnCode: 0, returnData: Buffer.from(bytes).toString('base64') };
}

const calls = [];
const fakeConnection = {
    async getSymbolRegistry(symbol) {
        if (symbol === 'BOUNTY') return { program: '11111111111111111111111111111112' };
        throw new Error('missing symbol');
    },
    async callContract(_signer, _program, functionName, args, value) {
        calls.push({ functionName, args, value });
        return 'test-signature';
    },
    async callReadonlyContract(_program, functionName) {
        if (functionName === 'get_bounty_count_exact') return encoded([7]);
        if (functionName === 'get_accounting_migration_status') return encoded([2, 1, 100, 0, 1]);
        if (functionName === 'get_accounting_health') return encoded([2, 0, 100, 10, 110, 110, 1]);
        if (functionName === 'get_admin_transition') {
            const bytes = new Uint8Array(64);
            bytes.fill(3, 0, 32);
            bytes.fill(4, 32, 64);
            return { success: true, returnCode: 0, returnData: Buffer.from(bytes).toString('base64') };
        }
        throw new Error(`unexpected ${functionName}`);
    },
    async getBountyBoardStats() {
        return {
            bounty_count: 2,
            bounty_count_exact: '2',
            completed_count: 1,
            completed_count_exact: '1',
            reward_volume: 90,
            reward_volume_raw_exact: '90',
            cancel_count: 0,
            cancel_count_exact: '0',
            paused: false,
        };
    },
};

const client = new BountyBoardClient(fakeConnection);
const signer = Keypair.fromSeed(Uint8Array.from({ length: 32 }, (_, index) => index));
const bountyCount = await client.getBountyCount();
const migration = await client.getAccountingMigrationStatus();
const health = await client.getAccountingHealth();
const adminTransition = await client.getAdminTransition();
const stats = await client.getStats();
assert.equal(bountyCount, 7n);
assert.equal(migration.cursor, 1n);
assert.equal(migration.locked, true);
assert.equal(health.totalLiability, 110n);
assert.equal(health.solvent, true);
assert.deepEqual(adminTransition.currentAdmin.toBytes(), new Uint8Array(32).fill(3));
assert.deepEqual(adminTransition.pendingAdmin?.toBytes(), new Uint8Array(32).fill(4));
assert.equal(stats.totalRewardVolume, 90n);

await client.createBounty(signer, {
    titleHash: new Uint8Array(32).fill(1),
    rewardAmount: 100n,
    deadlineSlot: 200n,
    paymentValue: 0n,
});
await client.beginAccountingV2Migration(signer, 2n);
await client.migrateAccountingV2Bounty(signer, 1n);
await client.completeAccountingV2Migration(signer, 100n, 10n, 110n);
await client.proposeAdmin(signer, new PublicKey(new Uint8Array(32).fill(4)));
await client.acceptAdmin(signer);
await client.cancelAdminProposal(signer);
assert.deepEqual(calls.map(call => call.functionName), [
    'create_bounty',
    'begin_accounting_v2_migration',
    'migrate_accounting_v2_bounty',
    'complete_accounting_v2_migration',
    'propose_admin',
    'accept_admin',
    'cancel_admin_proposal',
]);
assert.equal(calls[0].value, 0n);
assert.deepEqual([...calls[1].args.slice(0, 3)], [0xAB, 0x20, 0x08]);
assert.deepEqual([...calls[2].args.slice(0, 2)], [0xAB, 0x08]);
assert.deepEqual([...calls[3].args.slice(0, 5)], [0xAB, 0x20, 0x08, 0x08, 0x08]);

await assert.rejects(client.createBounty(signer, {
    titleHash: new Uint8Array(32),
    rewardAmount: 1n,
    deadlineSlot: 2n,
}), /zero hash/);
await assert.rejects(client.submitWork(signer, {
    bountyId: 0n,
    proofHash: new Uint8Array(32),
}), /zero hash/);

const malformedClient = new BountyBoardClient({
    ...fakeConnection,
    async callReadonlyContract(_program, functionName) {
        if (functionName === 'get_accounting_migration_status') return encoded([2, 1, 100, 0, 2]);
        if (functionName === 'get_accounting_health') return encoded([2, 0, 100, 10, 110, 110, 1], true);
        throw new Error(`unexpected ${functionName}`);
    },
});
await assert.rejects(malformedClient.getAccountingMigrationStatus(), /0 or 1/);
await assert.rejects(malformedClient.getAccountingHealth(), /exactly 56 bytes/);

const malformedRowClient = new BountyBoardClient({
    ...fakeConnection,
    async callReadonlyContract() {
        return { success: false, returnCode: 2 };
    },
});
await assert.rejects(malformedRowClient.getBounty(1n), /returned code 2/);
await assert.rejects(malformedRowClient.getSubmission(1n, 0), /returned code 2/);
await assert.rejects(malformedRowClient.getBountyTerms(1n), /returned code 2/);

console.log('BountyBoard SDK exact-accounting tests passed');
