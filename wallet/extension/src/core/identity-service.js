import { LichenRPC, getConfiguredRpcEndpoint, getTrustedRpcEndpoint } from './rpc-service.js';
import { base58Decode, decryptPrivateKey } from './crypto-service.js';
import { buildSignedSingleInstructionTransaction, encodeTransactionBase64 } from './tx-service.js';
import { baseUnitsToDecimalString, parseDecimalBaseUnits } from './amount-service.js';

const BASE_FEE_SPORES = 1_000_000n;

function ensureWalletAndPassword(wallet, password) {
  if (!wallet) throw new Error('No active wallet');
  if (typeof password !== 'string' || password.length === 0) {
    throw new Error('Wallet password required');
  }
}

function normalizeName(input) {
  return String(input || '').trim().toLowerCase().replace(/\.licn$/, '');
}

// ── Binary Arg Encoding (WASM ABI layout descriptor) ──

function hexToBytes(hex) {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
  return bytes;
}

function buildLayoutArgs(layout, chunks) {
  const hdr = new Uint8Array(1 + layout.length);
  hdr[0] = 0xAB;
  for (let i = 0; i < layout.length; i++) hdr[1 + i] = layout[i];
  let total = 0;
  for (const c of chunks) total += c.length;
  const out = new Uint8Array(hdr.length + total);
  out.set(hdr, 0);
  let off = hdr.length;
  for (const c of chunks) { out.set(c, off); off += c.length; }
  return out;
}

function padBytes(data, len) {
  if (data.length >= len) return data.subarray ? data.subarray(0, len) : data.slice(0, len);
  const r = new Uint8Array(len);
  r.set(data, 0);
  return r;
}

function u32LE(v) {
  return new Uint8Array([v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF, (v >> 24) & 0xFF]);
}

function u64LE(v) {
  const b = new Uint8Array(8);
  const big = BigInt(v);
  for (let i = 0; i < 8; i++) b[i] = Number((big >> BigInt(i * 8)) & 0xFFn);
  return b;
}

function encodeLichenIdArgs(callerAddress, functionName, params) {
  const caller = base58Decode(callerAddress);
  const te = new TextEncoder();
  switch (functionName) {
    case 'register_identity': {
      const nm = te.encode(params.name || '');
      return buildLayoutArgs([0x20, 0x01, 0x40, 0x04], [caller, new Uint8Array([params.agent_type & 0xFF]), padBytes(nm, 64), u32LE(nm.length)]);
    }
    case 'update_agent_type':
      return buildLayoutArgs([0x20, 0x01], [caller, new Uint8Array([params.agent_type & 0xFF])]);
    case 'register_name': {
      const nm = te.encode(params.name || '');
      return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [caller, padBytes(nm, 32), u32LE(nm.length), new Uint8Array([(params.duration_years || 1) & 0xFF])]);
    }
    case 'renew_name': {
      const nm = te.encode(params.name || '');
      return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [caller, padBytes(nm, 32), u32LE(nm.length), new Uint8Array([(params.additional_years || 1) & 0xFF])]);
    }
    case 'transfer_name': {
      const nm = te.encode(params.name || '');
      return buildLayoutArgs([0x20, 0x20, 0x04, 0x20], [caller, padBytes(nm, 32), u32LE(nm.length), base58Decode(params.new_owner)]);
    }
    case 'release_name': {
      const nm = te.encode(params.name || '');
      return buildLayoutArgs([0x20, 0x20, 0x04], [caller, padBytes(nm, 32), u32LE(nm.length)]);
    }
    case 'add_skill': {
      const nm = te.encode(params.name || '');
      return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [caller, padBytes(nm, 32), u32LE(nm.length), new Uint8Array([(params.proficiency || 50) & 0xFF])]);
    }
    case 'vouch': {
      return buildLayoutArgs([0x20, 0x20], [caller, base58Decode(params.vouchee)]);
    }
    case 'set_endpoint': {
      const url = te.encode(params.url || '');
      const stride = Math.max(32, Math.min(255, url.length));
      return buildLayoutArgs([0x20, stride, 0x04], [caller, padBytes(url, stride), u32LE(url.length)]);
    }
    case 'set_rate': {
      const d = new Uint8Array(40); d.set(caller, 0); d.set(u64LE(params.licn_per_unit || 0), 32);
      return d;
    }
    case 'set_availability':
      return buildLayoutArgs([0x20, 0x01], [caller, new Uint8Array([(params.status || 0) & 0xFF])]);
    case 'attest_skill': {
      const identity = base58Decode(params.identity);
      const sn = te.encode(params.skill_name || '');
      return buildLayoutArgs([0x20, 0x20, 0x20, 0x04, 0x01], [caller, identity, padBytes(sn, 32), u32LE(sn.length), new Uint8Array([(params.level || 50) & 0xFF])]);
    }
    case 'revoke_attestation': {
      const identity = base58Decode(params.identity);
      const sn = te.encode(params.skill_name || '');
      return buildLayoutArgs([0x20, 0x20, 0x20, 0x04], [caller, identity, padBytes(sn, 32), u32LE(sn.length)]);
    }
    default:
      return new TextEncoder().encode(JSON.stringify(params));
  }
}

function validateNameFormat(normalized) {
  if (!normalized) throw new Error('Name required');
  if (normalized.length < 3 || normalized.length > 32 || !/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(normalized)) {
    throw new Error('Invalid name format');
  }
}

function parseIntegerRange(value, label, min, max, fallback = null) {
  const text = String(value ?? '').trim();
  if (!text && fallback !== null) return fallback;
  if (!/^\d+$/.test(text)) {
    throw new Error(`${label} must be an integer between ${min} and ${max}`);
  }
  const parsed = Number(text);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) {
    throw new Error(`${label} must be an integer between ${min} and ${max}`);
  }
  return parsed;
}

function parseAgentType(agentType) {
  return parseIntegerRange(agentType, 'Agent type', 0, 10, 9);
}

function isAddressLike(address) {
  try {
    return base58Decode(String(address || '').trim()).length === 32;
  } catch {
    return false;
  }
}

function validateEndpoint(endpoint) {
  const value = String(endpoint || '').trim();
  if (!value) {
    throw new Error('Endpoint cannot be cleared by the current LichenID contract');
  }

  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error('Endpoint must be a valid http(s) URL');
  }

  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error('Endpoint must use http(s)');
  }

  if (value.length > 256) {
    throw new Error('Endpoint URL must be 256 characters or less');
  }

  return value;
}

function parseRateLicn(rateLicn) {
  const spores = parseDecimalBaseUnits(rateLicn ?? '0', 9, 'Rate');
  if (spores > 1_000_000n * 1_000_000_000n) {
    throw new Error('Rate is above supported maximum');
  }
  return spores;
}

function baseUnitBigInt(value) {
  if (typeof value === 'bigint') return value >= 0n ? value : 0n;
  if (typeof value === 'number') return Number.isSafeInteger(value) && value >= 0 ? BigInt(value) : 0n;
  const text = String(value ?? '0').trim();
  return /^\d+$/.test(text) ? BigInt(text) : 0n;
}

function spendableBaseUnits(result) {
  if (!result || typeof result !== 'object') return 0n;
  if (Object.prototype.hasOwnProperty.call(result, 'spendable')) {
    return baseUnitBigInt(result.spendable);
  }
  if (Object.prototype.hasOwnProperty.call(result, 'available')) {
    return baseUnitBigInt(result.available);
  }
  return 0n;
}

function valueLicnToSpores(valueLicn) {
  if (typeof valueLicn === 'bigint') {
    if (valueLicn < 0n) throw new Error('Transaction value must be non-negative');
    return valueLicn;
  }
  return parseDecimalBaseUnits(valueLicn ?? '0', 9, 'Transaction value');
}

function sporesToJsonNumber(spores, label = 'Transaction value') {
  const value = baseUnitBigInt(spores);
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new Error(`${label} is above the supported browser-safe transaction value`);
  }
  return Number(value);
}

export async function loadIdentitySnapshot(address, network) {
  if (!address) return null;

  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));

  const [profile, lichenNameResult] = await Promise.all([
    rpc.call('getLichenIdProfile', [address]).catch(() => null),
    rpc.call('reverseLichenName', [address]).catch(() => null)
  ]);
  // reverseLichenName returns {"name": "x.lichen"} or null — extract string
  const lichenName = lichenNameResult?.name || null;

  const rep = Number(profile?.reputation?.score ?? profile?.identity?.reputation ?? 0);
  const skills = Array.isArray(profile?.skills) ? profile.skills.length : 0;
  const identityName = profile?.identity?.name || null;

  return {
    name: identityName,
    lichenName: lichenName,
    reputation: rep,
    skills,
    active: profile?.identity?.is_active !== false && profile?.identity?.is_active !== 0,
    raw: profile
  };
}

export async function loadIdentityDetails(address, network) {
  if (!address) return null;

  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  const [profile, lichenNameResult2] = await Promise.all([
    rpc.call('getLichenIdProfile', [address]).catch(() => null),
    rpc.call('reverseLichenName', [address]).catch(() => null)
  ]);
  const lichenName2 = lichenNameResult2?.name || null;

  if (!profile) {
    return null;
  }

  const identityName = profile?.identity?.name || null;

  return {
    name: identityName,
    lichenName: lichenName2,
    reputation: Number(profile?.reputation?.score ?? profile?.identity?.reputation ?? 0),
    agentType: profile?.identity?.agent_type ?? null,
    active: profile?.identity?.is_active !== false && profile?.identity?.is_active !== 0,
    skills: Array.isArray(profile?.skills) ? profile.skills : [],
    achievements: Array.isArray(profile?.achievements) ? profile.achievements : [],
    vouchesReceived: Array.isArray(profile?.vouches?.received) ? profile.vouches.received : [],
    vouchesGiven: Array.isArray(profile?.vouches?.given) ? profile.vouches.given : [],
    endpoint: profile?.agent?.endpoint || '',
    availability: profile?.agent?.availability_name || 'offline',
    rate: Number(profile?.agent?.rate ?? 0) / 1_000_000_000,
    raw: profile
  };
}

async function getLichenIdProgramAddress(network) {
  const trustedRpc = new LichenRPC(getTrustedRpcEndpoint(network));
  const symbols = ['YID', 'yid', 'LICHENID'];
  for (const symbol of symbols) {
    try {
      const result = await trustedRpc.call('getSymbolRegistry', [symbol]);
      const program = result?.program || result?.address || result?.pubkey;
      if (program) return program;
    } catch {
      // keep trying
    }
  }

  try {
    const contracts = [];
    let cursor = null;
    do {
      const params = [{ limit: 1000 }];
      if (cursor) params[0].cursor = cursor;
      const result = await trustedRpc.call('getAllContracts', params);
      contracts.push(...(Array.isArray(result?.contracts) ? result.contracts : []));
      cursor = result?.has_more ? result?.next_cursor : null;
    } while (cursor);
    const contract = contracts.find((entry) => entry.name === 'lichenid' || entry.symbol === 'YID');
    if (contract) return contract.program_id || contract.address;
  } catch {
    // trusted metadata lookup unavailable
  }

  throw new Error('LichenID contract not found on network');
}

async function sendIdentityContractCall({ wallet, password, network, functionName, args, valueLicn = 0 }) {
  ensureWalletAndPassword(wallet, password);
  const valueSpores = valueLicnToSpores(valueLicn);
  const valueJson = sporesToJsonNumber(valueSpores);

  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  const lichenidAddr = await getLichenIdProgramAddress(network);
  const blockhash = await rpc.getRecentBlockhash();

  try {
    const balanceResult = await rpc.getBalance(wallet.address);
    const spendable = spendableBaseUnits(balanceResult);
    const required = valueSpores + BASE_FEE_SPORES;
    if (spendable < required) {
      throw new Error(`Insufficient LICN: need ${baseUnitsToDecimalString(required, 9)}, have ${baseUnitsToDecimalString(spendable, 9)} spendable`);
    }
  } catch (error) {
    if (String(error?.message || '').includes('Insufficient LICN')) {
      throw error;
    }
  }

  const contractProgramId = new Uint8Array(32).fill(0xff);
  const lichenIdPubkey = base58Decode(lichenidAddr);

  // Encode args as proper binary with WASM ABI layout descriptor
  const argsBytes = encodeLichenIdArgs(wallet.address, functionName, args);

  const callPayload = JSON.stringify({
    Call: {
      function: functionName,
      args: Array.from(argsBytes),
      value: valueJson
    }
  });

  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash,
    programIdBytes: contractProgramId,
    accountPubkeys: [lichenIdPubkey],
    instructionDataBytes: new TextEncoder().encode(callPayload)
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransactionWithPreflight(txBase64);
  return { txHash };
}

export async function registerIdentity({ wallet, password, network, displayName, agentType }) {
  const name = String(displayName || '').trim();
  if (!name || name.length > 64) throw new Error('Display name required (1-64 chars)');

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'register_identity',
    args: {
      agent_type: parseAgentType(agentType),
      name
    }
  });
}

export async function addIdentitySkill({ wallet, password, network, skillName, proficiency }) {
  const name = String(skillName || '').trim();
  if (!name) throw new Error('Skill name required');
  if (name.length > 64) throw new Error('Skill name must be 64 characters or less');

  const prof = parseIntegerRange(proficiency, 'Proficiency', 1, 100, 50);

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'add_skill',
    args: {
      name,
      proficiency: prof
    }
  });
}

export async function updateIdentityAgentType({ wallet, password, network, agentType }) {
  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'update_agent_type',
    args: {
      agent_type: parseAgentType(agentType)
    }
  });
}

export async function vouchForIdentity({ wallet, password, network, vouchee }) {
  const voucheeAddress = String(vouchee || '').trim();
  if (!voucheeAddress) throw new Error('Vouchee address required');
  if (!isAddressLike(voucheeAddress)) throw new Error('Invalid vouchee address');

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'vouch',
    args: {
      vouchee: voucheeAddress
    }
  });
}

export async function setIdentityEndpoint({ wallet, password, network, endpoint }) {
  const validatedEndpoint = validateEndpoint(endpoint);

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'set_endpoint',
    args: {
      url: validatedEndpoint
    }
  });
}

export async function setIdentityAvailability({ wallet, password, network, online }) {
  if (typeof online !== 'boolean') {
    throw new Error('Availability must be online or offline');
  }

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'set_availability',
    args: {
      status: online ? 1 : 0
    }
  });
}

export async function setIdentityRate({ wallet, password, network, rateLicn }) {
  const rateSpores = parseRateLicn(rateLicn);

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'set_rate',
    args: {
      licn_per_unit: rateSpores.toString()
    }
  });
}

function getNameCostPerYear(nameLength) {
  if (nameLength <= 3) return 500;
  if (nameLength === 4) return 100;
  return 20;
}

export async function registerLichenName({ wallet, password, network, name, durationYears }) {
  const normalized = normalizeName(name);
  validateNameFormat(normalized);
  if (normalized.length <= 4) {
    throw new Error('3-4 char names are auction-only');
  }

  const years = parseIntegerRange(durationYears, 'Duration years', 1, 10, 1);
  const valueLicn = getNameCostPerYear(normalized.length) * years;

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'register_name',
    args: {
      name: normalized,
      duration_years: years
    },
    valueLicn
  });
}

export async function renewLichenName({ wallet, password, network, name, additionalYears }) {
  const normalized = normalizeName(name);
  validateNameFormat(normalized);

  const years = parseIntegerRange(additionalYears, 'Additional years', 1, 10, 1);
  const valueLicn = getNameCostPerYear(normalized.length) * years;

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'renew_name',
    args: {
      name: normalized,
      additional_years: years
    },
    valueLicn
  });
}

export async function transferLichenName({ wallet, password, network, name, recipient }) {
  const normalized = normalizeName(name);
  validateNameFormat(normalized);

  const recipientAddress = String(recipient || '').trim();
  if (!recipientAddress) throw new Error('Recipient required');
  if (!isAddressLike(recipientAddress)) throw new Error('Invalid recipient address');

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'transfer_name',
    args: {
      name: normalized,
      new_owner: recipientAddress
    }
  });
}

export async function releaseLichenName({ wallet, password, network, name }) {
  const normalized = normalizeName(name);
  validateNameFormat(normalized);

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'release_name',
    args: {
      name: normalized
    }
  });
}
