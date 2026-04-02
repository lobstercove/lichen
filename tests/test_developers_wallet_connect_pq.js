const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { pathToFileURL } = require('node:url');

(async function main() {
    const root = path.join(__dirname, '..');
    const walletConnectPath = path.join(root, 'developers', 'shared', 'wallet-connect.js');
    const walletConnectSource = fs.readFileSync(walletConnectPath, 'utf8');

    assert(
        walletConnectSource.includes('window.LichenPQ.generateKeypair'),
        'developers wallet-connect must use the PQ runtime fallback',
    );
    assert(
        !walletConnectSource.includes('window.nacl') && !walletConnectSource.includes('nacl.sign.keyPair'),
        'developers wallet-connect must not retain a classical NaCl fallback',
    );

    const pq = await import(pathToFileURL(path.join(root, 'monitoring', 'shared', 'pq.js')).href);

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

            if (payload.method === 'createWallet') {
                return {
                    json: async () => ({ error: { message: 'createWallet unavailable in test' } }),
                };
            }

            if (payload.method === 'getBalance') {
                return {
                    json: async () => ({ result: 0 }),
                };
            }

            throw new Error(`Unexpected RPC method during test: ${payload.method}`);
        },
    };

    context.window = context;
    context.LichenPQ = pq;
    context.bs58 = {
        encode: pq.base58Encode,
        decode: pq.base58Decode,
    };

    vm.createContext(context);
    vm.runInContext(walletConnectSource, context, { filename: walletConnectPath });

    assert.equal(typeof context.LichenWallet, 'function', 'LichenWallet constructor must be exported globally');

    const wallet = new context.LichenWallet({ rpcUrl: 'http://localhost:8899', persist: false });
    const info = await wallet.connect();

    assert.equal(rpcMethods[0], 'createWallet', 'wallet connect must try RPC wallet creation first');
    assert.equal(rpcMethods[1], 'getBalance', 'wallet connect must refresh balance after fallback wallet creation');
    assert.equal(info.balance, 0, 'wallet connect must return the fetched balance');
    assert.equal(wallet._walletData.hasKeys, true, 'fallback wallet must record that local keys exist');
    assert.equal(wallet.address, info.address, 'returned address must match wallet state');
    assert.equal(pq.addressToBytes(info.address).length, 32, 'fallback wallet must produce a valid native address');
    assert.equal(Buffer.from(wallet._walletData.publicKey, 'hex').length, pq.ML_DSA_65_PUBLIC_KEY_BYTES,
        'fallback wallet must persist the full PQ public key hex');

    console.log('developers-wallet-connect-pq: ok');
})().catch((error) => {
    console.error(error);
    process.exit(1);
});