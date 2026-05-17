import assert from 'node:assert/strict';
import {
  BRIDGE_ASSETS,
  BRIDGE_CHAINS,
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
