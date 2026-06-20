import { LichenRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { decryptPrivateKey } from './crypto-service.js';
import { buildAmountInstructionData, buildSignedSingleInstructionTransaction, encodeTransactionBase64 } from './tx-service.js';
import { baseUnitsToDecimalString, parsePositiveDecimalBaseUnits } from './amount-service.js';

const MAX_STAKING_AMOUNT_BASE_UNITS = 1_000_000_000n * 1_000_000_000n;
const BASE_FEE_SPORES = 1_000_000n;

function baseUnitBigInt(value) {
  if (typeof value === 'bigint') return value > 0n ? value : 0n;
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value <= 0) return 0n;
    return BigInt(value);
  }
  const text = String(value ?? '0').trim();
  if (!/^\d+$/.test(text)) return 0n;
  return BigInt(text);
}

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
  const deposited = Number(position?.licn_deposited || 0) / 1_000_000_000;
  const redeemableValue = Number(position?.current_value_licn || 0) / 1_000_000_000;
  const rewards = position?.rewards_earned !== undefined && position?.rewards_earned !== null
    ? Number(position.rewards_earned) / 1_000_000_000
    : Math.max(0, redeemableValue - deposited);

  return {
    staked: stLicn,
    deposited,
    redeemableValue,
    rewards,
    validator: position?.validator || null,
    active: stLicn > 0 || redeemableValue > 0,
    raw: position
  };
}

export async function stakeLicn({ wallet, password, amountLicn, tier = 0, network }) {
  if (!wallet) throw new Error('No active wallet');
  const amount = validateAmount(amountLicn, 'Stake amount');
  const tierByte = Math.max(0, Math.min(3, Number(tier) || 0));
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));

  const blockhash = await rpc.getRecentBlockhash();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  // Build 10-byte instruction: [opcode(1), amount_le(8), tier(1)]
  const instructionData = buildAmountInstructionData(13, amount, tierByte);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash,
    programIdBytes: new Uint8Array(32), // SYSTEM_PROGRAM_ID = [0; 32]
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransactionWithPreflight(txBase64);
  return { txHash };
}

export async function unstakeStLicn({ wallet, password, amountLicn, network }) {
  if (!wallet) throw new Error('No active wallet');
  const amount = validateAmount(amountLicn, 'Unstake amount');
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));

  const blockhash = await rpc.getRecentBlockhash();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  const instructionData = buildAmountInstructionData(14, amount);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash,
    programIdBytes: new Uint8Array(32), // SYSTEM_PROGRAM_ID = [0; 32]
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransactionWithPreflight(txBase64);
  return { txHash };
}

export async function claimMossStake({ wallet, password, network }) {
  if (!wallet) throw new Error('No active wallet');
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  const balance = await rpc.getBalance(wallet.address);
  const spendable = baseUnitBigInt(balance?.spendable ?? balance?.available ?? balance?.spores ?? balance?.balance ?? 0);
  if (spendable < BASE_FEE_SPORES) {
    throw new Error(`Insufficient LICN for transaction fee (need ${baseUnitsToDecimalString(BASE_FEE_SPORES, 9)} LICN)`);
  }

  const blockhash = await rpc.getRecentBlockhash();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  // Instruction type 15 = MossStakeClaim, no amount needed
  const instructionData = new Uint8Array([15]);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash,
    programIdBytes: new Uint8Array(32),
    accountPubkeys: [],
    instructionDataBytes: instructionData
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransactionWithPreflight(txBase64);
  return { txHash };
}
