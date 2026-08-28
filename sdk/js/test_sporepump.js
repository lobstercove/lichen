import assert from 'node:assert/strict';

import {
    Keypair,
    SporePumpClient,
    SPOREPUMP_CREATION_FEE,
} from './dist/index.js';

function encoded(bytes, returnCode = 0) {
    return { success: true, returnCode, returnData: Buffer.from(bytes).toString('base64') };
}

const calls = [];
const fakeConnection = {
    async getSymbolRegistry(symbol) {
        if (symbol === 'SPOREPUMP') return { program: '11111111111111111111111111111112' };
        throw new Error('missing symbol');
    },
    async callContract(_signer, _program, functionName, args, value) {
        calls.push({ functionName, args, value });
        return 'test-signature';
    },
    async callReadonlyContract(_program, functionName) {
        if (functionName === 'get_token_info') {
            const bytes = new Uint8Array(33);
            const view = new DataView(bytes.buffer);
            [11n, 22n, 33n, 44n].forEach((value, index) => view.setBigUint64(index * 8, value, true));
            return encoded(bytes);
        }
        if (functionName === 'get_platform_stats') {
            const bytes = new Uint8Array(88);
            const view = new DataView(bytes.buffer);
            for (let index = 0; index < 11; index += 1) view.setBigUint64(index * 8, BigInt(index + 1), true);
            view.setBigUint64(72, 1n, true);
            return encoded(bytes);
        }
        if (functionName === 'get_accounting_migration_token') {
            const bytes = new Uint8Array(73);
            const view = new DataView(bytes.buffer);
            bytes.fill(7, 0, 32);
            [[32, 12n], [40, 13n], [48, 14n], [56, 15n], [65, 16n]]
                .forEach(([offset, value]) => view.setBigUint64(offset, value, true));
            bytes[64] = 1;
            return encoded(bytes);
        }
        if (functionName === 'get_graduation_status') {
            const bytes = new Uint8Array(113);
            const view = new DataView(bytes.buffer);
            bytes[0] = 3;
            bytes.fill(8, 17, 49);
            [[1, 1n], [9, 2n], [49, 3n], [57, 4n], [65, 5n], [73, 6n], [81, 7n], [89, 8n], [97, 9n], [105, 10n]]
                .forEach(([offset, value]) => view.setBigUint64(offset, value, true));
            return encoded(bytes);
        }
        throw new Error(`unexpected ${functionName}`);
    },
};

const client = new SporePumpClient(fakeConnection);
const signer = Keypair.fromSeed(Uint8Array.from({ length: 32 }, (_, index) => index));

const token = await client.getTokenInfo(1n);
assert.equal(token.marketCap, 44n);
const stats = await client.getPlatformStats();
assert.equal(stats.creatorRoyaltyBps, 11n);
const migrationToken = await client.getAccountingMigrationToken(1n);
assert.equal(migrationToken.creatorRoyalty, 16n);
assert.equal(migrationToken.lifecycleState, 1);
const graduation = await client.getGraduationStatus(1n);
assert.equal(graduation.reverseRouteId, 6n);
assert.equal(graduation.protocolTokenInventory, 10n);

await client.createToken(signer, { name: 'Moss Token', symbol: 'moss' });
await client.buy(signer, 7n, 1_000_000_000n, 99n);
await client.sell(signer, 7n, 100n, 88n);

assert.deepEqual(calls.map((call) => call.functionName), [
    'create_token_with_metadata',
    'buy_with_min_output',
    'sell_with_min_output',
]);
assert.equal(calls[0].value, SPOREPUMP_CREATION_FEE);
assert.deepEqual([...calls[0].args.slice(0, 7)], [0xAB, 32, 32, 4, 32, 4, 8]);
assert.equal(calls[1].value, 1_000_000_000n);
assert.deepEqual([...calls[1].args.slice(0, 5)], [0xAB, 32, 8, 8, 8]);

await assert.rejects(client.createToken(signer, { name: 'ok', symbol: '1BAD' }));
console.log('SporePump SDK exact-layout tests passed');
