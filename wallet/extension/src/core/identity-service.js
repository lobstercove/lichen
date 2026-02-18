import { MoltChainRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { base58Decode, decryptPrivateKey } from './crypto-service.js';
import { buildSignedSingleInstructionTransaction, encodeTransactionBase64 } from './tx-service.js';

const BASE_FEE_MOLT = 0.001;

function ensureWalletAndPassword(wallet, password) {
  if (!wallet) throw new Error('No active wallet');
  if (typeof password !== 'string' || password.length === 0) {
    throw new Error('Wallet password required');
  }
}

function normalizeName(input) {
  return String(input || '').trim().toLowerCase().replace(/\.molt$/, '');
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

function encodeMoltyIdArgs(callerHex, functionName, params) {
  const caller = hexToBytes(callerHex);
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
      const d = new Uint8Array(40); d.set(caller, 0); d.set(u64LE(params.molt_per_unit || 0), 32);
      return d;
    }
    case 'set_availability':
      return buildLayoutArgs([0x20, 0x01], [caller, new Uint8Array([(params.status || 0) & 0xFF])]);
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

function parseAgentType(agentType) {
  const value = Number(agentType ?? 9);
  if (!Number.isInteger(value) || value < 0 || value > 9) {
    throw new Error('Agent type must be an integer between 0 and 9');
  }
  return value;
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
  if (!value) return '';

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

function parseRateMolt(rateMolt) {
  const parsed = Number(rateMolt ?? 0);
  if (!Number.isFinite(parsed) || parsed < 0) {
    throw new Error('Rate must be a non-negative number');
  }
  if (parsed > 1_000_000) {
    throw new Error('Rate is above supported maximum');
  }
  return parsed;
}

export async function loadIdentitySnapshot(address, network) {
  if (!address) return null;

  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));

  const [profile, moltNameResult] = await Promise.all([
    rpc.call('getMoltyIdProfile', [address]).catch(() => null),
    rpc.call('reverseMoltName', [address]).catch(() => null)
  ]);
  // reverseMoltName returns {"name": "x.molt"} or null — extract string
  const moltName = moltNameResult?.name || null;

  const rep = Number(profile?.reputation?.score || profile?.identity?.reputation || 0);
  const skills = Array.isArray(profile?.skills) ? profile.skills.length : 0;

  return {
    name: moltName || profile?.identity?.name || null,
    reputation: rep,
    skills,
    active: profile?.identity?.is_active !== false && profile?.identity?.is_active !== 0,
    raw: profile
  };
}

export async function loadIdentityDetails(address, network) {
  if (!address) return null;

  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));
  const [profile, moltNameResult2] = await Promise.all([
    rpc.call('getMoltyIdProfile', [address]).catch(() => null),
    rpc.call('reverseMoltName', [address]).catch(() => null)
  ]);
  const moltName2 = moltNameResult2?.name || null;

  if (!profile) {
    return null;
  }

  return {
    name: moltName2 || profile?.identity?.name || null,
    reputation: Number(profile?.reputation?.score || profile?.identity?.reputation || 0),
    agentType: profile?.identity?.agent_type ?? null,
    active: profile?.identity?.is_active !== false && profile?.identity?.is_active !== 0,
    skills: Array.isArray(profile?.skills) ? profile.skills : [],
    achievements: Array.isArray(profile?.achievements) ? profile.achievements : [],
    vouchesReceived: Array.isArray(profile?.vouches?.received) ? profile.vouches.received : [],
    vouchesGiven: Array.isArray(profile?.vouches?.given) ? profile.vouches.given : [],
    endpoint: profile?.agent?.endpoint || '',
    availability: profile?.agent?.availability_name || 'offline',
    rate: Number(profile?.agent?.rate || 0) / 1_000_000_000,
    raw: profile
  };
}

async function getMoltyIdProgramAddress(rpc) {
  const symbols = ['YID', 'yid', 'MOLTYID'];
  for (const symbol of symbols) {
    try {
      const result = await rpc.call('getSymbolRegistry', [symbol]);
      const program = result?.program || result?.address || result?.pubkey;
      if (program) return program;
    } catch {
      // keep trying
    }
  }
  throw new Error('MoltyID contract not found on network');
}

async function sendIdentityContractCall({ wallet, password, network, functionName, args, valueMolt = 0 }) {
  ensureWalletAndPassword(wallet, password);

  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));
  const moltyidAddr = await getMoltyIdProgramAddress(rpc);
  const latestBlock = await rpc.getLatestBlock();

  try {
    const balanceResult = await rpc.getBalance(wallet.address);
    const spendable = Number(balanceResult?.spendable ?? balanceResult?.balance ?? 0) / 1_000_000_000;
    const required = Number(valueMolt || 0) + BASE_FEE_MOLT;
    if (Number.isFinite(spendable) && spendable < required) {
      throw new Error(`Insufficient MOLT: need ${required.toFixed(6)}, have ${spendable.toFixed(6)} spendable`);
    }
  } catch (error) {
    if (String(error?.message || '').includes('Insufficient MOLT')) {
      throw error;
    }
  }

  const contractProgramId = new Uint8Array(32).fill(0xff);
  const moltyIdPubkey = base58Decode(moltyidAddr);

  // Encode args as proper binary with WASM ABI layout descriptor
  const argsBytes = encodeMoltyIdArgs(wallet.publicKey, functionName, args);

  const callPayload = JSON.stringify({
    Call: {
      function: functionName,
      args: Array.from(argsBytes),
      value: Math.floor(valueMolt * 1_000_000_000)
    }
  });

  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);

  const transaction = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromPublicKeyHex: wallet.publicKey,
    blockhash: latestBlock.hash,
    programIdBytes: contractProgramId,
    accountPubkeys: [moltyIdPubkey],
    instructionDataBytes: new TextEncoder().encode(callPayload)
  });

  const txBase64 = encodeTransactionBase64(transaction);
  const txHash = await rpc.sendTransaction(txBase64);
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

  const prof = Number(proficiency ?? 50);
  if (!Number.isFinite(prof) || prof < 1 || prof > 100) {
    throw new Error('Proficiency must be between 1 and 100');
  }

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'add_skill',
    args: {
      name,
      proficiency: Math.round(prof)
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

export async function setIdentityRate({ wallet, password, network, rateMolt }) {
  const rateShells = Math.floor(parseRateMolt(rateMolt) * 1_000_000_000);

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'set_rate',
    args: {
      molt_per_unit: rateShells
    }
  });
}

function getNameCostPerYear(nameLength) {
  if (nameLength <= 3) return 500;
  if (nameLength === 4) return 100;
  return 20;
}

export async function registerMoltName({ wallet, password, network, name, durationYears }) {
  const normalized = normalizeName(name);
  validateNameFormat(normalized);
  if (normalized.length <= 4) {
    throw new Error('3-4 char names are auction-only');
  }

  const years = Math.max(1, Math.min(10, Number(durationYears || 1)));
  const valueMolt = getNameCostPerYear(normalized.length) * years;

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'register_name',
    args: {
      name: normalized,
      duration_years: years
    },
    valueMolt
  });
}

export async function renewMoltName({ wallet, password, network, name, additionalYears }) {
  const normalized = normalizeName(name);
  validateNameFormat(normalized);

  const years = Math.max(1, Math.min(10, Number(additionalYears || 1)));
  const valueMolt = getNameCostPerYear(normalized.length) * years;

  return sendIdentityContractCall({
    wallet,
    password,
    network,
    functionName: 'renew_name',
    args: {
      name: normalized,
      additional_years: years
    },
    valueMolt
  });
}

export async function transferMoltName({ wallet, password, network, name, recipient }) {
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

export async function releaseMoltName({ wallet, password, network, name }) {
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
