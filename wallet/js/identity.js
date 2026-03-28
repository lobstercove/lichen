// ============================================================================
// LichenID Identity Module — Full wallet integration
// Handles: profile, .lichen names, reputation, skills, vouches, achievements,
//          agent service config, and all signed contract transactions
// ============================================================================

// ── Constants ──
const LICHENID_PROGRAM_ADDRESS = null; // Resolved at runtime via RPC
let _lichenidAddress = null;
let _identityCache = null;
let _identityLoading = false;

function clearIdentityCache() {
    _identityCache = null;
    _identityLoading = false;
}

const AGENT_TYPES = [
    { value: 0, label: 'Unknown', desc: 'Unspecified or new identity' },
    { value: 1, label: 'Trading', desc: 'Market-making, arbitrage, DeFi strategies' },
    { value: 2, label: 'Development', desc: 'Smart contracts, tooling, protocol dev' },
    { value: 3, label: 'Analysis', desc: 'On-chain analytics, data feeds, research' },
    { value: 4, label: 'Creative', desc: 'Content creation, design, media' },
    { value: 5, label: 'Infrastructure', desc: 'Validators, RPCs, indexers, relayers' },
    { value: 6, label: 'Governance', desc: 'Voting, proposals, DAO operations' },
    { value: 7, label: 'Oracle', desc: 'External data feeds, price oracles' },
    { value: 8, label: 'Storage', desc: 'Data persistence, archival, backups' },
    { value: 9, label: 'General', desc: 'Multi-purpose or uncategorized agent' },
    { value: 10, label: 'Personal', desc: 'Human user — personal identity' }
];

// ACHIEVEMENT_DEFS provided by shared/utils.js (loaded before this file)

const TRUST_TIERS = [
    { name: 'Newcomer', min: 0, color: '#6c7a89' },
    { name: 'Verified', min: 100, color: '#3498db' },
    { name: 'Trusted', min: 500, color: '#2ecc71' },
    { name: 'Established', min: 1000, color: '#f1c40f' },
    { name: 'Elite', min: 5000, color: '#e67e22' },
    { name: 'Legendary', min: 10000, color: '#e74c3c' },
];

const NAME_PRICING = {
    3: { cost: 500, label: '500 LICN', auctionOnly: true },
    4: { cost: 100, label: '100 LICN', auctionOnly: true },
    5: { cost: 20, label: '20 LICN', auctionOnly: false },
};

// ── Helpers ──
function getTrustTier(score) {
    for (let i = TRUST_TIERS.length - 1; i >= 0; i--) {
        if (score >= TRUST_TIERS[i].min) return TRUST_TIERS[i];
    }
    return TRUST_TIERS[0];
}

function getNextTier(score) {
    for (const t of TRUST_TIERS) {
        if (score < t.min) return t;
    }
    return null;
}

function escHtml(s) {
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
}

function fmtNumber(n) { return Number(n).toLocaleString(); }

function fmtLicn(spores) {
    return (Number(spores) / 1_000_000_000).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 });
}

function fmtAddr(addr, len = 8) {
    if (!addr || addr.length < 16) return addr || '—';
    return addr.slice(0, len) + '…' + addr.slice(-4);
}

function getAgentTypeName(val) {
    const t = AGENT_TYPES.find(a => a.value === Number(val));
    return t ? t.label : 'Unknown';
}

function getNameCostPerYear(nameLen) {
    if (nameLen <= 3) return 500;
    if (nameLen === 4) return 100;
    return 20;
}

// ── Binary Arg Encoding Helpers (WASM ABI Layout Descriptor) ──
// The LichenID contract uses raw WASM function params (pointers + values).
// The runtime's "layout descriptor" mode (0xAB prefix) lets us specify
// which I32 params are pointers (stride >= 32) vs raw values (stride < 32).

function buildLayoutArgs(layout, dataChunks) {
    const header = new Uint8Array(1 + layout.length);
    header[0] = 0xAB;
    for (let i = 0; i < layout.length; i++) header[1 + i] = layout[i];
    let totalData = 0;
    for (const c of dataChunks) totalData += c.length;
    const result = new Uint8Array(header.length + totalData);
    result.set(header, 0);
    let off = header.length;
    for (const c of dataChunks) { result.set(c, off); off += c.length; }
    return result;
}

function padBytes(data, targetLen) {
    if (data.length >= targetLen) return data.subarray(0, targetLen);
    const r = new Uint8Array(targetLen);
    r.set(data, 0);
    return r;
}

function u32LE(val) {
    const b = new Uint8Array(4);
    b[0] = val & 0xFF; b[1] = (val >> 8) & 0xFF;
    b[2] = (val >> 16) & 0xFF; b[3] = (val >> 24) & 0xFF;
    return b;
}

function u64LE(val) {
    const b = new Uint8Array(8);
    const big = BigInt(val);
    for (let i = 0; i < 8; i++) b[i] = Number((big >> BigInt(i * 8)) & 0xFFn);
    return b;
}

/**
 * Encode args for a LichenID contract call using the WASM ABI layout descriptor.
 * callerPubkey: Uint8Array (32 bytes) — the transaction signer's public key
 * functionName: string — the contract function to call
 * params: object — the high-level params from the modal (same keys as before)
 */
function encodeLichenIdArgs(callerPubkey, functionName, params) {
    const te = new TextEncoder();
    switch (functionName) {
        case 'register_identity': {
            // register_identity(owner_ptr:I32, agent_type:I32(u8), name_ptr:I32, name_len:I32(u32))
            const nameBytes = te.encode(params.name || '');
            return buildLayoutArgs([0x20, 0x01, 0x40, 0x04], [
                callerPubkey,                          // 32 B — owner
                new Uint8Array([params.agent_type & 0xFF]), // 1 B
                padBytes(nameBytes, 64),                // 64 B — name (padded)
                u32LE(nameBytes.length),                // 4 B — name_len
            ]);
        }
        case 'update_agent_type': {
            // update_agent_type(caller_ptr:I32, new_agent_type:I32(u8))
            return buildLayoutArgs([0x20, 0x01], [
                callerPubkey,
                new Uint8Array([params.agent_type & 0xFF]),
            ]);
        }
        case 'register_name': {
            // register_name(caller_ptr:I32, name_ptr:I32, name_len:I32(u32), duration_years:I32(u8))
            const nameBytes = te.encode(params.name || '');
            return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [
                callerPubkey,
                padBytes(nameBytes, 32),
                u32LE(nameBytes.length),
                new Uint8Array([(params.duration_years || 1) & 0xFF]),
            ]);
        }
        case 'renew_name': {
            // renew_name(caller_ptr:I32, name_ptr:I32, name_len:I32(u32), additional_years:I32(u8))
            const nameBytes = te.encode(params.name || '');
            return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [
                callerPubkey,
                padBytes(nameBytes, 32),
                u32LE(nameBytes.length),
                new Uint8Array([(params.additional_years || 1) & 0xFF]),
            ]);
        }
        case 'transfer_name': {
            // transfer_name(caller_ptr:I32, name_ptr:I32, name_len:I32(u32), new_owner_ptr:I32)
            const nameBytes = te.encode(params.name || '');
            const newOwnerBytes = bs58.decode(params.new_owner);
            return buildLayoutArgs([0x20, 0x20, 0x04, 0x20], [
                callerPubkey,
                padBytes(nameBytes, 32),
                u32LE(nameBytes.length),
                newOwnerBytes,
            ]);
        }
        case 'release_name': {
            // release_name(caller_ptr:I32, name_ptr:I32, name_len:I32(u32))
            const nameBytes = te.encode(params.name || '');
            return buildLayoutArgs([0x20, 0x20, 0x04], [
                callerPubkey,
                padBytes(nameBytes, 32),
                u32LE(nameBytes.length),
            ]);
        }
        case 'add_skill': {
            // add_skill(caller_ptr:I32, skill_name_ptr:I32, skill_name_len:I32(u32), proficiency:I32(u8))
            const nameBytes = te.encode(params.name || '');
            return buildLayoutArgs([0x20, 0x20, 0x04, 0x01], [
                callerPubkey,
                padBytes(nameBytes, 32),
                u32LE(nameBytes.length),
                new Uint8Array([(params.proficiency || 50) & 0xFF]),
            ]);
        }
        case 'vouch': {
            // vouch(voucher_ptr:I32, vouchee_ptr:I32) — both 32-byte pointers
            const voucheeBytes = bs58.decode(params.vouchee);
            return buildLayoutArgs([0x20, 0x20], [
                callerPubkey,
                voucheeBytes,
            ]);
        }
        case 'set_endpoint': {
            // set_endpoint(caller_ptr:I32, url_ptr:I32, url_len:I32(u32))
            const urlBytes = te.encode(params.url || '');
            const urlStride = Math.max(32, Math.min(255, urlBytes.length));
            return buildLayoutArgs([0x20, urlStride, 0x04], [
                callerPubkey,
                padBytes(urlBytes, urlStride),
                u32LE(urlBytes.length),
            ]);
        }
        case 'set_rate': {
            // set_rate(caller_ptr:I32, licn_per_unit:I64) — default mode works
            const data = new Uint8Array(32 + 8);
            data.set(callerPubkey, 0);
            data.set(u64LE(params.licn_per_unit || 0), 32);
            return data; // no layout prefix — default handles I32 ptr + I64 val
        }
        case 'set_availability': {
            // set_availability(caller_ptr:I32, status:I32(u8))
            return buildLayoutArgs([0x20, 0x01], [
                callerPubkey,
                new Uint8Array([(params.status || 0) & 0xFF]),
            ]);
        }
        case 'attest_skill': {
            // attest_skill(attester_ptr:I32, identity_ptr:I32, skill_name_ptr:I32, skill_name_len:I32(u32), level:I32(u8))
            const identityBytes = bs58.decode(params.identity);
            const skillNameBytes = te.encode(params.skill_name || '');
            return buildLayoutArgs([0x20, 0x20, 0x20, 0x04, 0x01], [
                callerPubkey,
                identityBytes,
                padBytes(skillNameBytes, 32),
                u32LE(skillNameBytes.length),
                new Uint8Array([(params.level || 50) & 0xFF]),
            ]);
        }
        case 'revoke_attestation': {
            // revoke_attestation(revoker_ptr:I32, identity_ptr:I32, skill_name_ptr:I32, skill_name_len:I32(u32))
            const identityBytes = bs58.decode(params.identity);
            const skillNameBytes = te.encode(params.skill_name || '');
            return buildLayoutArgs([0x20, 0x20, 0x20, 0x04], [
                callerPubkey,
                identityBytes,
                padBytes(skillNameBytes, 32),
                u32LE(skillNameBytes.length),
            ]);
        }
        case 'create_name_auction': {
            // create_name_auction(caller_ptr:I32, name_ptr:I32, name_len:I32(u32), reserve_bid:I64, end_slot:I64)
            const nameBytes = te.encode(params.name || '');
            // Layout: ptr(32), ptr(32), u32(4) — then raw I64 vals appended
            const base = buildLayoutArgs([0x20, 0x20, 0x04], [
                callerPubkey,
                padBytes(nameBytes, 32),
                u32LE(nameBytes.length),
            ]);
            // Append reserve_bid and end_slot as raw I64 LE
            const extra = new Uint8Array(16);
            extra.set(u64LE(params.reserve_bid || 0), 0);
            extra.set(u64LE(params.end_slot_offset || 432000), 8);
            const result = new Uint8Array(base.length + extra.length);
            result.set(base, 0);
            result.set(extra, base.length);
            return result;
        }
        case 'bid_name_auction': {
            // bid_name_auction(bidder_ptr:I32, name_ptr:I32, name_len:I32(u32), bid_amount:I64)
            // bid_amount is both a param AND must match tx value
            const nameBytes = te.encode(params.name || '');
            const base = buildLayoutArgs([0x20, 0x20, 0x04], [
                callerPubkey,
                padBytes(nameBytes, 32),
                u32LE(nameBytes.length),
            ]);
            // Append bid_amount as raw I64 LE
            const bidAmountSpores = Math.floor((params.bid_amount || 0) * 1_000_000_000);
            const extra = u64LE(bidAmountSpores);
            const result = new Uint8Array(base.length + extra.length);
            result.set(base, 0);
            result.set(extra, base.length);
            return result;
        }
        default:
            // Fallback: JSON-encode (legacy — will likely fail for ptr-param functions)
            return new TextEncoder().encode(JSON.stringify(params));
    }
}

// ── LichenID Program Address Resolution ──
async function getLichenIdProgramAddress() {
    if (_lichenidAddress) return _lichenidAddress;
    // Try each symbol individually (RPC expects a single string, not an array)
    const symbols = ['YID', 'yid', 'LICHENID'];
    for (const symbol of symbols) {
        try {
            const result = await rpc.call('getSymbolRegistry', [symbol]);
            const program = result?.program || result?.address || result?.pubkey;
            if (program) { _lichenidAddress = program; return _lichenidAddress; }
        } catch (_) { continue; }
    }
    // Fallback: scan full contract list
    try {
        const contracts = await rpc.call('getAllContracts');
        if (Array.isArray(contracts)) {
            const c = contracts.find(c => c.name === 'lichenid' || c.symbol === 'YID');
            if (c) { _lichenidAddress = c.program_id || c.address; return _lichenidAddress; }
        }
    } catch (_) { }
    return null;
}

// ── Contract Call Builder ──
// Build a signed LichenID contract call transaction
async function buildContractCall(functionName, args, password, valueLicn = 0) {
    const wallet = getActiveWallet();
    if (!wallet) throw new Error('No active wallet');

    // Pre-flight: check LICN balance covers fee + value
    try {
        const balResult = await rpc.call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / 1_000_000_000;
        const baseFee = 0.001; // 0.001 LICN base fee
        const totalNeeded = valueLicn + baseFee;
        if (spendable < totalNeeded) {
            throw new Error(`Insufficient LICN: need ${totalNeeded.toLocaleString(undefined, { maximumFractionDigits: 9 })} (${valueLicn > 0 ? valueLicn + ' value + ' : ''}${baseFee} fee), have ${spendable.toLocaleString(undefined, { maximumFractionDigits: 9 })} spendable`);
        }
    } catch (e) {
        if (e.message.includes('Insufficient')) throw e;
        // Non-blocking on RPC errors
    }

    const lichenidAddr = await getLichenIdProgramAddress();
    if (!lichenidAddr) throw new Error('LichenID contract not found on network');

    const latestBlock = await rpc.getLatestBlock();
    const fromPubkey = LichenCrypto.hexToBytes(wallet.publicKey);
    const contractProgramId = new Uint8Array(32).fill(0xFF);
    const lichenidPubkey = bs58.decode(lichenidAddr);

    // Encode args as proper binary with WASM ABI layout descriptor
    const argsBytes = encodeLichenIdArgs(fromPubkey, functionName, args);

    const callPayload = JSON.stringify({
        Call: {
            function: functionName,
            args: Array.from(argsBytes),
            value: Math.floor(valueLicn * 1_000_000_000)
        }
    });

    const message = {
        instructions: [{
            program_id: Array.from(contractProgramId),
            accounts: [Array.from(fromPubkey), Array.from(lichenidPubkey)],
            data: Array.from(new TextEncoder().encode(callPayload))
        }],
        blockhash: latestBlock.hash
    };

    const privateKey = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password);
    const messageBytes = serializeMessageBincode(message);
    const signature = await LichenCrypto.signTransaction(privateKey, messageBytes);

    const transaction = { signatures: [Array.from(signature)], message };
    const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
    return btoa(String.fromCharCode(...txBytes));
}

// ── RPC Data Loading ──
async function loadIdentityData() {
    const wallet = getActiveWallet();
    if (!wallet) return null;

    try {
        const [profile, lichenNameResult] = await Promise.all([
            rpc.call('getLichenIdProfile', [wallet.address]).catch(() => null),
            rpc.call('reverseLichenName', [wallet.address]).catch(() => null),
        ]);
        // reverseLichenName returns {"name": "alice.lichen"} or null — extract the string
        const lichenName = lichenNameResult?.name || null;

        let nameDetails = null;
        if (lichenName) {
            try {
                nameDetails = await rpc.call('resolveLichenName', [lichenName]);
            } catch (_) { }
        }

        _identityCache = {
            profile,
            lichenName,
            nameDetails,
            address: wallet.address,
            timestamp: Date.now()
        };
        return _identityCache;
    } catch (e) {
        console.error('Failed to load identity:', e);
        return null;
    }
}

// ============================================================================
// MAIN RENDER
// ============================================================================
async function loadIdentity() {
    const container = document.getElementById('identityContent');
    if (!container) return;

    if (_identityLoading) return;
    _identityLoading = true;

    container.innerHTML = `
        <div class="id-loading">
            <i class="fas fa-spinner fa-spin"></i>
            <span>Loading LichenID...</span>
        </div>
    `;

    const data = await loadIdentityData();
    _identityLoading = false;

    if (!data || !data.profile?.identity) {
        renderNoIdentity(container);
        return;
    }

    renderIdentity(container, data);
}

// Retry loading identity after a state-changing tx (register, edit, etc.)
// Retries `maxRetries` times with `delayMs` between attempts.
async function retryLoadIdentity(maxRetries = 5, delayMs = 1200) {
    for (let i = 0; i < maxRetries; i++) {
        await new Promise(r => setTimeout(r, delayMs));
        _identityCache = null;
        _identityLoading = false;
        const data = await loadIdentityData();
        if (data && data.profile?.identity) {
            await loadIdentity();
            return;
        }
    }
    // Fallback — just re-render whatever state we have
    await loadIdentity();
}

if (typeof window !== 'undefined') {
    window.clearIdentityCache = clearIdentityCache;
}

// ── LichenID Intro Banner ──
function renderIdentityBanner(compact = false) {
    if (compact) {
        return `
            <div class="id-banner id-banner-compact">
                <div class="id-banner-icon"><i class="fas fa-fingerprint"></i></div>
                <div class="id-banner-text">
                    <h3>LichenID</h3>
                    <p>On-chain identity, .lichen name, reputation &amp; agent services.</p>
                </div>
            </div>
        `;
    }
    return `
        <div class="id-banner">
            <div class="id-banner-icon"><i class="fas fa-fingerprint"></i></div>
            <div class="id-banner-text">
                <h3>LichenID — On-Chain Identity</h3>
                <p>Your portable reputation, .lichen name, skills, and agent service profile — all anchored on Lichen.</p>
            </div>
        </div>
    `;
}

// ── No Identity — Simple Registration Card ──
function renderNoIdentity(container) {
    container.innerHTML = `
        <div class="id-onboard">
            <div class="id-onboard-step id-onboard-active" onclick="showRegisterIdentityModal()">
                <div class="id-onboard-num"><i class="fas fa-fingerprint" style="font-size:1rem;"></i></div>
                <div class="id-onboard-body">
                    <div class="id-onboard-title">Register Your LichenID</div>
                    <div class="id-onboard-desc">Create your on-chain identity — choose a display name and agent type. Free — only the 0.0001 LICN transaction fee.</div>
                </div>
                <div class="id-onboard-arrow"><i class="fas fa-chevron-right"></i></div>
            </div>
        </div>
    `;
}

// ── Full Identity Render ──
function renderIdentity(container, data) {
    const { profile, lichenName, nameDetails } = data;
    const identity = profile.identity;
    const rep = Number(profile?.reputation?.score || identity?.reputation || 0);
    const tier = getTrustTier(rep);
    const nextTier = getNextTier(rep);
    const maxRep = 100000;
    const repPct = Math.min(100, (rep / maxRep) * 100);

    const agentType = getAgentTypeName(identity.agent_type);
    const agentDesc = AGENT_TYPES.find(a => a.value === Number(identity.agent_type))?.desc || '';
    const rawName = identity?.name || profile?.identity?.display_name || 'Unnamed';
    const displayName = escHtml(rawName);
    const isActive = identity.is_active !== false && identity.is_active !== 0;

    const skills = Array.isArray(profile?.skills) ? profile.skills : [];
    const achievements = Array.isArray(profile?.achievements) ? profile.achievements : [];
    const vouchesReceived = Array.isArray(profile?.vouches?.received) ? profile.vouches.received : [];
    const vouchesGiven = Array.isArray(profile?.vouches?.given) ? profile.vouches.given : [];
    const contributions = profile?.contributions || {};

    const achievedIds = new Set(achievements.map(a => Number(a.id)).filter(Boolean));

    // Agent service data
    const endpoint = profile?.agent?.endpoint || '';
    const availability = profile?.agent?.availability_name || 'offline';
    const rateLicn = (Number(profile?.agent?.rate || 0) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 9 });

    container.innerHTML = `
        ${renderProfileStrip(displayName, agentType, agentDesc, tier, rep, isActive, lichenName)}
        <div class="id-grid">
            ${renderRepSection(rep, tier, nextTier, repPct, maxRep)}
            ${renderNameSection(lichenName, nameDetails)}
            ${renderSkillsSection(skills)}
            ${renderVouchesSection(vouchesReceived, vouchesGiven)}
            ${renderAchievementsSection(achievements, achievedIds)}
            ${renderAgentSection(endpoint, availability, rateLicn, profile?.agent?.metadata)}
        </div>
    `;
    // Load auctions after DOM is rendered
    setTimeout(() => loadAuctionList(), 100);
}

// ============================================================================
// SECTION RENDERERS — Compact grid layout
// ============================================================================

// ── Profile Strip (top bar, always visible when has identity) ──
function renderProfileStrip(displayName, agentType, agentDesc, tier, rep, isActive, lichenName) {
    const lichenDisplay = lichenName ? escHtml((lichenName.endsWith('.lichen') ? lichenName : lichenName + '.lichen')) : null;
    // Avoid showing "name name.lichen" when display name matches the .lichen name
    const lichenBase = lichenName ? lichenName.replace(/\.lichen$/, '').toLowerCase() : null;
    const rawDisplayLower = String(displayName).toLowerCase().replace(/\.lichen$/, '');
    const showDisplayName = !lichenDisplay || rawDisplayLower !== lichenBase;
    return `
        <div class="id-profile-strip">
            <div class="id-strip-avatar" style="background:${tier.color}18; border-color:${tier.color};">
                <i class="fas fa-fingerprint" style="color:${tier.color};"></i>
            </div>
            <div class="id-strip-info">
                <div class="id-strip-name">
                    ${showDisplayName ? escHtml(displayName) : ''}
                    ${lichenDisplay ? `<span class="id-strip-lichen">${lichenDisplay}</span>` : (showDisplayName ? '' : escHtml(displayName))}
                </div>
                <div class="id-strip-meta">
                    <span class="id-pill" style="background:${tier.color}18;color:${tier.color};border-color:${tier.color}33;">${tier.name}</span>
                    <span class="id-pill">${agentType}</span>
                    ${isActive
            ? '<span class="id-pill id-pill-success"><i class="fas fa-circle" style="font-size:0.35em;vertical-align:middle;"></i> Active</span>'
            : '<span class="id-pill id-pill-muted">Inactive</span>'}
                    <span class="id-strip-rep">${fmtNumber(rep)} rep</span>
                </div>
            </div>
            <button class="id-action-btn" onclick="showEditProfileModal()" title="Edit Profile">
                <i class="fas fa-pen"></i>
            </button>
        </div>
    `;
}

// ── Reputation Section ──
function renderRepSection(rep, tier, nextTier, repPct, maxRep) {
    const nextInfo = nextTier
        ? `<span class="id-next-label">Next: <strong>${nextTier.name}</strong> at ${fmtNumber(nextTier.min)}</span>`
        : '<span class="id-next-label"><strong>Max tier reached</strong></span>';

    return `
        <div class="id-section">
            <div class="id-section-head">
                <span><i class="fas fa-chart-line"></i> Reputation</span>
            </div>
            <div class="id-section-body">
                <div class="id-rep-row">
                    <span class="id-rep-number">${fmtNumber(rep)}</span>
                    <span class="id-rep-max">/ ${fmtNumber(maxRep)}</span>
                </div>
                <div class="id-progress">
                    <div class="id-progress-bar" style="width:${repPct}%; background:${tier.color};"></div>
                </div>
                <div class="id-tier-steps">${TRUST_TIERS.map(t => {
        const active = rep >= t.min;
        return `<span class="id-tier-step${active ? ' active' : ''}" style="${active ? `background:${t.color}18;color:${t.color};border-color:${t.color}33;` : ''}">${t.name}</span>`;
    }).join('')}</div>
                ${nextInfo}
            </div>
        </div>
    `;
}

// ── Name Section ──
function renderNameSection(lichenName, nameDetails) {
    // Auction sub-section (always shown)
    const auctionHtml = `
        <div class="id-section-divider" style="margin-top:0.75rem;padding-top:0.75rem;border-top:1px solid var(--border,#2a2e3e);">
            <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:0.5rem;">
                <span style="font-size:0.8rem;opacity:0.7;"><i class="fas fa-gavel"></i> Premium Name Auctions</span>
                <button class="id-link-btn" onclick="loadAuctionList()"><i class="fas fa-sync-alt"></i> Refresh</button>
            </div>
            <div id="auctionListContainer" style="font-size:0.8rem;">
                <span style="opacity:0.5;">Loading auctions...</span>
            </div>
            <div id="adminAuctionCreate"></div>
        </div>
    `;

    if (!lichenName) {
        return `
            <div class="id-section">
                <div class="id-section-head">
                    <span><i class="fas fa-at"></i> .lichen Name</span>
                </div>
                <div class="id-section-body id-section-empty">
                    <p>No name registered</p>
                    <small>5+ chars from 20 LICN/yr</small>
                    <div style="margin-top:0.75rem;">
                        <button class="btn btn-small btn-primary" onclick="showRegisterNameModal()">
                            <i class="fas fa-plus"></i> Register
                        </button>
                    </div>
                    ${auctionHtml}
                </div>
            </div>
        `;
    }

    const name = lichenName.endsWith('.lichen') ? lichenName : lichenName + '.lichen';
    const expiry = nameDetails?.expiry_slot;
    const registered = nameDetails?.registered_slot || 0;
    // Expiry will be updated asynchronously
    const expiryId = 'nameExpiry_' + Date.now();

    // Kick off async slot fetch to update expiry display
    (async () => {
        try {
            const currentSlot = await getCurrentSlot();
            const el = document.getElementById(expiryId);
            if (el) el.textContent = 'Expires ' + (expiry ? formatSlotExpiry(expiry, registered, currentSlot) : '—');
        } catch (_) { }
    })();

    return `
        <div class="id-section">
            <div class="id-section-head">
                <span><i class="fas fa-at"></i> .lichen Name</span>
            </div>
            <div class="id-section-body">
                <div class="id-name-display">${escHtml(name)}</div>
                <div class="id-name-expiry" id="${expiryId}">Expires ${expiry ? formatSlotExpiry(expiry, registered, _cachedCurrentSlot) : '—'}</div>
                <div class="id-section-actions">
                    <button class="id-link-btn" onclick="showRenewNameModal()"><i class="fas fa-redo"></i> Renew</button>
                    <button class="id-link-btn" onclick="showTransferNameModal()"><i class="fas fa-exchange-alt"></i> Transfer</button>
                    <button class="id-link-btn id-link-danger" onclick="showReleaseNameModal()"><i class="fas fa-trash-alt"></i> Release</button>
                </div>
                ${auctionHtml}
            </div>
        </div>
    `;
}

// ── Skills Section ──
function renderSkillsSection(skills) {
    const list = skills.length > 0
        ? skills.slice(0, 8).map(s => {
            const name = escHtml(String(s.name || s.skill || 'Unnamed'));
            const prof = Number(s.proficiency || s.level || 0);
            const level = Math.max(0, Math.min(5, Math.round(prof / 20) || prof));
            const attCount = Number(s.attestation_count || s.attestations || 0);
            const pct = (level / 5) * 100;
            return `
                <div class="id-skill-row">
                    <span class="id-skill-name">${name}</span>
                    <div class="id-skill-bar"><div class="id-progress-bar" style="width:${pct}%;"></div></div>
                    <span class="id-skill-lvl">${level}/5</span>
                </div>
            `;
        }).join('')
        : '<div class="id-section-empty"><p>No skills yet</p></div>';

    return `
        <div class="id-section">
            <div class="id-section-head">
                <span><i class="fas fa-tools"></i> Skills</span>
                <button class="id-link-btn" onclick="showAddSkillModal()"><i class="fas fa-plus"></i> Add</button>
            </div>
            <div class="id-section-body">${list}</div>
        </div>
    `;
}

// ── Vouches Section ──
function renderVouchesSection(received, given) {
    const chips = received.length > 0
        ? received.slice(0, 12).map(v => {
            const label = v.voucher_name ? escHtml(v.voucher_name) + '.lichen' : fmtAddr(v.voucher, 8);
            return `<span class="id-chip">${label}</span>`;
        }).join('')
        : '<span class="id-chip id-chip-muted">None yet</span>';

    return `
        <div class="id-section">
            <div class="id-section-head">
                <span><i class="fas fa-handshake"></i> Vouches</span>
                <button class="id-link-btn" onclick="showVouchModal()"><i class="fas fa-plus"></i> Vouch</button>
            </div>
            <div class="id-section-body">
                <div class="id-vouch-counts">
                    <span><strong>${received.length}</strong> received</span>
                    <span><strong>${given.length}</strong> given</span>
                </div>
                <div class="id-chip-list">${chips}</div>
            </div>
        </div>
    `;
}

// ── Achievements Section ──
function renderAchievementsSection(achievements, achievedIds) {
    const all = ACHIEVEMENT_DEFS.map(def => {
        const earned = achievedIds.has(def.id);
        return `<span class="id-badge ${earned ? 'id-badge-earned' : 'id-badge-locked'}" title="${escHtml(def.desc)}"><i class="fas ${def.icon}"></i> ${def.name}</span>`;
    }).join('');

    return `
        <div class="id-section id-section-full">
            <div class="id-section-head">
                <span><i class="fas fa-award"></i> Achievements</span>
                <span class="id-section-counter">${achievements.length}/${ACHIEVEMENT_DEFS.length}</span>
            </div>
            <div class="id-section-body">
                <div class="id-chip-list">${all}</div>
            </div>
        </div>
    `;
}

// ── Agent Service Section ──
function renderAgentSection(endpoint, availability, rateLicn, metadata) {
    const isOnline = availability === 'online';
    return `
        <div class="id-section id-section-full">
            <div class="id-section-head">
                <span><i class="fas fa-satellite-dish"></i> Agent Service</span>
                <button class="id-link-btn" onclick="showEditAgentModal()"><i class="fas fa-cog"></i> Configure</button>
            </div>
            <div class="id-section-body">
                <div class="id-agent-grid">
                    <div class="id-agent-item">
                        <span class="id-kv-label">Endpoint</span>
                        <span class="id-kv-value id-mono">${endpoint ? escHtml(endpoint) : '<em style="opacity:0.4;">Not set</em>'}</span>
                    </div>
                    <div class="id-agent-item">
                        <span class="id-kv-label">Status</span>
                        <span class="id-kv-value">${isOnline
            ? '<span class="id-pill id-pill-success"><i class="fas fa-circle" style="font-size:0.35em;vertical-align:middle;"></i> Online</span>'
            : '<span class="id-pill id-pill-muted"><i class="fas fa-circle" style="font-size:0.35em;vertical-align:middle;"></i> Offline</span>'}</span>
                    </div>
                    <div class="id-agent-item">
                        <span class="id-kv-label">Rate</span>
                        <span class="id-kv-value">${rateLicn} LICN/req</span>
                    </div>
                </div>
            </div>
        </div>
    `;
}

// ============================================================================
// SLOT EXPIRY FORMATTER — queries chain for real current slot
// ============================================================================
let _cachedCurrentSlot = null;
let _cachedSlotTime = 0;

async function getCurrentSlot() {
    // Cache for 30 seconds to avoid hammering RPC
    if (_cachedCurrentSlot && Date.now() - _cachedSlotTime < 30000) return _cachedCurrentSlot;
    try {
        const slot = await rpc.call('getSlot', []);
        if (typeof slot === 'number' && slot > 0) {
            _cachedCurrentSlot = slot;
            _cachedSlotTime = Date.now();
            return slot;
        }
    } catch (_) { }
    return _cachedCurrentSlot || 0;
}

function formatSlotExpiry(expirySlot, registeredSlot, currentSlot) {
    const slot = Number(expirySlot || 0);
    if (!slot) return 'Unknown';

    const regSlot = Number(registeredSlot || 0);
    const durationSlots = slot - regSlot;
    const durationYears = Math.max(1, Math.round(durationSlots / SLOTS_PER_YEAR));

    // Use chain's current slot for accurate date calculation
    const curSlot = currentSlot || _cachedCurrentSlot;
    if (curSlot && curSlot > 0) {
        const remainingSlots = slot - curSlot;
        if (remainingSlots <= 0) {
            return '<span style="color:#ef4444;">Expired</span>';
        }
        const remainingMs = remainingSlots * MS_PER_SLOT;
        const remainingDays = Math.floor(remainingMs / 86_400_000);
        const approxDate = new Date(Date.now() + remainingMs);
        const dateStr = approxDate.toLocaleDateString(undefined, { year: 'numeric', month: 'short' });
        const daysLabel = remainingDays > 365
            ? `~${(remainingDays / 365).toFixed(1)}yr`
            : `${remainingDays}d`;
        // Warn if expiring within 90 days
        if (remainingDays <= 90) {
            return `<span style="color:#f59e0b;">${dateStr} (${daysLabel} left)</span>`;
        }
        return `${dateStr} (${daysLabel} left)`;
    }

    // Fallback: show relative duration only
    return `~${durationYears}yr from registration`;
}

// ============================================================================
// MODALS — Identity Actions
// ============================================================================

// ── Register Identity ──
async function showRegisterIdentityModal() {
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: 'Register LichenID',
        message: 'Create your on-chain identity. Choose a display name and agent type.<br><small style="color:var(--text-muted);">This is free — only the 0.0001 LICN transaction fee applies.</small>',
        icon: 'fas fa-fingerprint',
        confirmText: 'Register',
        requiredLicn: FEE,
        fields: [
            { id: 'displayName', label: 'Display Name', type: 'text', placeholder: 'e.g. CryptoBuilder' },
            {
                id: 'agentType', label: 'Agent Type', type: 'select',
                options: AGENT_TYPES.map(t => ({ value: t.value, label: `${t.label} — ${t.desc}` }))
            },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;

    const displayName = (values.displayName || '').trim();
    if (!displayName || displayName.length > 64) {
        showToast('Display name required (1-64 characters)');
        return;
    }

    try {
        showToast('Registering identity...');
        const agentType = parseInt(values.agentType || '9');
        const tx = await buildContractCall('register_identity', {
            agent_type: agentType,
            name: displayName
        }, values.password);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Contract error: ' + (result.error || 'unknown'));
            return;
        }
        showToast('Identity registered! Loading profile...');
        _identityCache = null;
        // Show immediate feedback — swap to loading state
        const container = document.getElementById('identityContent');
        if (container) {
            container.innerHTML = `
                <div class="id-loading">
                    <i class="fas fa-spinner fa-spin"></i>
                    <span>Identity registered — loading dashboard...</span>
                </div>
            `;
        }
        // Retry loading identity — the tx may take 1-3 blocks to be indexed
        await retryLoadIdentity(10, 1500);
    } catch (e) {
        showToast('Registration failed: ' + e.message);
    }
}

// ── Edit Profile (Agent Type) ──
async function showEditProfileModal() {
    const current = _identityCache?.profile?.identity?.agent_type || 9;
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: 'Update Agent Type',
        message: 'Change your agent classification.',
        icon: 'fas fa-id-badge',
        confirmText: 'Update',
        requiredLicn: FEE,
        fields: [
            {
                id: 'agentType', label: 'Agent Type', type: 'select',
                options: AGENT_TYPES.map(t => ({ value: t.value, label: `${t.label} — ${t.desc}`, selected: t.value === current }))
            },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;

    try {
        showToast('Updating agent type...');
        const tx = await buildContractCall('update_agent_type', { agent_type: parseInt(values.agentType) }, values.password);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Update failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast('Agent type updated!');
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Update failed: ' + e.message);
    }
}

// ── Register .lichen Name ──
async function showRegisterNameModal() {
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: 'Register .lichen Name',
        requiredLicn: 20 + FEE,
        message: `
            <div class="id-pricing-table">
                <div class="id-pricing-row"><span>5+ chars</span><span>20 LICN/year</span></div>
                <div class="id-pricing-row id-pricing-muted"><span>4 chars</span><span>100 LICN/year <em>(auction only)</em></span></div>
                <div class="id-pricing-row id-pricing-muted"><span>3 chars</span><span>500 LICN/year <em>(auction only)</em></span></div>
            </div>
            <small>Names are lowercase, 5-32 chars (a-z, 0-9, hyphens). Duration: 1-10 years.<br>Premium short names (3-4 chars) can only be acquired through the auction system.</small>
            <div id="nameRegCostPreview" style="margin-top:0.75rem;padding:0.5rem 0.75rem;background:var(--bg-tertiary,#1a1e2e);border-radius:8px;font-size:0.85rem;display:none;">
                <span style="opacity:0.7;">Total cost:</span> <strong id="nameRegCostValue">—</strong>
            </div>
        `,
        icon: 'fas fa-at',
        confirmText: 'Register Name',
        fields: [
            { id: 'name', label: 'Name (without .lichen)', type: 'text', placeholder: 'myname (5+ characters)' },
            { id: 'duration', label: 'Duration (years)', type: 'number', placeholder: '1', min: 1, max: 10, step: 1 },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ],
        onRender: (modal) => {
            const nameInput = modal.querySelector('#name');
            const durationInput = modal.querySelector('#duration');
            const preview = modal.querySelector('#nameRegCostPreview');
            const costValue = modal.querySelector('#nameRegCostValue');
            // Enforce lowercase and strip invalid chars (only a-z, 0-9, hyphens allowed)
            if (nameInput) {
                nameInput.style.textTransform = 'lowercase';
                nameInput.addEventListener('input', () => {
                    const pos = nameInput.selectionStart;
                    // Lowercase, strip dots and anything not a-z 0-9 or hyphen
                    nameInput.value = nameInput.value.toLowerCase().replace(/[^a-z0-9-]/g, '');
                    nameInput.setSelectionRange(Math.min(pos, nameInput.value.length), Math.min(pos, nameInput.value.length));
                });
            }
            const updateCost = () => {
                const n = (nameInput?.value || '').toLowerCase().replace(/\.lichen$/, '').trim();
                const d = Math.max(1, Math.min(10, parseInt(durationInput?.value) || 1));
                if (n.length >= 5) {
                    const costPerYear = getNameCostPerYear(n.length);
                    const total = costPerYear * d;
                    if (costValue) costValue.textContent = `${total} LICN (${costPerYear} LICN × ${d} yr)`;
                    if (preview) preview.style.display = 'block';
                } else {
                    if (preview) preview.style.display = 'none';
                }
            };
            if (nameInput) nameInput.addEventListener('input', updateCost);
            if (durationInput) durationInput.addEventListener('input', updateCost);
        }
    });
    if (!values || !values.password || !values.name) return;

    const name = values.name.toLowerCase().replace(/\.lichen$/, '').trim();
    const duration = Math.max(1, Math.min(10, parseInt(values.duration) || 1));
    const costPerYear = getNameCostPerYear(name.length);
    const totalCost = costPerYear * duration;

    // Validate name length — 5+ chars required (3-4 are auction-only)
    if (name.length < 5) {
        showToast('Name must be at least 5 characters. 3-4 char names are auction-only.');
        return;
    }

    // Validate name format
    if (name.length > 32 || !/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(name)) {
        showToast('Invalid name: 5-32 chars, a-z 0-9 hyphens, no leading/trailing hyphens');
        return;
    }

    try {
        showToast(`Registering ${name}.lichen for ${duration}yr (${totalCost} LICN)...`);
        const tx = await buildContractCall('register_name', {
            name: name,
            duration_years: duration
        }, values.password, totalCost);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Registration failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast(`${name}.lichen registered!`);
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Name registration failed: ' + e.message);
    }
}

// ── Renew .lichen Name ──
async function showRenewNameModal() {
    const currentName = _identityCache?.lichenName;
    if (!currentName) { showToast('No name to renew'); return; }

    const name = currentName.replace(/\.lichen$/, '');
    const costPerYear = getNameCostPerYear(name.length);

    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: `Renew ${name}.lichen`,
        message: `Cost: ${costPerYear} LICN per additional year.`,
        icon: 'fas fa-redo',
        confirmText: 'Renew Name',
        requiredLicn: costPerYear + FEE,
        fields: [
            { id: 'years', label: 'Additional Years', type: 'number', placeholder: '1', min: 1, max: 10, step: 1 },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;

    const years = Math.max(1, Math.min(10, parseInt(values.years) || 1));
    const totalCost = costPerYear * years;

    try {
        showToast(`Renewing for ${years}yr (${totalCost} LICN)...`);
        const tx = await buildContractCall('renew_name', {
            name: name,
            additional_years: years
        }, values.password, totalCost);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Renewal failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast('Name renewed!');
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Renewal failed: ' + e.message);
    }
}

// ── Transfer .lichen Name ──
async function showTransferNameModal() {
    const currentName = _identityCache?.lichenName;
    if (!currentName) { showToast('No name to transfer'); return; }

    const name = currentName.replace(/\.lichen$/, '');

    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: `Transfer ${name}.lichen`,
        message: 'Transfer ownership to another address. This is irreversible.',
        icon: 'fas fa-exchange-alt',
        confirmText: 'Transfer Name',
        requiredLicn: FEE,
        fields: [
            { id: 'recipient', label: 'Recipient Address', type: 'text', placeholder: 'Base58 address' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.recipient) return;

    // AUDIT-FIX W-6: Validate recipient address format before building tx
    if (!LichenCrypto.isValidAddress(values.recipient)) {
        showToast('Invalid recipient address — must be a valid Base58 address');
        return;
    }

    try {
        showToast('Transferring name...');
        const tx = await buildContractCall('transfer_name', {
            name: name,
            new_owner: values.recipient
        }, values.password);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Transfer failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast('Name transferred!');
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Transfer failed: ' + e.message);
    }
}

// ── Release .lichen Name ──
async function showReleaseNameModal() {
    const currentName = _identityCache?.lichenName;
    if (!currentName) { showToast('No name to release'); return; }

    const name = currentName.replace(/\.lichen$/, '');

    const confirmed = await showConfirmModal({
        title: `Release ${name}.lichen?`,
        message: 'This will permanently release your .lichen name. It can be re-registered by anyone. This action cannot be undone.',
        icon: 'fas fa-exclamation-triangle',
        confirmText: 'Release Name',
        cancelText: 'Keep Name',
        danger: true
    });
    if (!confirmed) return;

    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: 'Confirm Release',
        message: `You are about to release <strong>${escHtml(name)}.lichen</strong>.`,
        icon: 'fas fa-trash-alt',
        confirmText: 'Sign & Release',
        requiredLicn: FEE,
        fields: [
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;

    try {
        showToast('Releasing name...');
        const tx = await buildContractCall('release_name', { name: name }, values.password);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Release failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast('Name released');
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Release failed: ' + e.message);
    }
}

// ── Add Skill ──
async function showAddSkillModal() {
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: 'Add Skill',
        message: 'Add a skill to your identity profile.',
        icon: 'fas fa-tools',
        confirmText: 'Add Skill',
        requiredLicn: FEE,
        fields: [
            { id: 'skillName', label: 'Skill Name', type: 'text', placeholder: 'e.g. Rust, Trading, Security' },
            { id: 'proficiency', label: 'Proficiency (1-100)', type: 'number', placeholder: '50', min: 1, max: 100, step: 1 },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.skillName) return;

    const skillName = values.skillName.trim();
    if (!skillName || skillName.length > 64) {
        showToast('Skill name required (1-64 chars)');
        return;
    }

    const proficiency = Math.max(1, Math.min(100, parseInt(values.proficiency) || 50));

    try {
        showToast('Adding skill...');
        const tx = await buildContractCall('add_skill', {
            name: skillName,
            proficiency: proficiency
        }, values.password);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast('Skill added!');
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Failed: ' + e.message);
    }
}

// ── Vouch for Someone ──
async function showVouchModal() {
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: 'Vouch for Identity',
        message: 'Vouch for another LichenID holder. Both parties must have registered identities.',
        icon: 'fas fa-handshake',
        confirmText: 'Vouch',
        requiredLicn: FEE,
        fields: [
            { id: 'vouchee', label: 'Address to Vouch For', type: 'text', placeholder: 'Base58 address' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.vouchee) return;

    // AUDIT-FIX W-6: Validate vouchee address format before building tx
    if (!LichenCrypto.isValidAddress(values.vouchee)) {
        showToast('Invalid address — must be a valid Base58 address');
        return;
    }

    try {
        showToast('Sending vouch...');
        const tx = await buildContractCall('vouch', { vouchee: values.vouchee }, values.password);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Vouch failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast('Vouch sent!');
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Vouch failed: ' + e.message);
    }
}

// ── Edit Agent Service Configuration ──
async function showEditAgentModal() {
    const agent = _identityCache?.profile?.agent || {};
    const currentEndpoint = agent.endpoint || '';
    const currentRate = (Number(agent.rate || 0) / 1_000_000_000).toString();
    const currentAvailability = agent.availability_name || 'offline';
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;

    const values = await showPasswordModal({
        title: 'Agent Service Configuration',
        message: 'Configure how other agents discover and interact with your identity.',
        icon: 'fas fa-satellite-dish',
        confirmText: 'Save Changes',
        requiredLicn: FEE,
        fields: [
            { id: 'endpoint', label: 'Service Endpoint URL', type: 'text', placeholder: 'https://api.example.com/agent', value: currentEndpoint },
            { id: 'rate', label: 'Rate (LICN per request)', type: 'number', placeholder: '0.001', value: currentRate },
            {
                id: 'availability', label: 'Availability', type: 'select',
                options: [
                    { value: 'online', label: 'Online', selected: currentAvailability === 'online' },
                    { value: 'offline', label: 'Offline', selected: currentAvailability !== 'online' }
                ]
            },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;

    try {
        const tasks = [];

        // Update endpoint if changed
        if (values.endpoint !== currentEndpoint) {
            tasks.push(async () => {
                const tx = await buildContractCall('set_endpoint', { url: values.endpoint }, values.password);
                return await rpc.sendTransaction(tx);
            });
        }

        // Update rate if changed
        const newRateLicn = parseFloat(values.rate || '0');
        const newRateSpores = Math.floor(newRateLicn * 1_000_000_000);
        const oldRateSpores = Number(agent.rate || 0);
        if (newRateSpores !== oldRateSpores) {
            tasks.push(async () => {
                const tx = await buildContractCall('set_rate', { licn_per_unit: newRateSpores }, values.password);
                return await rpc.sendTransaction(tx);
            });
        }

        // Update availability if changed
        const newAvailNum = values.availability === 'online' ? 1 : 0;
        const oldAvailNum = currentAvailability === 'online' ? 1 : 0;
        if (newAvailNum !== oldAvailNum) {
            tasks.push(async () => {
                const tx = await buildContractCall('set_availability', { status: newAvailNum }, values.password);
                return await rpc.sendTransaction(tx);
            });
        }

        if (tasks.length === 0) {
            showToast('No changes to save');
            return;
        }

        showToast('Saving agent configuration...');
        // Execute sequentially (each needs fresh blockhash)
        for (const task of tasks) {
            const taskResult = await task();
            if (taskResult?.error) {
                showToast('Update failed: ' + (taskResult.error || 'unknown'));
                return;
            }
        }
        showToast('Agent service updated!');
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Update failed: ' + e.message);
    }
}

// ============================================================================
// AUCTION SYSTEM — Premium .lichen Name Auctions
// ============================================================================

// Known premium names that may have auctions (admin curated)
const PREMIUM_AUCTION_NAMES = ['eth', 'btc', 'sol', 'dex', 'dao', 'nft', 'defi', 'swap', 'lend', 'pay', 'lichen', 'moss', 'spore', 'moon', 'pump'];

async function loadAuctionList() {
    const container = document.getElementById('auctionListContainer');
    if (!container) return;
    container.innerHTML = '<span style="opacity:0.5;"><i class="fas fa-spinner fa-spin"></i> Checking auctions...</span>';

    try {
        const currentSlot = await getCurrentSlot();
        const auctions = [];
        // Query known premium names for active auctions
        for (const name of PREMIUM_AUCTION_NAMES) {
            try {
                const result = await rpc.call('getNameAuction', [name]);
                if (result && result.active) {
                    auctions.push({ ...result, name });
                }
            } catch (_) { }
        }

        if (auctions.length === 0) {
            container.innerHTML = '<span style="opacity:0.5;">No active auctions</span>';
        } else {
            container.innerHTML = auctions.map(a => {
                const ended = a.ended || (currentSlot > a.end_slot);
                const highBid = (a.highest_bid / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
                const reserve = (a.reserve_bid / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
                const bidder = a.highest_bidder && a.highest_bidder !== '11111111111111111111111111111111'
                    ? a.highest_bidder.slice(0, 6) + '...' + a.highest_bidder.slice(-4)
                    : 'None';
                const slotsLeft = Math.max(0, a.end_slot - currentSlot);
                const secsLeft = Math.floor(slotsLeft / 2);
                const timeLeft = secsLeft > 86400 ? Math.floor(secsLeft / 86400) + 'd' :
                    secsLeft > 3600 ? Math.floor(secsLeft / 3600) + 'h' :
                        secsLeft > 60 ? Math.floor(secsLeft / 60) + 'm' : secsLeft + 's';

                return `
                    <div style="padding:0.4rem 0;border-bottom:1px solid var(--border,#2a2e3e22);">
                        <div style="display:flex;justify-content:space-between;align-items:center;">
                            <strong style="color:var(--accent,#00C9DB);">${escHtml(a.name)}.lichen</strong>
                            ${ended
                        ? '<span style="font-size:0.7rem;color:#f59e0b;">ENDED</span>'
                        : `<span style="font-size:0.7rem;opacity:0.6;">${timeLeft} left</span>`}
                        </div>
                        <div style="display:flex;justify-content:space-between;font-size:0.75rem;opacity:0.7;">
                            <span>Bid: ${highBid} LICN (reserve: ${reserve})</span>
                            <span>By: ${bidder}</span>
                        </div>
                        ${!ended ? `<button class="id-link-btn" onclick="showBidAuctionModal('${escHtml(a.name)}')" style="margin-top:0.25rem;font-size:0.75rem;"><i class="fas fa-gavel"></i> Place Bid</button>` : ''}
                    </div>
                `;
            }).join('');
        }

        // Admin create auction button
        const adminContainer = document.getElementById('adminAuctionCreate');
        if (adminContainer) {
            // Check if current wallet is admin (simple heuristic: check if the wallet deployed the contract)
            try {
                const wallet = getActiveWallet();
                const lichenidAddr = await getLichenIdProgramAddress();
                if (wallet && lichenidAddr) {
                    const accountInfo = await rpc.call('getAccount', [lichenidAddr]).catch(() => null);
                    const deployer = accountInfo?.deployer || accountInfo?.owner || '';
                    if (deployer === wallet.address) {
                        adminContainer.innerHTML = `
                            <div style="margin-top:0.5rem;padding-top:0.5rem;border-top:1px dashed var(--border,#2a2e3e);">
                                <button class="id-link-btn" onclick="showCreateAuctionModal()" style="font-size:0.75rem;"><i class="fas fa-plus"></i> Create Auction (Admin)</button>
                            </div>
                        `;
                    }
                }
            } catch (_) { }
        }
    } catch (e) {
        container.innerHTML = '<span style="opacity:0.5;color:#f87171;">Failed to load auctions</span>';
    }
}

async function showBidAuctionModal(name) {
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: `Bid on ${name}.lichen`,
        message: 'Enter your bid amount in LICN. Must exceed the current highest bid.',
        icon: 'fas fa-gavel',
        confirmText: 'Place Bid',
        requiredLicn: 1 + FEE,
        fields: [
            { id: 'amount', label: 'Bid Amount (LICN)', type: 'number', placeholder: '100', min: 1, step: 'any' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.amount) return;

    const bidAmount = parseFloat(values.amount);
    if (isNaN(bidAmount) || bidAmount <= 0) {
        showToast('Invalid bid amount');
        return;
    }

    try {
        showToast(`Placing bid of ${bidAmount} LICN on ${name}.lichen...`);
        const tx = await buildContractCall('bid_name_auction', {
            name: name,
            bid_amount: bidAmount
        }, values.password, bidAmount);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Bid failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast(`Bid placed on ${name}.lichen!`);
        await loadAuctionList();
    } catch (e) {
        showToast('Bid failed: ' + e.message);
    }
}

async function showCreateAuctionModal() {
    const FEE = typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001;
    const values = await showPasswordModal({
        title: 'Create Premium Name Auction',
        message: 'Admin only: create an auction for a premium short name (3-4 chars).',
        icon: 'fas fa-gavel',
        confirmText: 'Create Auction',
        requiredLicn: FEE,
        fields: [
            { id: 'name', label: 'Name (3-4 chars)', type: 'text', placeholder: 'eth' },
            { id: 'reserve', label: 'Reserve Bid (LICN)', type: 'number', placeholder: '100', min: 1, step: 'any' },
            { id: 'endSlots', label: 'Duration (slots, ~2/sec)', type: 'number', placeholder: '432000', min: 216000, max: 3024000, step: 1 },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ],
        onRender: (modal) => {
            const nameInput = modal.querySelector('#name');
            if (nameInput) {
                nameInput.style.textTransform = 'lowercase';
                nameInput.addEventListener('input', () => {
                    const pos = nameInput.selectionStart;
                    nameInput.value = nameInput.value.toLowerCase();
                    nameInput.setSelectionRange(pos, pos);
                });
            }
        }
    });
    if (!values || !values.password || !values.name) return;

    const name = values.name.toLowerCase().trim();
    if (name.length < 3 || name.length > 4) {
        showToast('Premium names must be 3-4 characters');
        return;
    }

    const reserveSpores = Math.floor(parseFloat(values.reserve || '100') * 1_000_000_000);
    const endSlots = parseInt(values.endSlots || '432000');

    try {
        showToast(`Creating auction for ${name}.lichen...`);
        const tx = await buildContractCall('create_name_auction', {
            name: name,
            reserve_bid: reserveSpores,
            end_slot_offset: endSlots
        }, values.password);
        const result = await rpc.sendTransaction(tx);
        if (result?.error) {
            showToast('Failed: ' + (result.error || 'unknown'));
            return;
        }
        showToast(`Auction created for ${name}.lichen!`);
        await loadAuctionList();
    } catch (e) {
        showToast('Failed: ' + e.message);
    }
}
