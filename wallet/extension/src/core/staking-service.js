import { MoltChainRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { decryptPrivateKey } from './crypto-service.js';
import { buildAmountInstructionData, buildSignedSingleInstructionTransaction, encodeTransactionBase64 } from './tx-service.js';

function validateAmount(amountMolt, label) {
  const amount = Number(amountMolt);
  if (!Number.isFinite(amount) || amount <= 0) {
    throw new Error(`${label} must be a positive number`);
  }
  if (amount > 1_000_000_000) {
    throw new Error(`${label} is too large`);
  }
  return amount;
}

export async function loadStakingSnapshot(address, network) {
  if (!address) return null;

  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));
  const position = await rpc.call('getStakingPosition', [address]).catch(() => null);

  const stMolt = Number(position?.st_molt_amount || 0) / 1_000_000_000;
  const rewards = Number(position?.unclaimed_rewards || 0) / 1_000_000_000;

  return {
    staked: stMolt,
    rewards,
    validator: position?.validator || null,
    active: stMolt > 0,
    raw: position
  };
}

export async function stakeMolt({ wallet, password, amountMolt, tier = 0, network }) {
  if (!wallet) throw new Error('No active wallet');
  const amount = validateAmount(amountMolt, 'Stake amount');
  const tierByte = Math.max(0, Math.min(3, Number(tier) || 0));
  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));

  const latestBlock = await rpc.getLatestBlock();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  // Build 10-byte instruction: [opcode(1), amount_le(8), tier(1)]
  const instructionData = buildAmountInstructionData(13, amount, tierByte);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromPublicKeyHex: wallet.publicKey,
    blockhash: latestBlock.hash,
    programIdBytes: new Uint8Array(32), // SYSTEM_PROGRAM_ID = [0; 32]
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransaction(txBase64);
  return { txHash };
}

export async function unstakeStMolt({ wallet, password, amountMolt, network }) {
  if (!wallet) throw new Error('No active wallet');
  const amount = validateAmount(amountMolt, 'Unstake amount');
  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));

  const latestBlock = await rpc.getLatestBlock();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  const instructionData = buildAmountInstructionData(14, amount);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromPublicKeyHex: wallet.publicKey,
    blockhash: latestBlock.hash,
    programIdBytes: new Uint8Array(32), // SYSTEM_PROGRAM_ID = [0; 32]
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransaction(txBase64);
  return { txHash };
}

export async function claimReefStake({ wallet, password, network }) {
  if (!wallet) throw new Error('No active wallet');
  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));

  const latestBlock = await rpc.getLatestBlock();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  // Instruction type 15 = ReefStakeClaim, no amount needed
  const instructionData = new Uint8Array([15]);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromPublicKeyHex: wallet.publicKey,
    blockhash: latestBlock.hash,
    programIdBytes: new Uint8Array(32),
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransaction(txBase64);
  return { txHash };
}
