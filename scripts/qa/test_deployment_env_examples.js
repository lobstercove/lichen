#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..', '..');

function read(relPath) {
  return fs.readFileSync(path.join(root, relPath), 'utf8');
}

function fail(message) {
  throw new Error(message);
}

function assert(condition, message) {
  if (!condition) {
    fail(message);
  }
}

function parseExampleEnv(relPath) {
  const active = new Map();
  const declared = new Set();
  const lines = read(relPath).split(/\r?\n/);

  for (const line of lines) {
    let body = line.trim();
    if (!body) {
      continue;
    }
    let commented = false;
    if (body.startsWith('#')) {
      commented = true;
      body = body.slice(1).trimStart();
    }

    const match = body.match(/^([A-Z][A-Z0-9_]+)=(.*)$/);
    if (!match) {
      continue;
    }

    const [, key, value] = match;
    declared.add(key);
    if (!commented) {
      active.set(key, value);
    }
  }

  return { active, declared };
}

function parseSystemdUnit(relPath) {
  const source = read(relPath);
  const envFiles = [];
  const inlineEnv = new Map();
  const execVars = new Set();

  for (const line of source.split(/\r?\n/)) {
    const envFile = line.match(/^\s*EnvironmentFile=(\S+)/);
    if (envFile) {
      envFiles.push(envFile[1]);
    }
    const environment = line.match(/^\s*Environment=([A-Z][A-Z0-9_]+)=(.*)$/);
    if (environment) {
      inlineEnv.set(environment[1], environment[2]);
    }
    for (const match of line.matchAll(/\$\{([A-Z][A-Z0-9_]+)\}/g)) {
      execVars.add(match[1]);
    }
  }

  return { envFiles, inlineEnv, execVars };
}

function heredocKeys(startMarker, endMarker, fallbackKeys) {
  const setupPath = path.join(root, 'deploy/setup.sh');
  if (
    process.env.LICHEN_QA_FORCE_SETUP_FALLBACK === '1' ||
    !fs.existsSync(setupPath)
  ) {
    return new Set(fallbackKeys);
  }

  const source = fs.readFileSync(setupPath, 'utf8');
  const start = source.indexOf(startMarker);
  assert(start >= 0, `deploy/setup.sh missing heredoc start: ${startMarker}`);
  const blockStart = source.indexOf('\n', start) + 1;
  const end = source.indexOf(`\n${endMarker}`, blockStart);
  assert(end >= 0, `deploy/setup.sh missing heredoc end: ${endMarker}`);

  return new Set(
    source
      .slice(blockStart, end)
      .split(/\r?\n/)
      .map((line) => line.match(/^([A-Z][A-Z0-9_]+)=/))
      .filter(Boolean)
      .map((match) => match[1]),
  );
}

function requireActiveKeys(env, keys, label) {
  for (const key of keys) {
    assert(env.active.has(key), `${label} missing active ${key}`);
  }
}

function requireDeclaredKeys(env, keys, label) {
  for (const key of keys) {
    assert(env.declared.has(key), `${label} missing documented ${key}`);
  }
}

function requireValues(env, expected, label) {
  for (const [key, value] of Object.entries(expected)) {
    assert(env.active.get(key) === value, `${label} ${key} expected ${value}`);
  }
}

function requireRedactedSecrets(env, label) {
  const secretKeys = new Set([
    'CUSTODY_API_AUTH_TOKEN',
    'CUSTODY_SIGNER_AUTH_TOKEN',
    'LICHEN_KEYPAIR_PASSWORD',
    'LICHEN_SIGNER_AUTH_TOKEN',
  ]);
  for (const [key, value] of env.active.entries()) {
    if (secretKeys.has(key)) {
      assert(
        /^REPLACE_WITH_[A-Z0-9_]+$/.test(value),
        `${label} ${key} must use a REPLACE_WITH_ redacted placeholder`,
      );
    }
  }
}

function requireNoInlineCustodySeeds(env, label) {
  for (const key of ['CUSTODY_MASTER_SEED', 'CUSTODY_DEPOSIT_MASTER_SEED']) {
    assert(!env.active.has(key), `${label} must not inline ${key}`);
  }
}

const validatorUnit = parseSystemdUnit('deploy/lichen-validator.service');
const custodyUnit = parseSystemdUnit('deploy/lichen-custody.service');
const custodyMainnetUnit = parseSystemdUnit('deploy/lichen-custody-mainnet.service');
const faucetUnit = parseSystemdUnit('deploy/lichen-faucet.service');

assert(
  validatorUnit.envFiles.includes('/etc/lichen/env'),
  'validator unit must keep /etc/lichen/env EnvironmentFile contract',
);
assert(
  custodyUnit.envFiles.includes('/etc/lichen/custody-env'),
  'custody testnet unit must keep /etc/lichen/custody-env contract',
);
assert(
  custodyMainnetUnit.envFiles.includes('/etc/lichen/custody-env-mainnet'),
  'custody mainnet unit must keep /etc/lichen/custody-env-mainnet contract',
);
assert(faucetUnit.envFiles.length === 0, 'faucet unit should remain inline-env only');

const setupValidatorKeys = heredocKeys('cat > "$ENV_FILE" <<EOF', 'EOF', [
  'LICHEN_NETWORK',
  'LICHEN_RPC_PORT',
  'LICHEN_WS_PORT',
  'LICHEN_P2P_PORT',
  'LICHEN_EXTERNAL_ADDR',
  'LICHEN_KEYPAIR_PASSWORD',
  'LICHEN_SIGNER_BIND',
  'LICHEN_SIGNER_AUTH_TOKEN',
  'LICHEN_CONTRACTS_DIR',
  'RUST_LOG',
  'LICHEN_INCIDENT_STATUS_FILE',
  'LICHEN_SIGNED_METADATA_MANIFEST_FILE',
  'LICHEN_SERVICE_FLEET_CONFIG_FILE',
  'LICHEN_SERVICE_FLEET_UPSTREAM_RPC_URL',
  'LICHEN_SERVICE_FLEET_STATUS_FILE',
  'LICHEN_EXTRA_ARGS',
]);
const setupCustodyKeys = heredocKeys(
  'cat > "$CONFIG_DIR/$CUSTODY_ENV_NAME" <<CUSTEOF',
  'CUSTEOF',
  [
    'CUSTODY_DB_PATH',
    'CUSTODY_API_AUTH_TOKEN',
    'CUSTODY_MASTER_SEED_FILE',
    'CUSTODY_DEPOSIT_MASTER_SEED_FILE',
    'CUSTODY_LICHEN_RPC_URL',
    'CUSTODY_POLL_INTERVAL_SECS',
    'CUSTODY_DEPOSIT_TTL_SECS',
    'CUSTODY_LISTEN_PORT',
    'CUSTODY_SIGNER_AUTH_TOKEN',
    'LICHEN_KEYPAIR_PASSWORD',
    'LICHEN_INCIDENT_STATUS_FILE',
    'RUST_LOG',
    'CUSTODY_TREASURY_KEYPAIR',
  ],
);

const validatorRpcCustodyKeys = ['CUSTODY_URL', 'CUSTODY_API_AUTH_TOKEN'];
const validatorExpected = {
  'deploy/env-testnet.example': {
    LICHEN_NETWORK: 'testnet',
    LICHEN_RPC_PORT: '8899',
    LICHEN_WS_PORT: '8900',
    LICHEN_P2P_PORT: '7001',
    LICHEN_SIGNER_BIND: '127.0.0.1:9201',
    LICHEN_CONTRACTS_DIR: '/var/lib/lichen/contracts',
    LICHEN_INCIDENT_STATUS_FILE: '/etc/lichen/incident-status-testnet.json',
    LICHEN_SIGNED_METADATA_MANIFEST_FILE:
      '/etc/lichen/signed-metadata-manifest-testnet.json',
    LICHEN_SERVICE_FLEET_CONFIG_FILE: '/etc/lichen/service-fleet-testnet.json',
    LICHEN_SERVICE_FLEET_STATUS_FILE:
      '/var/lib/lichen/service-fleet-status-testnet.json',
    LICHEN_EXTRA_ARGS: '--auto-update=off',
    CUSTODY_URL: 'http://127.0.0.1:9105',
  },
  'deploy/env-mainnet.example': {
    LICHEN_NETWORK: 'mainnet',
    LICHEN_RPC_PORT: '9899',
    LICHEN_WS_PORT: '9900',
    LICHEN_P2P_PORT: '8001',
    LICHEN_SIGNER_BIND: '127.0.0.1:9201',
    LICHEN_CONTRACTS_DIR: '/var/lib/lichen/contracts',
    LICHEN_INCIDENT_STATUS_FILE: '/etc/lichen/incident-status-mainnet.json',
    LICHEN_SIGNED_METADATA_MANIFEST_FILE:
      '/etc/lichen/signed-metadata-manifest-mainnet.json',
    LICHEN_SERVICE_FLEET_CONFIG_FILE: '/etc/lichen/service-fleet-mainnet.json',
    LICHEN_SERVICE_FLEET_STATUS_FILE:
      '/var/lib/lichen/service-fleet-status-mainnet.json',
    LICHEN_EXTRA_ARGS: '--auto-update=off',
    CUSTODY_URL: 'http://127.0.0.1:9106',
  },
};

for (const [relPath, expected] of Object.entries(validatorExpected)) {
  const env = parseExampleEnv(relPath);
  requireActiveKeys(env, setupValidatorKeys, relPath);
  requireActiveKeys(env, validatorUnit.execVars, relPath);
  requireActiveKeys(env, validatorRpcCustodyKeys, relPath);
  requireValues(env, expected, relPath);
  requireRedactedSecrets(env, relPath);
}

const custodyRouteKeys = [
  'CUSTODY_SOLANA_RPC_URL',
  'CUSTODY_ETH_RPC_URL',
  'CUSTODY_BNB_RPC_URL',
  'CUSTODY_EVM_RPC_URL',
  'CUSTODY_NEOX_RPC_URL',
  'CUSTODY_NEOX_CHAIN_ID',
  'CUSTODY_SOLANA_CONFIRMATIONS',
  'CUSTODY_EVM_CONFIRMATIONS',
  'CUSTODY_NEOX_CONFIRMATIONS',
  'CUSTODY_TREASURY_SOLANA',
  'CUSTODY_TREASURY_ETH',
  'CUSTODY_TREASURY_BNB',
  'CUSTODY_TREASURY_EVM',
  'CUSTODY_TREASURY_NEOX',
  'CUSTODY_SOLANA_FEE_PAYER',
  'CUSTODY_SOLANA_TREASURY_OWNER',
  'CUSTODY_SOLANA_USDC_MINT',
  'CUSTODY_SOLANA_USDT_MINT',
  'CUSTODY_ETH_USDC_TOKEN_ADDR',
  'CUSTODY_ETH_USDT_TOKEN_ADDR',
  'CUSTODY_BSC_USDC_TOKEN_ADDR',
  'CUSTODY_BSC_USDT_TOKEN_ADDR',
  'CUSTODY_LUSD_TOKEN_ADDR',
  'CUSTODY_WSOL_TOKEN_ADDR',
  'CUSTODY_WETH_TOKEN_ADDR',
  'CUSTODY_WBNB_TOKEN_ADDR',
  'CUSTODY_WGAS_TOKEN_ADDR',
  'CUSTODY_WNEO_TOKEN_ADDR',
  'CUSTODY_NEOX_NEO_TOKEN_ADDR',
  'CUSTODY_REBALANCE_THRESHOLD_BPS',
  'CUSTODY_REBALANCE_TARGET_BPS',
  'CUSTODY_REBALANCE_MAX_SLIPPAGE_BPS',
  'CUSTODY_JUPITER_API_URL',
  'CUSTODY_UNISWAP_ROUTER',
  'CUSTODY_SIGNER_ENDPOINTS',
  'CUSTODY_SIGNER_THRESHOLD',
  'CUSTODY_SIGNER_PQ_ADDRESSES',
  'CUSTODY_SIGNER_AUTH_TOKENS',
  'CUSTODY_WEBHOOK_ALLOWED_HOSTS',
  'CUSTODY_WEBHOOK_MAX_INFLIGHT',
  'CUSTODY_WITHDRAWAL_PENDING_BURN_TTL_SECS',
  'CUSTODY_EVM_MULTISIG_ADDRESS',
  'CUSTODY_NEOX_MULTISIG_ADDRESS',
  'CUSTODY_OPERATOR_CONFIRMATION_TOKENS',
  'CUSTODY_WITHDRAWAL_ELEVATED_DELAY_SECS',
  'CUSTODY_WITHDRAWAL_EXTRAORDINARY_DELAY_SECS',
  'CUSTODY_ALLOW_INSECURE_SEED',
  'CUSTODY_ALLOW_LOCAL_WEBHOOKS',
  'CUSTODY_WS_EVENTS_ALLOW_QUERY_TOKEN',
  'LICHEN_LOCAL_DEV',
  'DEV_CORS',
];

const custodyExpected = {
  'deploy/custody-env.example': {
    CUSTODY_DB_PATH: '/var/lib/lichen/custody-db',
    CUSTODY_MASTER_SEED_FILE:
      '/etc/lichen/secrets/custody-master-seed-testnet.txt',
    CUSTODY_DEPOSIT_MASTER_SEED_FILE:
      '/etc/lichen/secrets/custody-deposit-seed-testnet.txt',
    CUSTODY_LICHEN_RPC_URL: 'http://127.0.0.1:8899',
    CUSTODY_POLL_INTERVAL_SECS: '15',
    CUSTODY_DEPOSIT_TTL_SECS: '86400',
    CUSTODY_LISTEN_PORT: '9105',
    LICHEN_INCIDENT_STATUS_FILE: '/etc/lichen/incident-status-testnet.json',
    RUST_LOG: 'info',
    CUSTODY_TREASURY_KEYPAIR: '/etc/lichen/custody-treasury-testnet.json',
  },
  'deploy/custody-env-mainnet.example': {
    CUSTODY_DB_PATH: '/var/lib/lichen/custody-db-mainnet',
    CUSTODY_MASTER_SEED_FILE:
      '/etc/lichen/secrets/custody-master-seed-mainnet.txt',
    CUSTODY_DEPOSIT_MASTER_SEED_FILE:
      '/etc/lichen/secrets/custody-deposit-seed-mainnet.txt',
    CUSTODY_LICHEN_RPC_URL: 'http://127.0.0.1:9899',
    CUSTODY_POLL_INTERVAL_SECS: '15',
    CUSTODY_DEPOSIT_TTL_SECS: '86400',
    CUSTODY_LISTEN_PORT: '9106',
    LICHEN_INCIDENT_STATUS_FILE: '/etc/lichen/incident-status-mainnet.json',
    RUST_LOG: 'info',
    CUSTODY_TREASURY_KEYPAIR: '/etc/lichen/custody-treasury-mainnet.json',
  },
};

for (const [relPath, expected] of Object.entries(custodyExpected)) {
  const env = parseExampleEnv(relPath);
  requireActiveKeys(env, setupCustodyKeys, relPath);
  requireDeclaredKeys(env, custodyRouteKeys, relPath);
  requireValues(env, expected, relPath);
  requireRedactedSecrets(env, relPath);
  requireNoInlineCustodySeeds(env, relPath);
}

const faucetExample = parseExampleEnv('deploy/faucet-env.example');
for (const [key, value] of faucetUnit.inlineEnv.entries()) {
  assert(faucetExample.active.get(key) === value, `faucet example drifted for ${key}`);
}
assert(
  faucetExample.active.size === faucetUnit.inlineEnv.size,
  'faucet example should contain only active keys from the systemd unit',
);

const ci = read('.github/workflows/ci.yml');
assert(
  ci.includes('node scripts/qa/test_deployment_env_examples.js'),
  'CI expected-contracts job must run deployment env example QA',
);

const pkg = JSON.parse(read('package.json'));
assert(
  pkg.scripts['test-deployment-docs'].includes(
    'node scripts/qa/test_deployment_env_examples.js',
  ),
  'npm test-deployment-docs must include deployment env example QA',
);

console.log('deployment env examples match systemd and setup contracts');
