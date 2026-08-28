import assert from 'node:assert/strict';

import { ComputeMarketClient, Keypair } from './dist/index.js';

function encoded(values, trailing = false) {
    const bytes = new Uint8Array(values.length * 8 + (trailing ? 1 : 0));
    const view = new DataView(bytes.buffer);
    values.forEach((value, index) => view.setBigUint64(index * 8, BigInt(value), true));
    return { success: true, returnCode: 0, returnData: Buffer.from(bytes).toString('base64') };
}

const calls = [];
const fakeConnection = {
    async getSymbolRegistry(symbol) {
        if (symbol === 'COMPUTE') return { program: '11111111111111111111111111111112' };
        throw new Error('missing symbol');
    },
    async callContract(_signer, _program, functionName, args, value) {
        calls.push({ functionName, args, value });
        return 'test-signature';
    },
    async callReadonlyContract(_program, functionName) {
        if (functionName === 'get_accounting_migration_status') return encoded([2, 1, 100, 20, 0, 1]);
        if (functionName === 'get_accounting_health') return encoded([3, 0, 100, 20, 10, 130, 150, 1]);
        throw new Error(`unexpected ${functionName}`);
    },
};

const client = new ComputeMarketClient(fakeConnection);
const signer = Keypair.fromSeed(Uint8Array.from({ length: 32 }, (_, index) => index));
const migration = await client.getAccountingMigrationStatus();
const health = await client.getAccountingHealth();
assert.equal(migration.cursor, 1n);
assert.equal(migration.locked, true);
assert.equal(health.totalLiability, 130n);
assert.equal(health.solvent, true);

await client.beginAccountingV3Migration(signer, 2n);
await client.migrateAccountingV3Job(signer, 1n);
await client.completeAccountingV3Migration(signer, 100n, 20n, 10n, 130n);
assert.deepEqual(calls.map(call => call.functionName), [
    'begin_accounting_v3_migration',
    'migrate_accounting_v3_job',
    'complete_accounting_v3_migration',
]);
assert.deepEqual([...calls[0].args.slice(0, 3)], [0xAB, 0x20, 0x08]);
assert.deepEqual([...calls[1].args.slice(0, 2)], [0xAB, 0x08]);
assert.deepEqual([...calls[2].args.slice(0, 6)], [0xAB, 0x20, 0x08, 0x08, 0x08, 0x08]);

await assert.rejects(client.submitJob(signer, {
    computeUnits: 10n,
    maxPrice: 100n,
    codeHash: new Uint8Array(32),
}), /zero hash/);

const trailingClient = new ComputeMarketClient({
    ...fakeConnection,
    async callReadonlyContract(_program, functionName) {
        if (functionName === 'get_accounting_migration_status') return encoded([2, 1, 100, 20, 0, 1], true);
        if (functionName === 'get_accounting_health') return encoded([3, 0, 100, 20, 10, 130, 150, 1], true);
        throw new Error(`unexpected ${functionName}`);
    },
});
await assert.rejects(trailingClient.getAccountingMigrationStatus(), /exactly 48 bytes/);
await assert.rejects(trailingClient.getAccountingHealth(), /exactly 64 bytes/);

console.log('Compute Market SDK exact-accounting tests passed');
