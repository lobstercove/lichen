const storage = new Map();
const stateKey = 'moltWalletState';

storage.set(stateKey, {
  schemaVersion: 1,
  wallets: [{ id: 'w1', address: '3P4oG8v8p3kFh2b9W6xY4qR2mN7tK5sL1cD9eF2gH7Jk', publicKey: '00', encryptedKey: 'enc' }],
  activeWalletId: 'w1',
  isLocked: false,
  settings: {},
  network: { selected: 'local-testnet' }
});

globalThis.chrome = {
  storage: {
    local: {
      async get(key) {
        if (!key) {
          const out = {};
          for (const [k, v] of storage.entries()) out[k] = v;
          return out;
        }
        if (typeof key === 'string') return { [key]: storage.get(key) };
        if (Array.isArray(key)) {
          const out = {};
          for (const k of key) out[k] = storage.get(k);
          return out;
        }
        return {};
      },
      async set(obj) {
        for (const [k, v] of Object.entries(obj || {})) storage.set(k, v);
      }
    }
  }
};

const { handleProviderRequest } = await import(new URL('../extension/src/core/provider-router.js', import.meta.url));

const ctx = {
  origin: 'https://example-dapp.test',
  network: 'local-testnet',
  activeAddress: '3P4oG8v8p3kFh2b9W6xY4qR2mN7tK5sL1cD9eF2gH7Jk',
  isLocked: false,
  appVersion: '0.1.0'
};

const checks = [
  ['eth_chainId', { method: 'eth_chainId' }, (r) => r.ok && r.result === '0x539'],
  ['net_version', { method: 'net_version' }, (r) => r.ok && r.result === '1337'],
  ['eth_coinbase', { method: 'eth_coinbase' }, (r) => r.ok],
  ['web3_clientVersion', { method: 'web3_clientVersion' }, (r) => r.ok && String(r.result).startsWith('MoltWallet/')],
  ['net_listening', { method: 'net_listening' }, (r) => r.ok && r.result === true],
  ['eth_getCode', { method: 'eth_getCode', params: ['0xabc', 'latest'] }, (r) => r.ok && r.result === '0x'],
  ['eth_estimateGas', { method: 'eth_estimateGas', params: [{ to: '0xabc' }] }, (r) => r.ok && r.result === '0x5208'],
  ['eth_gasPrice', { method: 'eth_gasPrice' }, (r) => r.ok && r.result === '0x3b9aca00'],
  ['wallet_watchAsset', { method: 'wallet_watchAsset', params: [{ type: 'ERC20', options: { symbol: 'USDC' } }] }, (r) => r.ok && r.result === true],
  ['wallet_switchEthereumChain->testnet', { method: 'wallet_switchEthereumChain', params: [{ chainId: '0x2711' }] }, (r) => r.ok],
  ['wallet_addEthereumChain->mainnet rpc', { method: 'wallet_addEthereumChain', params: [{ chainId: '0x2710', rpcUrls: ['https://rpc.moltchain.network'] }] }, (r) => r.ok],
  ['eth_getBalance alias shape', { method: 'eth_getBalance', params: ['3P4oG8v8p3kFh2b9W6xY4qR2mN7tK5sL1cD9eF2gH7Jk', 'latest'] }, (r) => r.ok || !r.ok],
  ['eth_getTransactionCount alias shape', { method: 'eth_getTransactionCount', params: ['3P4oG8v8p3kFh2b9W6xY4qR2mN7tK5sL1cD9eF2gH7Jk', 'latest'] }, (r) => r.ok || !r.ok]
];

const results = [];
for (const [name, payload, validator] of checks) {
  try {
    const response = await handleProviderRequest(payload, ctx);
    results.push({ name, ok: !!validator(response), response });
  } catch (error) {
    results.push({ name, ok: false, error: String(error?.message || error) });
  }
}

const postState = storage.get(stateKey);
const summary = {
  pass: results.filter((r) => r.ok).length,
  fail: results.filter((r) => !r.ok).length,
  selectedNetwork: postState?.network?.selected,
  mainnetRPC: postState?.settings?.mainnetRPC || null,
  results
};

console.log(JSON.stringify(summary, null, 2));
process.exit(summary.fail ? 1 : 0);
