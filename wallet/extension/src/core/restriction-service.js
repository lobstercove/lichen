import { base58Decode, base58Encode } from './crypto-service.js';
import { LichenRPC, getTrustedRpcEndpoint } from './rpc-service.js';

export const NATIVE_RESTRICTION_ASSET = 'native';
export const RESTRICTION_METHODS = Object.freeze({
  targetStatus: 'getRestrictionStatus',
  accountStatus: 'getAccountRestrictionStatus',
  assetStatus: 'getAssetRestrictionStatus',
  accountAssetStatus: 'getAccountAssetRestrictionStatus',
  contractLifecycleStatus: 'getContractLifecycleStatus',
  incidentStatus: 'getIncidentStatus',
  canSend: 'canSend',
  canReceive: 'canReceive',
  canTransfer: 'canTransfer'
});

const SYSTEM_PROGRAM_BYTES = new Uint8Array(32);
const CONTRACT_PROGRAM_BYTES = new Uint8Array(32).fill(255);
const BLOCKED_LIFECYCLE_STATUSES = new Set(['suspended', 'quarantined', 'terminated']);

export function getTrustedRestrictionRpc(network = 'local-testnet') {
  return new LichenRPC(getTrustedRpcEndpoint(network || 'local-testnet'));
}

export function restrictionStatusIsActive(status) {
  if (!status || typeof status !== 'object') return false;
  if (status.active === true || status.restricted === true || status.blocked === true || status.allowed === false) {
    return true;
  }
  const activeIds = status.active_restriction_ids;
  return Array.isArray(activeIds) && activeIds.length > 0;
}

export function restrictionStatusIds(status) {
  if (!status || typeof status !== 'object') {
    return [];
  }
  const ids = [];
  for (const key of ['active_restriction_ids', 'source_restriction_ids', 'recipient_restriction_ids']) {
    if (Array.isArray(status[key])) ids.push(...status[key]);
  }
  if (status.restriction_id !== undefined && status.restriction_id !== null) ids.push(status.restriction_id);
  return uniqueStrings(ids.map((id) => String(id)));
}

function uniqueStrings(values) {
  const seen = new Set();
  const out = [];
  for (const value of values) {
    const text = String(value || '').trim();
    if (!text || seen.has(text)) continue;
    seen.add(text);
    out.push(text);
  }
  return out;
}

function restrictionIdsLabel(status) {
  const ids = restrictionStatusIds(status);
  if (!ids.length) return '';
  const head = ids.slice(0, 3).map((id) => `#${id}`).join(', ');
  return ids.length > 3 ? `${head} +${ids.length - 3}` : head;
}

function makeRestrictionLabel(prefix, status) {
  const ids = restrictionIdsLabel(status);
  return ids ? `${prefix} ${ids}` : prefix;
}

export function extensionRestrictionStatusItems(status) {
  if (!status) return [];
  const items = [];
  if (restrictionStatusIsActive(status.accountStatus)) {
    items.push(makeRestrictionLabel('Account restriction active', status.accountStatus));
  }
  if (restrictionStatusIsActive(status.nativeAssetStatus)) {
    items.push(makeRestrictionLabel('LICN asset restriction active', status.nativeAssetStatus));
  }
  if (restrictionStatusIsActive(status.nativeAccountAssetStatus)) {
    items.push(makeRestrictionLabel('LICN account-asset restriction active', status.nativeAccountAssetStatus));
  }
  if (status.nativeCanSend?.allowed === false || status.nativeCanSend?.blocked === true) {
    items.push(makeRestrictionLabel('LICN send blocked', status.nativeCanSend));
  }
  if (status.nativeCanReceive?.allowed === false || status.nativeCanReceive?.blocked === true) {
    items.push(makeRestrictionLabel('LICN receive blocked', status.nativeCanReceive));
  }
  return uniqueStrings(items);
}

async function safeTrustedCall(rpc, method, params, errors) {
  try {
    return await rpc.call(method, params);
  } catch (error) {
    if (Array.isArray(errors)) errors.push(error?.message || String(error));
    return null;
  }
}

export async function loadExtensionRestrictionStatus({ account, network = 'local-testnet' }) {
  const address = String(account || '').trim();
  if (!address) {
    return {
      account: null,
      network,
      updatedAt: Date.now(),
      unavailable: true,
      criticalErrors: ['Missing wallet address']
    };
  }

  const rpc = getTrustedRestrictionRpc(network);
  const criticalErrors = [];
  const optionalErrors = [];
  const [
    incidentStatus,
    accountStatus,
    nativeAssetStatus,
    nativeAccountAssetStatus,
    nativeCanSend,
    nativeCanReceive
  ] = await Promise.all([
    safeTrustedCall(rpc, RESTRICTION_METHODS.incidentStatus, [], optionalErrors),
    safeTrustedCall(rpc, RESTRICTION_METHODS.accountStatus, [address], criticalErrors),
    safeTrustedCall(rpc, RESTRICTION_METHODS.assetStatus, [{ asset: NATIVE_RESTRICTION_ASSET }], criticalErrors),
    safeTrustedCall(rpc, RESTRICTION_METHODS.accountAssetStatus, [{
      account: address,
      asset: NATIVE_RESTRICTION_ASSET
    }], criticalErrors),
    safeTrustedCall(rpc, RESTRICTION_METHODS.canSend, [{
      account: address,
      asset: NATIVE_RESTRICTION_ASSET,
      amount: 0
    }], criticalErrors),
    safeTrustedCall(rpc, RESTRICTION_METHODS.canReceive, [{
      account: address,
      asset: NATIVE_RESTRICTION_ASSET,
      amount: 0
    }], criticalErrors)
  ]);

  const status = {
    account: address,
    network,
    updatedAt: Date.now(),
    incidentStatus,
    accountStatus,
    nativeAssetStatus,
    nativeAccountAssetStatus,
    nativeCanSend,
    nativeCanReceive,
    criticalErrors,
    optionalErrors
  };
  status.blocked = extensionRestrictionStatusItems(status).length > 0;
  status.unavailable = criticalErrors.length > 0;
  return status;
}

function bytesFromValue(value) {
  if (value instanceof Uint8Array) return value;
  if (Array.isArray(value)) return Uint8Array.from(value);
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (/^0x[0-9a-fA-F]+$/.test(trimmed)) {
      const hex = trimmed.slice(2);
      return Uint8Array.from(hex.match(/.{1,2}/g).map((part) => parseInt(part, 16)));
    }
    try {
      const decoded = base58Decode(trimmed);
      if (decoded.length === 32) return decoded;
    } catch {
      // Fall through to UTF-8; instruction data can be JSON text.
    }
    return new TextEncoder().encode(trimmed);
  }
  return new Uint8Array(0);
}

function pubkeyToAddress(value) {
  const bytes = bytesFromValue(value);
  if (bytes.length !== 32) return null;
  return base58Encode(bytes);
}

function bytesEqual(a, b) {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function readU64LEString(bytes, offset) {
  let value = 0n;
  for (let i = 0; i < 8; i++) {
    value |= BigInt(bytes[offset + i] || 0) << BigInt(i * 8);
  }
  return value.toString();
}

function normalizeInstruction(instruction) {
  return {
    programId: bytesFromValue(instruction?.program_id ?? instruction?.programId),
    accounts: Array.isArray(instruction?.accounts) ? instruction.accounts : [],
    data: bytesFromValue(instruction?.data)
  };
}

function decodeJsonBytes(bytes) {
  if (!bytes || !bytes.length) return null;
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return null;
  }
}

function normalizeAddressArg(value) {
  if (typeof value === 'string') {
    const decoded = pubkeyToAddress(value);
    return decoded || value;
  }
  if (Array.isArray(value)) return pubkeyToAddress(value);
  if (value && typeof value === 'object') {
    return normalizeAddressArg(value.address ?? value.pubkey ?? value.publicKey ?? value.bytes);
  }
  return null;
}

function amountArgToString(value) {
  if (typeof value === 'bigint') return value >= 0n ? value.toString() : null;
  if (typeof value === 'number') {
    if (!Number.isFinite(value) || value < 0) return null;
    return Math.floor(value).toString();
  }
  if (typeof value === 'string') {
    const trimmed = value.trim();
    return /^\d+$/.test(trimmed) ? trimmed : null;
  }
  if (value && typeof value === 'object') {
    return amountArgToString(value.amount ?? value.value ?? value.spores);
  }
  return null;
}

function decodeContractCall(data) {
  const decoded = decodeJsonBytes(data);
  const call = decoded?.Call || decoded?.call || null;
  if (!call || typeof call !== 'object') return null;

  let args = call.args;
  if (Array.isArray(args)) {
    const parsed = decodeJsonBytes(Uint8Array.from(args));
    if (parsed !== null) args = parsed;
  }

  return {
    functionName: String(call.function || call.function_name || call.method || '').trim(),
    args: Array.isArray(args) ? args : [],
    value: amountArgToString(call.value ?? 0) || '0'
  };
}

function addContractTransferTarget(targets, contract, caller, call) {
  const functionName = String(call?.functionName || '').toLowerCase();
  if (functionName === 'transfer' && call.args.length >= 2) {
    const to = normalizeAddressArg(call.args[0]);
    const amount = amountArgToString(call.args[1]);
    if (caller && to && amount) {
      targets.transfers.push({
        kind: 'contract_token_transfer',
        from: caller,
        to,
        asset: contract,
        amount,
        label: 'contract token transfer'
      });
    }
  }

  if (functionName === 'transfer_from' && call.args.length >= 3) {
    const from = normalizeAddressArg(call.args[0]);
    const to = normalizeAddressArg(call.args[1]);
    const amount = amountArgToString(call.args[2]);
    if (from && to && amount) {
      targets.transfers.push({
        kind: 'contract_token_transfer_from',
        from,
        to,
        asset: contract,
        amount,
        label: 'contract token transfer_from'
      });
    }
  }
}

export function analyzeRestrictionPreflightTargets(transaction, fallbackFromAddress = null) {
  const message = transaction?.message || transaction;
  const instructions = Array.isArray(message?.instructions) ? message.instructions : [];
  const targets = {
    transfers: [],
    contracts: []
  };

  for (const rawInstruction of instructions) {
    const instruction = normalizeInstruction(rawInstruction);
    if (bytesEqual(instruction.programId, SYSTEM_PROGRAM_BYTES)) {
      if (instruction.data.length >= 9 && instruction.data[0] === 0 && instruction.accounts.length >= 2) {
        const from = pubkeyToAddress(instruction.accounts[0]);
        const to = pubkeyToAddress(instruction.accounts[1]);
        const amount = readU64LEString(instruction.data, 1);
        if (from && to) {
          targets.transfers.push({
            kind: 'native_transfer',
            from,
            to,
            asset: NATIVE_RESTRICTION_ASSET,
            amount,
            label: 'LICN transfer'
          });
        }
      }
      continue;
    }

    if (!bytesEqual(instruction.programId, CONTRACT_PROGRAM_BYTES)) continue;

    const caller = pubkeyToAddress(instruction.accounts[0]) || fallbackFromAddress || null;
    const contract = pubkeyToAddress(instruction.accounts[1]);
    if (!contract) continue;
    const call = decodeContractCall(instruction.data);
    if (!call) continue;
    targets.contracts.push({
      kind: 'contract_call',
      caller,
      contract,
      functionName: call?.functionName || null,
      value: call?.value || '0',
      label: call?.functionName ? `contract call ${call.functionName}` : 'contract call'
    });

    if (BigInt(call.value || '0') > 0n && caller) {
      targets.transfers.push({
        kind: 'contract_value_transfer',
        from: caller,
        to: contract,
        asset: NATIVE_RESTRICTION_ASSET,
        amount: call.value,
        label: 'contract value transfer'
      });
    }
    addContractTransferTarget(targets, contract, caller, call);
  }

  return targets;
}

function incidentWarning(incidentStatus) {
  if (!incidentStatus || typeof incidentStatus !== 'object') return null;
  const mode = String(incidentStatus.mode || incidentStatus.status || '').toLowerCase();
  const severity = String(incidentStatus.severity || '').toLowerCase();
  const normalMode = !mode || mode === 'normal' || mode === 'ok' || mode === 'none';
  const lowSeverity = !severity || severity === 'info' || severity === 'low' || severity === 'normal';
  if (normalMode && lowSeverity) return null;
  const label = [mode && `mode=${mode}`, severity && `severity=${severity}`].filter(Boolean).join(', ');
  return label ? `Network incident active (${label})` : 'Network incident status is active';
}

function contractLifecycleBlocked(status) {
  if (!status || typeof status !== 'object') return false;
  const lifecycleStatus = String(status.lifecycle_status || status.status || '').toLowerCase();
  return BLOCKED_LIFECYCLE_STATUSES.has(lifecycleStatus) || restrictionStatusIsActive(status);
}

function contractLifecycleLabel(status, contract) {
  const lifecycleStatus = String(status?.lifecycle_status || status?.status || 'restricted');
  const ids = restrictionIdsLabel(status);
  if (ids) return `Contract ${contract} is ${lifecycleStatus} by restriction ${ids}`;
  return `Contract ${contract} is ${lifecycleStatus}`;
}

function transferBlocked(status, target) {
  if (!status || typeof status !== 'object') return true;
  return status.allowed === false || status.blocked === true || restrictionStatusIsActive(status);
}

function transferBlockLabel(status, target) {
  const ids = restrictionIdsLabel(status);
  if (ids) return `${target.label} blocked by consensus restriction ${ids}`;
  return `${target.label} blocked by consensus restriction`;
}

function decimalLicnToSporesString(amountLicn) {
  const text = String(amountLicn ?? '').trim();
  if (!/^\d+(\.\d{0,9})?$/.test(text)) {
    throw new Error('Invalid LICN amount');
  }
  const [whole, fraction = ''] = text.split('.');
  const spores = BigInt(whole || '0') * 1_000_000_000n
    + BigInt((fraction + '000000000').slice(0, 9));
  if (spores <= 0n) throw new Error('Invalid LICN amount');
  return spores.toString();
}

export async function preflightNativeTransferRestrictions({
  fromAddress,
  toAddress,
  amountLicn,
  network = 'local-testnet'
}) {
  const from = String(fromAddress || '').trim();
  const to = String(toAddress || '').trim();
  const amount = decimalLicnToSporesString(amountLicn);
  const rpc = getTrustedRestrictionRpc(network);
  const targets = {
    transfers: [{
      kind: 'native_transfer',
      from,
      to,
      asset: NATIVE_RESTRICTION_ASSET,
      amount,
      label: 'LICN transfer'
    }],
    contracts: []
  };
  return runRestrictionPreflight({ rpc, targets, network, skipIfEmpty: false });
}

export async function preflightTransactionRestrictions({
  transaction,
  fromAddress = null,
  network = 'local-testnet'
}) {
  const targets = analyzeRestrictionPreflightTargets(transaction, fromAddress);
  const rpc = getTrustedRestrictionRpc(network);
  return runRestrictionPreflight({ rpc, targets, network, skipIfEmpty: true });
}

async function runRestrictionPreflight({ rpc, targets, network, skipIfEmpty }) {
  const transferTargets = Array.isArray(targets?.transfers) ? targets.transfers : [];
  const contractTargets = Array.isArray(targets?.contracts) ? targets.contracts : [];
  const hasTargets = transferTargets.length > 0 || contractTargets.length > 0;
  const warnings = [];
  const blocks = [];
  const checks = [];

  if (!hasTargets && skipIfEmpty) {
    return {
      allowed: true,
      skipped: true,
      network,
      targets,
      checks,
      warnings,
      blocks
    };
  }

  try {
    const incidentStatus = await rpc.call(RESTRICTION_METHODS.incidentStatus, []);
    const warning = incidentWarning(incidentStatus);
    if (warning) warnings.push(warning);
    checks.push({ method: RESTRICTION_METHODS.incidentStatus, status: incidentStatus });
  } catch (error) {
    warnings.push(`Incident status unavailable: ${error?.message || error}`);
  }

  for (const target of contractTargets) {
    try {
      const status = await rpc.call(RESTRICTION_METHODS.contractLifecycleStatus, [{ contract: target.contract }]);
      checks.push({ method: RESTRICTION_METHODS.contractLifecycleStatus, target, status });
      if (contractLifecycleBlocked(status)) {
        blocks.push(contractLifecycleLabel(status, target.contract));
      }
    } catch (error) {
      blocks.push(`Contract lifecycle preflight unavailable for ${target.contract}: ${error?.message || error}`);
    }
  }

  for (const target of transferTargets) {
    try {
      const status = await rpc.call(RESTRICTION_METHODS.canTransfer, [{
        from: target.from,
        to: target.to,
        asset: target.asset,
        amount: target.amount
      }]);
      checks.push({ method: RESTRICTION_METHODS.canTransfer, target, status });
      if (transferBlocked(status, target)) {
        blocks.push(transferBlockLabel(status, target));
      }
    } catch (error) {
      blocks.push(`${target.label} restriction preflight unavailable: ${error?.message || error}`);
    }
  }

  return {
    allowed: blocks.length === 0,
    skipped: false,
    network,
    targets,
    checks,
    warnings: uniqueStrings(warnings),
    blocks: uniqueStrings(blocks)
  };
}

export function restrictionPreflightSummary(preflight) {
  if (!preflight) return '';
  if (Array.isArray(preflight.blocks) && preflight.blocks.length > 0) {
    return preflight.blocks.join(' | ');
  }
  if (Array.isArray(preflight.warnings) && preflight.warnings.length > 0) {
    return preflight.warnings.join(' | ');
  }
  if (preflight.skipped) return 'No transfer or contract restriction targets detected';
  return 'Restriction preflight passed';
}

export function assertRestrictionPreflightAllowed(preflight) {
  if (!preflight || preflight.allowed !== false) return;
  throw new Error(restrictionPreflightSummary(preflight) || 'Transaction blocked by consensus restriction');
}
