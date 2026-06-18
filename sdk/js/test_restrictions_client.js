import assert from 'node:assert/strict';
import {
  BRIDGE_ASSETS,
  BRIDGE_CHAINS,
  Connection,
  PublicKey,
  RestrictionGovernanceClient,
} from './dist/index.js';

class MockConnection {
  calls = [];

  async rpcRequest(method, params = []) {
    this.calls.push({ method, params });
    return {
      method,
      params,
      unsigned: true,
      encoding: 'base64',
      wire_format: 'lichen_tx_v1',
      tx_type: 'native',
      transaction_base64: 'AA==',
      transaction: 'AA==',
      wire_size: 1,
      message_hash: '00',
      signature_count: 0,
      recent_blockhash: '00',
      proposer: '',
      governance_authority: '',
      action_label: 'restrict',
      action: {},
      instruction: {
        program_id: '',
        accounts: [],
        instruction_type: 34,
        governance_action_type: 10,
        data_hex: '',
      },
    };
  }
}

function key(byte) {
  return new PublicKey(new Uint8Array(32).fill(byte));
}

const connection = new MockConnection();
const client = new RestrictionGovernanceClient(connection);
const proposer = key(1);
const authority = key(2);
const account = key(3);
const recipient = key(4);
const asset = key(5);

await client.getAccountRestrictionStatus(account);
assert.deepEqual(connection.calls.at(-1), {
  method: 'getAccountRestrictionStatus',
  params: [account.toBase58()],
});

await client.getRestrictionStatus({ type: 'account_asset', account, asset: 'native' });
assert.deepEqual(connection.calls.at(-1), {
  method: 'getRestrictionStatus',
  params: [{ type: 'account_asset', account: account.toBase58(), asset: 'native' }],
});

await client.canTransfer({ from: account, to: recipient, asset, amount: 25n });
assert.deepEqual(connection.calls.at(-1), {
  method: 'canTransfer',
  params: [{
    from: account.toBase58(),
    to: recipient.toBase58(),
    asset: asset.toBase58(),
    amount: '25',
  }],
});

await client.buildRestrictAccountTx({
  proposer,
  governanceAuthority: authority,
  account,
  mode: 'outgoing_only',
  reason: 'testnet_drill',
  evidenceHash: 'aa'.repeat(32),
  expiresAtSlot: 123n,
  recentBlockhash: 'bb'.repeat(32),
});
assert.deepEqual(connection.calls.at(-1), {
  method: 'buildRestrictAccountTx',
  params: [{
    proposer: proposer.toBase58(),
    governance_authority: authority.toBase58(),
    recent_blockhash: 'bb'.repeat(32),
    reason: 'testnet_drill',
    evidence_hash: 'aa'.repeat(32),
    expires_at_slot: '123',
    account: account.toBase58(),
    mode: 'outgoing_only',
  }],
});

await client.buildSetFrozenAssetAmountTx({
  proposer,
  governanceAuthority: authority,
  account,
  asset,
  amount: 500n,
  reason: 'testnet_drill',
});
assert.deepEqual(connection.calls.at(-1), {
  method: 'buildSetFrozenAssetAmountTx',
  params: [{
    proposer: proposer.toBase58(),
    governance_authority: authority.toBase58(),
    reason: 'testnet_drill',
    account: account.toBase58(),
    asset: asset.toBase58(),
    amount: '500',
  }],
});

await client.buildResumeBridgeRouteTx({
  proposer,
  governanceAuthority: authority,
  chain: BRIDGE_CHAINS.NEOX,
  asset: BRIDGE_ASSETS.GAS,
  restrictionId: 12,
  liftReason: 'testnet_drill_complete',
});
assert.deepEqual(connection.calls.at(-1), {
  method: 'buildResumeBridgeRouteTx',
  params: [{
    proposer: proposer.toBase58(),
    governance_authority: authority.toBase58(),
    chain: 'neox',
    asset: 'gas',
    restriction_id: 12,
    lift_reason: 'testnet_drill_complete',
  }],
});

await assert.rejects(
  () => client.buildLiftRestrictionTx({
    proposer,
    governanceAuthority: authority,
    restrictionId: -1,
    liftReason: 'false_positive',
  }),
  /restrictionId must be a u64-safe integer value/,
);

const originalFetch = globalThis.fetch;
const rpcRequests = [];
globalThis.fetch = async (_url, init) => {
  const body = JSON.parse(init.body);
  rpcRequests.push(body);
  const page = rpcRequests.length === 1
    ? {
        contracts: [{ program_id: 'first', code_size: 1, is_executable: true, has_abi: false, abi_functions: 0, code_hash: '', version: 1, lifecycle_status: 'active', lifecycle_updated_slot: 0, lifecycle_effective_at_slot: 0 }],
        count: 1,
        has_more: true,
        next_cursor: 'cursor-1',
      }
    : {
        contracts: [{ program_id: 'second', code_size: 1, is_executable: true, has_abi: false, abi_functions: 0, code_hash: '', version: 1, lifecycle_status: 'active', lifecycle_updated_slot: 0, lifecycle_effective_at_slot: 0 }],
        count: 1,
        has_more: false,
        next_cursor: null,
      };
  return new Response(JSON.stringify({ jsonrpc: '2.0', id: body.id, result: page }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
};

try {
  const sdkConnection = new Connection('http://mock-rpc');
  const contracts = await sdkConnection.getAllContractsAll(1);
  assert.deepEqual(contracts.map((entry) => entry.program_id), ['first', 'second']);
  assert.deepEqual(rpcRequests.map((request) => request.params), [
    [{ limit: 1 }],
    [{ limit: 1, cursor: 'cursor-1' }],
  ]);
} finally {
  globalThis.fetch = originalFetch;
}
