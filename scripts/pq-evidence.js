'use strict';

const http = require('http');
const https = require('https');
const { createHash } = require('crypto');
const { URL } = require('url');

const EVIDENCE_SCHEMA_VERSION = 1;
const EVIDENCE_MESSAGE_PREFIX = 'Lichen:PQEvidence:v1:';
const DEFAULT_EVIDENCE_PURPOSE = 'neo-x-pq-watchtower';
const DEFAULT_ROUTE_EVIDENCE_TTL_SLOTS = 150;
const DEFAULT_NEOX_TESTNET_CHAIN_ID = '12227332';
const PQ_SCHEME_ML_DSA_65 = 0x01;
const SUPPORTED_EVIDENCE_KINDS = new Set([
    'route_health',
    'reserve_status',
    'deployment_manifest',
    'incident_hash',
    'custody_policy',
]);

const BS58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function isPlainObject(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
        return false;
    }
    const prototype = Object.getPrototypeOf(value);
    return prototype === Object.prototype || prototype === null;
}

function canonicalize(value, path = '$') {
    if (value === null || typeof value === 'string' || typeof value === 'boolean') {
        return value;
    }
    if (typeof value === 'number') {
        if (!Number.isFinite(value)) {
            throw new Error(`Canonical JSON does not allow non-finite number at ${path}`);
        }
        return value;
    }
    if (typeof value === 'bigint') {
        return value.toString();
    }
    if (Array.isArray(value)) {
        return value.map((entry, index) => canonicalize(entry, `${path}[${index}]`));
    }
    if (isPlainObject(value)) {
        const out = {};
        for (const key of Object.keys(value).sort()) {
            const child = value[key];
            if (child === undefined) {
                throw new Error(`Canonical JSON does not allow undefined at ${path}.${key}`);
            }
            if (typeof child === 'function' || typeof child === 'symbol') {
                throw new Error(`Canonical JSON does not allow ${typeof child} at ${path}.${key}`);
            }
            out[key] = canonicalize(child, `${path}.${key}`);
        }
        return out;
    }
    throw new Error(`Canonical JSON does not allow ${typeof value} at ${path}`);
}

function stableStringify(value) {
    return JSON.stringify(canonicalize(value));
}

function canonicalBytes(value) {
    return Buffer.from(stableStringify(value), 'utf8');
}

function sha256Hex(input) {
    return createHash('sha256').update(input).digest('hex');
}

function hashCanonical(value) {
    return sha256Hex(canonicalBytes(value));
}

function normalizeNonEmptyString(value, field) {
    if (value === undefined || value === null) {
        throw new Error(`Evidence ${field} is required`);
    }
    const normalized = String(value).trim();
    if (!normalized) {
        throw new Error(`Evidence ${field} is required`);
    }
    return normalized;
}

function normalizeLowerString(value, field) {
    return normalizeNonEmptyString(value, field).toLowerCase();
}

function normalizeChainId(value) {
    const normalized = normalizeNonEmptyString(value, 'domain.neo_chain_id');
    if (!/^[0-9]+$/.test(normalized)) {
        throw new Error(`Evidence domain.neo_chain_id must be a decimal string, got ${normalized}`);
    }
    return normalized;
}

function normalizeSlot(value, field) {
    const slot = typeof value === 'string' && value.trim() !== '' ? Number(value) : value;
    if (!Number.isSafeInteger(slot) || slot < 0) {
        throw new Error(`Evidence ${field} must be a non-negative safe integer`);
    }
    return slot;
}

function normalizePositiveInteger(value, field) {
    const parsed = typeof value === 'string' && value.trim() !== '' ? Number(value) : value;
    if (!Number.isSafeInteger(parsed) || parsed <= 0) {
        throw new Error(`Evidence ${field} must be a positive safe integer`);
    }
    return parsed;
}

function normalizeManifestHash(value) {
    const normalized = normalizeNonEmptyString(value, 'manifest_hash').replace(/^0x/i, '').toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(normalized)) {
        throw new Error('Evidence manifest_hash must be a 32-byte hex hash');
    }
    return normalized;
}

function normalizeEvidenceKind(kind) {
    const normalized = normalizeLowerString(kind, 'kind');
    if (!SUPPORTED_EVIDENCE_KINDS.has(normalized)) {
        throw new Error(`Unsupported evidence kind: ${normalized}`);
    }
    return normalized;
}

function normalizeEvidenceDomain(domain) {
    if (!domain || typeof domain !== 'object') {
        throw new Error('Evidence domain is required');
    }
    return {
        lichen_network: normalizeLowerString(
            domain.lichen_network !== undefined ? domain.lichen_network : domain.lichenNetwork,
            'domain.lichen_network',
        ),
        neo_network: normalizeLowerString(
            domain.neo_network !== undefined ? domain.neo_network : domain.neoNetwork,
            'domain.neo_network',
        ),
        neo_chain_id: normalizeChainId(
            domain.neo_chain_id !== undefined ? domain.neo_chain_id : domain.neoChainId,
        ),
        route: normalizeLowerString(domain.route, 'domain.route'),
        asset: normalizeLowerString(domain.asset, 'domain.asset'),
        purpose: normalizeLowerString(
            domain.purpose !== undefined ? domain.purpose : DEFAULT_EVIDENCE_PURPOSE,
            'domain.purpose',
        ),
    };
}

function evidenceIdBody(envelope) {
    return {
        schema_version: envelope.schema_version,
        kind: envelope.kind,
        domain: envelope.domain,
        slot: envelope.slot,
        issued_at_ms: envelope.issued_at_ms,
        expires_at_slot: envelope.expires_at_slot,
        manifest_hash: envelope.manifest_hash,
        payload_hash: envelope.payload_hash,
        required_signatures: envelope.required_signatures,
    };
}

function computeEvidenceId(envelope) {
    return hashCanonical(evidenceIdBody(envelope));
}

function evidenceSigningBody(envelope) {
    return {
        ...evidenceIdBody(envelope),
        evidence_id: envelope.evidence_id,
    };
}

function evidenceSigningBytes(envelope) {
    return Buffer.from(`${EVIDENCE_MESSAGE_PREFIX}${stableStringify(evidenceSigningBody(envelope))}`, 'utf8');
}

function createUnsignedEvidence({
    kind,
    domain,
    payload,
    slot,
    issuedAtMs,
    issued_at_ms,
    expiresAtSlot,
    expires_at_slot,
    manifestHash,
    manifest_hash,
    requiredSignatures,
    required_signatures,
}) {
    const normalized = {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        kind: normalizeEvidenceKind(kind),
        domain: normalizeEvidenceDomain(domain),
        slot: normalizeSlot(slot, 'slot'),
        issued_at_ms: normalizeSlot(
            issuedAtMs !== undefined ? issuedAtMs : issued_at_ms,
            'issued_at_ms',
        ),
        expires_at_slot: normalizeSlot(
            expiresAtSlot !== undefined ? expiresAtSlot : expires_at_slot,
            'expires_at_slot',
        ),
        manifest_hash: normalizeManifestHash(
            manifestHash !== undefined ? manifestHash : manifest_hash,
        ),
        payload_hash: hashCanonical(payload),
        required_signatures: normalizePositiveInteger(
            requiredSignatures !== undefined ? requiredSignatures : required_signatures,
            'required_signatures',
        ),
        payload: canonicalize(payload),
        signatures: [],
    };
    if (normalized.expires_at_slot <= normalized.slot) {
        throw new Error('Evidence expires_at_slot must be greater than slot');
    }
    normalized.evidence_id = computeEvidenceId(normalized);
    return normalized;
}

function hexToBytes(hex) {
    const clean = normalizeNonEmptyString(hex, 'hex').replace(/^0x/i, '');
    if (clean.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(clean)) {
        throw new Error('Invalid hex bytes');
    }
    const out = new Uint8Array(clean.length / 2);
    for (let i = 0; i < out.length; i += 1) {
        out[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
    }
    return out;
}

function bs58encode(bytes) {
    let leadingZeros = 0;
    for (let i = 0; i < bytes.length && bytes[i] === 0; i += 1) {
        leadingZeros += 1;
    }

    let value = 0n;
    for (const byte of bytes) {
        value = value * 256n + BigInt(byte);
    }

    let encoded = '';
    while (value > 0n) {
        encoded = BS58[Number(value % 58n)] + encoded;
        value /= 58n;
    }
    return '1'.repeat(leadingZeros) + encoded;
}

function publicKeyToAddressBytes(publicKey) {
    const bytes = publicKey instanceof Uint8Array ? publicKey : new Uint8Array(publicKey);
    const hash = createHash('sha256').update(bytes).digest();
    const address = Buffer.alloc(32);
    address[0] = PQ_SCHEME_ML_DSA_65;
    hash.copy(address, 1, 0, 31);
    return new Uint8Array(address);
}

function signerAddressFromPqSignature(pqSignature) {
    if (!pqSignature || typeof pqSignature !== 'object') {
        throw new Error('PQ signature object is required');
    }
    const publicKey = pqSignature.public_key;
    if (!publicKey || typeof publicKey !== 'object' || typeof publicKey.bytes !== 'string') {
        throw new Error('PQ signature is missing public_key.bytes');
    }
    const publicKeyBytes = hexToBytes(publicKey.bytes);
    return bs58encode(publicKeyToAddressBytes(publicKeyBytes));
}

function normalizeSignatureEntry(entry) {
    if (!entry || typeof entry !== 'object') {
        throw new Error('Evidence signature entry must be an object');
    }
    const pqSignature = entry.signature || entry.pq_signature || entry.pqSignature;
    if (!pqSignature || typeof pqSignature !== 'object') {
        throw new Error('Evidence signature entry is missing signature');
    }
    const signer = normalizeNonEmptyString(
        entry.signer || signerAddressFromPqSignature(pqSignature),
        'signature.signer',
    );
    return { signer, signature: pqSignature };
}

function signEvidenceEnvelope(unsignedEvidence, signerKeypairs, options = {}) {
    if (!Array.isArray(signerKeypairs) || signerKeypairs.length === 0) {
        throw new Error('At least one PQ signer keypair is required');
    }
    if (typeof options.signMessage !== 'function') {
        throw new Error('signEvidenceEnvelope requires options.signMessage');
    }

    const envelope = verifyUnsignedShape(unsignedEvidence);
    if (signerKeypairs.length < envelope.required_signatures) {
        throw new Error('Signer count is below evidence required_signatures');
    }

    const message = evidenceSigningBytes(envelope);
    const signatures = signerKeypairs.map((keypair) => {
        const pqSignature = options.signMessage(new Uint8Array(message), keypair);
        const signer = signerAddressFromPqSignature(pqSignature);
        if (keypair.address && keypair.address !== signer) {
            throw new Error(`PQ signer address mismatch: expected ${keypair.address}, got ${signer}`);
        }
        return { signer, signature: pqSignature };
    });

    return {
        ...envelope,
        signatures,
    };
}

function verifyUnsignedShape(evidence) {
    if (!evidence || typeof evidence !== 'object') {
        throw new Error('Evidence envelope is required');
    }
    const envelope = {
        schema_version: normalizePositiveInteger(evidence.schema_version, 'schema_version'),
        kind: normalizeEvidenceKind(evidence.kind),
        domain: normalizeEvidenceDomain(evidence.domain),
        slot: normalizeSlot(evidence.slot, 'slot'),
        issued_at_ms: normalizeSlot(evidence.issued_at_ms, 'issued_at_ms'),
        expires_at_slot: normalizeSlot(evidence.expires_at_slot, 'expires_at_slot'),
        manifest_hash: normalizeManifestHash(evidence.manifest_hash),
        payload_hash: normalizeManifestHash(evidence.payload_hash),
        required_signatures: normalizePositiveInteger(
            evidence.required_signatures,
            'required_signatures',
        ),
        payload: canonicalize(evidence.payload),
        signatures: Array.isArray(evidence.signatures) ? evidence.signatures : [],
    };
    if (envelope.schema_version !== EVIDENCE_SCHEMA_VERSION) {
        throw new Error(`Unsupported evidence schema_version: ${envelope.schema_version}`);
    }
    if (envelope.expires_at_slot <= envelope.slot) {
        throw new Error('Evidence expires_at_slot must be greater than slot');
    }
    envelope.evidence_id = normalizeManifestHash(evidence.evidence_id);
    return envelope;
}

function assertExpectedDomain(actual, expected) {
    if (!expected) {
        return;
    }
    const normalizedExpected = normalizeEvidenceDomain(expected);
    if (stableStringify(actual) !== stableStringify(normalizedExpected)) {
        throw new Error('Evidence domain does not match expected domain');
    }
}

function verifyEvidenceEnvelope(evidence, options = {}) {
    const envelope = verifyUnsignedShape(evidence);
    const expectedPayloadHash = hashCanonical(envelope.payload);
    if (expectedPayloadHash !== envelope.payload_hash) {
        throw new Error('Evidence payload_hash mismatch');
    }
    const expectedEvidenceId = computeEvidenceId(envelope);
    if (expectedEvidenceId !== envelope.evidence_id) {
        throw new Error('Evidence evidence_id mismatch');
    }
    if (options.expectedKind && normalizeEvidenceKind(options.expectedKind) !== envelope.kind) {
        throw new Error('Evidence kind does not match expected kind');
    }
    assertExpectedDomain(envelope.domain, options.expectedDomain);
    if (options.currentSlot !== undefined && normalizeSlot(options.currentSlot, 'currentSlot') > envelope.expires_at_slot) {
        throw new Error('Evidence is stale for the supplied current slot');
    }
    if (options.seenEvidenceIds && options.seenEvidenceIds.has(envelope.evidence_id)) {
        throw new Error('Evidence ID has already been consumed');
    }
    if (typeof options.verifySignature !== 'function') {
        throw new Error('verifyEvidenceEnvelope requires options.verifySignature');
    }

    const policyThreshold = options.requiredThreshold !== undefined
        ? normalizePositiveInteger(options.requiredThreshold, 'requiredThreshold')
        : envelope.required_signatures;
    const requiredThreshold = Math.max(policyThreshold, envelope.required_signatures);
    const trustedSigners = options.trustedSigners
        ? new Set(options.trustedSigners.map((signer) => normalizeNonEmptyString(signer, 'trustedSigners[]')))
        : null;
    if (trustedSigners && trustedSigners.size < requiredThreshold) {
        throw new Error('Trusted signer set is smaller than required threshold');
    }

    const message = evidenceSigningBytes(envelope);
    const validSigners = [];
    const seenSigners = new Set();
    for (const rawEntry of envelope.signatures) {
        const entry = normalizeSignatureEntry(rawEntry);
        const derivedSigner = signerAddressFromPqSignature(entry.signature);
        if (entry.signer !== derivedSigner) {
            throw new Error('Evidence signature signer does not match public key');
        }
        if (seenSigners.has(entry.signer)) {
            throw new Error('Evidence contains duplicate signer');
        }
        seenSigners.add(entry.signer);
        if (trustedSigners && !trustedSigners.has(entry.signer)) {
            throw new Error(`Evidence signer ${entry.signer} is not trusted`);
        }
        const publicKeyBytes = hexToBytes(entry.signature.public_key.bytes);
        if (!options.verifySignature(new Uint8Array(message), entry.signature, publicKeyBytes)) {
            throw new Error(`Evidence signature mismatch for signer ${entry.signer}`);
        }
        validSigners.push(entry.signer);
    }

    if (validSigners.length < requiredThreshold) {
        throw new Error(`Evidence quorum not met: ${validSigners.length}/${requiredThreshold}`);
    }
    if (options.seenEvidenceIds && options.recordSeenEvidence) {
        options.seenEvidenceIds.add(envelope.evidence_id);
    }

    return {
        ok: true,
        evidenceId: envelope.evidence_id,
        kind: envelope.kind,
        domain: envelope.domain,
        payloadHash: envelope.payload_hash,
        validSigners,
        requiredThreshold,
    };
}

function parseBigIntValue(value, fallback = 0n) {
    if (value === undefined || value === null || value === '') {
        return fallback;
    }
    try {
        return BigInt(String(value));
    } catch {
        return fallback;
    }
}

function routeStatusIsPaused(status) {
    if (!status || typeof status !== 'object') {
        return false;
    }
    if (status.paused || status.route_paused) {
        return true;
    }
    return Array.isArray(status.active_restriction_ids) && status.active_restriction_ids.length > 0;
}

function classifyRouteEvidenceAlerts(target, snapshot) {
    const alerts = [];
    const routeStatus = snapshot && snapshot.routeStatus;
    const stats = snapshot && snapshot.stats;
    if (routeStatusIsPaused(routeStatus)) {
        alerts.push({
            rule_id: 'bridge-route-paused',
            severity: 'critical',
            title: 'Bridge route paused',
            event: {
                event: 'BridgeRouteHealth',
                label: target.label,
                chain: target.chain,
                asset: target.asset,
                route_paused: true,
                active_restriction_ids: routeStatus.active_restriction_ids || [],
            },
        });
    }
    if (stats && typeof stats === 'object') {
        const supply = parseBigIntValue(stats.total_supply !== undefined ? stats.total_supply : stats.supply, 0n);
        const reserve = parseBigIntValue(stats.reserve_attested, 0n);
        const attestationCount = parseBigIntValue(stats.attestation_count, 0n);
        if (stats.paused) {
            alerts.push({
                rule_id: 'wrapped-token-paused',
                severity: 'high',
                title: 'Wrapped token paused',
                event: {
                    event: 'WrappedTokenHealth',
                    label: target.label,
                    symbol: target.symbol || '',
                    chain: target.chain,
                    asset: target.asset,
                    paused: true,
                },
            });
        }
        if (reserve < supply) {
            alerts.push({
                rule_id: 'wrapped-reserve-deficit',
                severity: 'critical',
                title: 'Wrapped token reserve deficit',
                event: {
                    event: 'WrappedTokenHealth',
                    label: target.label,
                    symbol: target.symbol || '',
                    chain: target.chain,
                    asset: target.asset,
                    supply: supply.toString(),
                    reserve_attested: reserve.toString(),
                    deficit: (supply - reserve).toString(),
                },
            });
        } else if (supply > 0n && attestationCount === 0n) {
            alerts.push({
                rule_id: 'wrapped-reserve-unattested',
                severity: 'high',
                title: 'Wrapped token reserve not attested',
                event: {
                    event: 'WrappedTokenHealth',
                    label: target.label,
                    symbol: target.symbol || '',
                    chain: target.chain,
                    asset: target.asset,
                    supply: supply.toString(),
                    attestation_count: attestationCount.toString(),
                },
            });
        }
    }
    return alerts;
}

function routeHealthState(alerts) {
    if (alerts.some((alert) => alert.severity === 'critical')) {
        return 'critical';
    }
    if (alerts.some((alert) => alert.severity === 'high')) {
        return 'high';
    }
    if (alerts.some((alert) => alert.severity === 'warning')) {
        return 'warning';
    }
    return 'healthy';
}

function cloneEvidenceValue(value) {
    if (value === null || value === undefined) {
        return null;
    }
    if (typeof value === 'bigint') {
        return value.toString();
    }
    if (Array.isArray(value)) {
        return value.map(cloneEvidenceValue);
    }
    if (typeof value === 'object') {
        const out = {};
        for (const key of Object.keys(value).sort()) {
            out[key] = cloneEvidenceValue(value[key]);
        }
        return out;
    }
    return value;
}

function buildRouteHealthEvidencePayload(target, snapshot, alerts = null) {
    const normalizedAlerts = alerts || classifyRouteEvidenceAlerts(target, snapshot);
    return {
        source: 'governance-watchtower',
        evidence_type: 'route_health',
        health: routeHealthState(normalizedAlerts),
        target: {
            label: target.label,
            chain: target.chain,
            asset: target.asset,
            symbol: target.symbol || '',
            stats_method: target.statsMethod || target.stats_method || '',
        },
        route_status: cloneEvidenceValue(snapshot && snapshot.routeStatus ? snapshot.routeStatus : null),
        wrapped_token_stats: cloneEvidenceValue(snapshot && snapshot.stats ? snapshot.stats : null),
        alerts: normalizedAlerts.map((alert) => ({
            rule_id: alert.rule_id || alert.ruleId,
            severity: alert.severity,
            title: alert.title,
            event: cloneEvidenceValue(alert.event),
        })),
    };
}

function normalizeRouteHealthTarget(target, index) {
    if (!target || typeof target.chain !== 'string' || !target.chain.trim()) {
        throw new Error(`Route evidence target at index ${index} is missing chain`);
    }
    if (typeof target.asset !== 'string' || !target.asset.trim()) {
        throw new Error(`Route evidence target at index ${index} is missing asset`);
    }
    const chain = target.chain.trim().toLowerCase();
    const asset = target.asset.trim().toLowerCase();
    const symbol = String(target.symbol || target.wrappedSymbol || '').trim().toUpperCase();
    const statsMethod = String(
        target.statsMethod
        || target.stats_method
        || (symbol ? `get${symbol.charAt(0)}${symbol.slice(1).toLowerCase()}Stats` : ''),
    ).trim();
    return {
        label: String(target.label || `${chain}:${asset}`),
        chain,
        asset,
        symbol,
        statsMethod,
        neoNetwork: target.neoNetwork || target.neo_network,
        neoChainId: target.neoChainId || target.neo_chain_id || target.chainId || target.chain_id,
    };
}

function routeHealthEvidenceDomain(target, options = {}) {
    if (options.domain) {
        return normalizeEvidenceDomain(options.domain);
    }
    return normalizeEvidenceDomain({
        lichen_network: options.lichenNetwork || options.lichen_network || process.env.LICHEN_NETWORK || 'local',
        neo_network: options.neoNetwork || options.neo_network || target.neoNetwork || 't4',
        neo_chain_id: options.neoChainId || options.neo_chain_id || target.neoChainId || DEFAULT_NEOX_TESTNET_CHAIN_ID,
        route: options.route || target.chain,
        asset: options.asset || target.asset,
        purpose: options.purpose || DEFAULT_EVIDENCE_PURPOSE,
    });
}

function postJson(urlString, payload) {
    return new Promise((resolve, reject) => {
        const url = new URL(urlString);
        const transport = url.protocol === 'https:' ? https : http;
        const body = JSON.stringify(payload);
        const req = transport.request(
            url,
            {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                    'Content-Length': Buffer.byteLength(body),
                },
            },
            (res) => {
                let responseBody = '';
                res.setEncoding('utf8');
                res.on('data', (chunk) => {
                    responseBody += chunk;
                });
                res.on('end', () => {
                    if (!res.statusCode || res.statusCode < 200 || res.statusCode >= 300) {
                        reject(new Error(`POST ${urlString} returned ${res.statusCode || 0}: ${responseBody}`));
                        return;
                    }
                    resolve(responseBody);
                });
            },
        );
        req.on('error', reject);
        req.write(body);
        req.end();
    });
}

async function rpcRequest(rpcUrl, method, params = []) {
    const responseBody = await postJson(rpcUrl, {
        jsonrpc: '2.0',
        id: 1,
        method,
        params,
    });
    const response = JSON.parse(responseBody || '{}');
    if (response.error) {
        throw new Error(response.error.message || 'Unknown RPC error');
    }
    return response.result;
}

async function collectRouteHealthEvidence(options = {}) {
    if (!options.rpcUrl) {
        throw new Error('Route health evidence requires rpcUrl');
    }
    const targets = (options.routeHealthTargets || options.targets || []).map(normalizeRouteHealthTarget);
    if (targets.length === 0) {
        return [];
    }
    const rawSlot = options.slot !== undefined ? options.slot : await rpcRequest(options.rpcUrl, 'getSlot', []);
    const slot = Number(rawSlot && typeof rawSlot === 'object' && rawSlot.slot !== undefined ? rawSlot.slot : rawSlot);
    const ttlSlots = options.ttlSlots !== undefined
        ? normalizePositiveInteger(options.ttlSlots, 'ttlSlots')
        : DEFAULT_ROUTE_EVIDENCE_TTL_SLOTS;
    const issuedAtMs = options.issuedAtMs !== undefined ? options.issuedAtMs : Date.now();
    const expiresAtSlot = options.expiresAtSlot !== undefined ? options.expiresAtSlot : slot + ttlSlots;
    if (!options.manifestHash) {
        throw new Error('Route health evidence requires manifestHash');
    }
    const requiredSignatures = options.requiredSignatures !== undefined
        ? normalizePositiveInteger(options.requiredSignatures, 'requiredSignatures')
        : 1;
    const signers = options.signers || [];

    return Promise.all(targets.map(async (target) => {
        const [routeResult, statsResult] = await Promise.allSettled([
            rpcRequest(options.rpcUrl, 'getBridgeRouteRestrictionStatus', [target.chain, target.asset]),
            target.statsMethod ? rpcRequest(options.rpcUrl, target.statsMethod, []) : Promise.resolve(null),
        ]);
        const snapshot = {
            routeStatus: routeResult.status === 'fulfilled' ? routeResult.value : null,
            stats: statsResult.status === 'fulfilled' ? statsResult.value : null,
        };
        const unsigned = createUnsignedEvidence({
            kind: 'route_health',
            domain: routeHealthEvidenceDomain(target, options),
            payload: buildRouteHealthEvidencePayload(target, snapshot),
            slot,
            issuedAtMs,
            expiresAtSlot,
            manifestHash: options.manifestHash,
            requiredSignatures,
        });
        if (signers.length === 0) {
            return unsigned;
        }
        return signEvidenceEnvelope(unsigned, signers, { signMessage: options.signMessage });
    }));
}

module.exports = {
    DEFAULT_EVIDENCE_PURPOSE,
    DEFAULT_NEOX_TESTNET_CHAIN_ID,
    DEFAULT_ROUTE_EVIDENCE_TTL_SLOTS,
    EVIDENCE_MESSAGE_PREFIX,
    EVIDENCE_SCHEMA_VERSION,
    SUPPORTED_EVIDENCE_KINDS,
    buildRouteHealthEvidencePayload,
    bs58encode,
    canonicalBytes,
    canonicalize,
    classifyRouteEvidenceAlerts,
    collectRouteHealthEvidence,
    computeEvidenceId,
    createUnsignedEvidence,
    evidenceSigningBytes,
    hashCanonical,
    hexToBytes,
    normalizeEvidenceDomain,
    publicKeyToAddressBytes,
    routeHealthEvidenceDomain,
    sha256Hex,
    signEvidenceEnvelope,
    signerAddressFromPqSignature,
    stableStringify,
    verifyEvidenceEnvelope,
};
