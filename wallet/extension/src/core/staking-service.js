import { LichenRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { decryptPrivateKey } from './crypto-service.js';
import { buildAmountInstructionData, buildSignedSingleInstructionTransaction, encodeTransactionBase64 } from './tx-service.js';
import { baseUnitsToDecimalString, parsePositiveDecimalBaseUnits } from './amount-service.js';

const MAX_STAKING_AMOUNT_BASE_UNITS = 1_000_000_000n * 1_000_000_000n;

function validateAmount(amountLicn, label) {
  const baseUnits = parsePositiveDecimalBaseUnits(amountLicn, 9, label);
  if (baseUnits > MAX_STAKING_AMOUNT_BASE_UNITS) {
    throw new Error(`${label} is too large`);
  }
  return baseUnitsToDecimalString(baseUnits, 9);
}

export async function loadStakingSnapshot(address, network) {
  if (!address) return null;

  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  const position = await rpc.call('getStakingPosition', [address]).catch(() => null);

  const stLicn = Number(position?.st_licn_amount || 0) / 1_000_000_000;
  const rewards = Number(position?.unclaimed_rewards || 0) / 1_000_000_000;

  return {
    staked: stLicn,
    rewards,
    validator: position?.validator || null,
    active: stLicn > 0,
    raw: position
  };
}

export async function stakeLicn({ wallet, password, amountLicn, tier = 0, network }) {
  if (!wallet) throw new Error('No active wallet');
  const amount = validateAmount(amountLicn, 'Stake amount');
  const tierByte = Math.max(0, Math.min(3, Number(tier) || 0));
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));

  const latestBlock = await rpc.getLatestBlock();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  // Build 10-byte instruction: [opcode(1), amount_le(8), tier(1)]
  const instructionData = buildAmountInstructionData(13, amount, tierByte);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash: latestBlock.hash,
    programIdBytes: new Uint8Array(32), // SYSTEM_PROGRAM_ID = [0; 32]
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransaction(txBase64);
  return { txHash };
}

export async function unstakeStLicn({ wallet, password, amountLicn, network }) {
  if (!wallet) throw new Error('No active wallet');
  const amount = validateAmount(amountLicn, 'Unstake amount');
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));

  const latestBlock = await rpc.getLatestBlock();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  const instructionData = buildAmountInstructionData(14, amount);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash: latestBlock.hash,
    programIdBytes: new Uint8Array(32), // SYSTEM_PROGRAM_ID = [0; 32]
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransaction(txBase64);
  return { txHash };
}

export async function claimMossStake({ wallet, password, network }) {
  if (!wallet) throw new Error('No active wallet');
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));

  const latestBlock = await rpc.getLatestBlock();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  // Instruction type 15 = MossStakeClaim, no amount needed
  const instructionData = new Uint8Array([15]);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash: latestBlock.hash,
    programIdBytes: new Uint8Array(32),
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransaction(txBase64);
  return { txHash };
}
