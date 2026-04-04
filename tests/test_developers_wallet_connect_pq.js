const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

(async function main() {
    const root = path.join(__dirname, '..');
    const walletConnectPath = path.join(root, 'developers', 'shared', 'wallet-connect.js');
    const walletConnectSource = fs.readFileSync(walletConnectPath, 'utf8');

    assert(
        !walletConnectSource.includes('window.LichenPQ.generateKeypair'),
        'developers wallet-connect must not use the PQ runtime fallback',
    );
    assert(
        !walletConnectSource.includes("lichenRpcCall('createWallet'"),
        'developers wallet-connect must not retain RPC wallet creation fallback',
    );

    const storage = new Map();
    const rpcMethods = [];
    const context = {
        console,
        Date,
        JSON,
        setInterval: () => 0,
        clearInterval: () => { },
        localStorage: {
            getItem(key) {
                return storage.has(key) ? storage.get(key) : null;
            },
            setItem(key, value) {
                storage.set(key, String(value));
            },
            removeItem(key) {
                storage.delete(key);
            },
        },
        fetch: async (_url, options) => {
            const payload = JSON.parse(options.body);
            rpcMethods.push(payload.method);

            if (payload.method === 'getBalance') {
                return {
                    json: async () => ({ result: 0 }),
                };
            }

            throw new Error(`Unexpected RPC method during test: ${payload.method}`);
        },
    };

    context.window = context;
    context.bs58 = {
        encode(value) {
            return String(value || '');
        },
        decode() {
            return new Uint8Array();
        },
    };

    vm.createContext(context);
    vm.runInContext(walletConnectSource, context, { filename: walletConnectPath });

    assert.equal(typeof context.LichenWallet, 'function', 'LichenWallet constructor must be exported globally');

    const wallet = new context.LichenWallet({ rpcUrl: 'http://localhost:8899', persist: false });
    await assert.rejects(
        wallet.connect(),
        /extension not found|Use the Lichen wallet extension/i,
        'wallet connect must fail closed when no extension is present',
    );
    assert.deepStrictEqual(rpcMethods, [], 'wallet connect must not hit RPC wallet creation or balance refresh without an extension');

    console.log('developers-wallet-connect-extension-only: ok');
})().catch((error) => {
    console.error(error);
    process.exit(1);
});