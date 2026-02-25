// ============================================================================
// MoltyID Identity Module — Full wallet integration
// Handles: profile, .molt names, reputation, skills, vouches, achievements,
//          agent service config, and all signed contract transactions
// ============================================================================

// ── Constants ──
const MOLTYID_PROGRAM_ADDRESS = null; // Resolved at runtime via RPC
let _moltyidAddress = null;
let _identityCache = null;
let _identityLoading = false;

const AGENT_TYPES = [
    { value: 0, label: 'Unknown',        desc: 'Unspecified or new identity' },
    { value: 1, label: 'Trading',        desc: 'Market-making, arbitrage, DeFi strategies' },
    { value: 2, label: 'Development',    desc: 'Smart contracts, tooling, protocol dev' },
    { value: 3, label: 'Analysis',       desc: 'On-chain analytics, data feeds, research' },
    { value: 4, label: 'Creative',       desc: 'Content creation, design, media' },
    { value: 5, label: 'Infrastructure', desc: 'Validators, RPCs, indexers, relayers' },
    { value: 6, label: 'Governance',     desc: 'Voting, proposals, DAO operations' },
    { value: 7, label: 'Oracle',         desc: 'External data feeds, price oracles' },
    { value: 8, label: 'Storage',        desc: 'Data persistence, archival, backups' },
    { value: 9, label: 'General',        desc: 'Multi-purpose or uncategorized agent' }
];

// ACHIEVEMENT_DEFS provided by shared/utils.js (loaded before this file)

const TRUST_TIERS = [
    { name: 'Newcomer',    min: 0,     color: '#6c7a89' },
    { name: 'Verified',    min: 100,   color: '#3498db' },
    { name: 'Trusted',     min: 500,   color: '#2ecc71' },
    { name: 'Established', min: 1000,  color: '#f1c40f' },
    { name: 'Elite',       min: 5000,  color: '#e67e22' },
    { name: 'Legendary',   min: 10000, color: '#e74c3c' },
];

const NAME_PRICING = {
    3: { cost: 500, label: '500 MOLT', auctionOnly: true },
    4: { cost: 100, label: '100 MOLT', auctionOnly: true },
    5: { cost: 20, label: '20 MOLT', auctionOnly: false },
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

function fmtMolt(shells) {
    return (Number(shells) / 1_000_000_000).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 });
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
// The MoltyID contract uses raw WASM function params (pointers + values).
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
 * Encode args for a MoltyID contract call using the WASM ABI layout descriptor.
 * callerPubkey: Uint8Array (32 bytes) — the transaction signer's public key
 * functionName: string — the contract function to call
 * params: object — the high-level params from the modal (same keys as before)
 */
function encodeMoltyIdArgs(callerPubkey, functionName, params) {
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
            // set_rate(caller_ptr:I32, molt_per_unit:I64) — default mode works
            const data = new Uint8Array(32 + 8);
            data.set(callerPubkey, 0);
            data.set(u64LE(params.molt_per_unit || 0), 32);
            return data; // no layout prefix — default handles I32 ptr + I64 val
        }
        case 'set_availability': {
            // set_availability(caller_ptr:I32, status:I32(u8))
            return buildLayoutArgs([0x20, 0x01], [
                callerPubkey,
                new Uint8Array([(params.status || 0) & 0xFF]),
            ]);
        }
        default:
            // Fallback: JSON-encode (legacy — will likely fail for ptr-param functions)
            return new TextEncoder().encode(JSON.stringify(params));
    }
}

// ── MoltyID Program Address Resolution ──
async function getMoltyIdProgramAddress() {
    if (_moltyidAddress) return _moltyidAddress;
    // Try each symbol individually (RPC expects a single string, not an array)
    const symbols = ['YID', 'yid', 'MOLTYID'];
    for (const symbol of symbols) {
        try {
            const result = await rpc.call('getSymbolRegistry', [symbol]);
            const program = result?.program || result?.address || result?.pubkey;
            if (program) { _moltyidAddress = program; return _moltyidAddress; }
        } catch (_) { continue; }
    }
    // Fallback: scan full contract list
    try {
        const contracts = await rpc.call('getAllContracts');
        if (Array.isArray(contracts)) {
            const c = contracts.find(c => c.name === 'moltyid' || c.symbol === 'YID');
            if (c) { _moltyidAddress = c.program_id || c.address; return _moltyidAddress; }
        }
    } catch (_) {}
    return null;
}

// ── Contract Call Builder ──
// Build a signed MoltyID contract call transaction
async function buildContractCall(functionName, args, password, valueMolt = 0) {
    const wallet = getActiveWallet();
    if (!wallet) throw new Error('No active wallet');

    // Pre-flight: check MOLT balance covers fee + value
    try {
        const balResult = await rpc.call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / 1_000_000_000;
        const baseFee = 0.001; // 0.001 MOLT base fee
        const totalNeeded = valueMolt + baseFee;
        if (spendable < totalNeeded) {
            throw new Error(`Insufficient MOLT: need ${totalNeeded.toLocaleString(undefined, { maximumFractionDigits: 9 })} (${valueMolt > 0 ? valueMolt + ' value + ' : ''}${baseFee} fee), have ${spendable.toLocaleString(undefined, { maximumFractionDigits: 9 })} spendable`);
        }
    } catch (e) {
        if (e.message.includes('Insufficient')) throw e;
        // Non-blocking on RPC errors
    }
    
    const moltyidAddr = await getMoltyIdProgramAddress();
    if (!moltyidAddr) throw new Error('MoltyID contract not found on network');
    
    const latestBlock = await rpc.getLatestBlock();
    const fromPubkey = MoltCrypto.hexToBytes(wallet.publicKey);
    const contractProgramId = new Uint8Array(32).fill(0xFF);
    const moltyidPubkey = bs58.decode(moltyidAddr);
    
    // Encode args as proper binary with WASM ABI layout descriptor
    const argsBytes = encodeMoltyIdArgs(fromPubkey, functionName, args);
    
    const callPayload = JSON.stringify({
        Call: {
            function: functionName,
            args: Array.from(argsBytes),
            value: Math.floor(valueMolt * 1_000_000_000)
        }
    });
    
    const message = {
        instructions: [{
            program_id: Array.from(contractProgramId),
            accounts: [Array.from(fromPubkey), Array.from(moltyidPubkey)],
            data: Array.from(new TextEncoder().encode(callPayload))
        }],
        blockhash: latestBlock.hash
    };
    
    const privateKey = await MoltCrypto.decryptPrivateKey(wallet.encryptedKey, password);
    const messageBytes = serializeMessageBincode(message);
    const signature = await MoltCrypto.signTransaction(privateKey, messageBytes);
    
    const transaction = { signatures: [Array.from(signature)], message };
    const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
    return btoa(String.fromCharCode(...txBytes));
}

// ── RPC Data Loading ──
async function loadIdentityData() {
    const wallet = getActiveWallet();
    if (!wallet) return null;
    
    try {
        const [profile, moltNameResult] = await Promise.all([
            rpc.call('getMoltyIdProfile', [wallet.address]).catch(() => null),
            rpc.call('reverseMoltName', [wallet.address]).catch(() => null),
        ]);
        // reverseMoltName returns {"name": "alice.molt"} or null — extract the string
        const moltName = moltNameResult?.name || null;
        
        let nameDetails = null;
        if (moltName) {
            try {
                nameDetails = await rpc.call('resolveMoltName', [moltName]);
            } catch (_) {}
        }
        
        _identityCache = {
            profile,
            moltName,
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
            <span>Loading MoltyID...</span>
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

// ── MoltyID Intro Banner ──
function renderIdentityBanner(compact = false) {
    if (compact) {
        return `
            <div class="id-banner id-banner-compact">
                <div class="id-banner-icon"><i class="fas fa-fingerprint"></i></div>
                <div class="id-banner-text">
                    <h3>MoltyID</h3>
                    <p>On-chain identity, .molt name, reputation &amp; agent services.</p>
                </div>
            </div>
        `;
    }
    return `
        <div class="id-banner">
            <div class="id-banner-icon"><i class="fas fa-fingerprint"></i></div>
            <div class="id-banner-text">
                <h3>MoltyID — On-Chain Identity</h3>
                <p>Your portable reputation, .molt name, skills, and agent service profile — all anchored on MoltChain.</p>
            </div>
        </div>
    `;
}

// ── No Identity — Interactive Onboarding Steps ──
function renderNoIdentity(container) {
    container.innerHTML = `
        ${renderIdentityBanner()}
        <div class="id-onboard">
            <div class="id-onboard-step id-onboard-active" onclick="showRegisterIdentityModal()">
                <div class="id-onboard-num">1</div>
                <div class="id-onboard-body">
                    <div class="id-onboard-title">Register Identity</div>
                    <div class="id-onboard-desc">Choose a display name and agent type. Free — only the 0.0001 MOLT transaction fee.</div>
                </div>
                <div class="id-onboard-arrow"><i class="fas fa-chevron-right"></i></div>
            </div>
            <div class="id-onboard-step id-onboard-locked">
                <div class="id-onboard-num"><i class="fas fa-lock" style="font-size:0.65rem;"></i></div>
                <div class="id-onboard-body">
                    <div class="id-onboard-title">Claim a .molt Name</div>
                    <div class="id-onboard-desc">Register a human-readable name (5+ chars, 20 MOLT/year). Premium 3–4 char via auction.</div>
                </div>
            </div>
            <div class="id-onboard-step id-onboard-locked">
                <div class="id-onboard-num"><i class="fas fa-lock" style="font-size:0.65rem;"></i></div>
                <div class="id-onboard-body">
                    <div class="id-onboard-title">Build Reputation</div>
                    <div class="id-onboard-desc">Earn rep through transactions, governance, vouches. Unlock trust tiers &amp; achievements.</div>
                </div>
            </div>
        </div>
    `;
}

// ── Full Identity Render ──
function renderIdentity(container, data) {
    const { profile, moltName, nameDetails } = data;
    const identity = profile.identity;
    const rep = Number(profile?.reputation?.score || identity?.reputation || 0);
    const tier = getTrustTier(rep);
    const nextTier = getNextTier(rep);
    const maxRep = 100000;
    const repPct = Math.min(100, (rep / maxRep) * 100);
    
    const agentType = getAgentTypeName(identity.agent_type);
    const agentDesc = AGENT_TYPES.find(a => a.value === Number(identity.agent_type))?.desc || '';
    const rawName = moltName || identity?.name || 'Unnamed';
    const displayName = escHtml(rawName.endsWith('.molt') ? rawName : rawName);
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
    const rateMolt = (Number(profile?.agent?.rate || 0) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 9 });
    
    container.innerHTML = `
        ${renderProfileStrip(displayName, agentType, agentDesc, tier, rep, isActive, moltName)}
        <div class="id-grid">
            ${renderRepSection(rep, tier, nextTier, repPct, maxRep)}
            ${renderNameSection(moltName, nameDetails)}
            ${renderSkillsSection(skills)}
            ${renderVouchesSection(vouchesReceived, vouchesGiven)}
            ${renderAchievementsSection(achievements, achievedIds)}
            ${renderAgentSection(endpoint, availability, rateMolt, profile?.agent?.metadata)}
        </div>
    `;
}

// ============================================================================
// SECTION RENDERERS — Compact grid layout
// ============================================================================

// ── Profile Strip (top bar, always visible when has identity) ──
function renderProfileStrip(displayName, agentType, agentDesc, tier, rep, isActive, moltName) {
    const moltDisplay = moltName ? escHtml((moltName.endsWith('.molt') ? moltName : moltName + '.molt')) : null;
    return `
        <div class="id-profile-strip">
            <div class="id-strip-avatar" style="background:${tier.color}18; border-color:${tier.color};">
                <i class="fas fa-fingerprint" style="color:${tier.color};"></i>
            </div>
            <div class="id-strip-info">
                <div class="id-strip-name">
                    ${escHtml(displayName)}
                    ${moltDisplay ? `<span class="id-strip-molt">${moltDisplay}</span>` : ''}
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
function renderNameSection(moltName, nameDetails) {
    if (!moltName) {
        return `
            <div class="id-section">
                <div class="id-section-head">
                    <span><i class="fas fa-at"></i> .molt Name</span>
                </div>
                <div class="id-section-body id-section-empty">
                    <p>No name registered</p>
                    <small>5+ chars from 20 MOLT/yr</small>
                    <button class="btn btn-small btn-primary" style="margin-top:0.5rem;" onclick="showRegisterNameModal()">
                        <i class="fas fa-plus"></i> Register
                    </button>
                </div>
            </div>
        `;
    }

    const name = moltName.endsWith('.molt') ? moltName : moltName + '.molt';
    const expiry = nameDetails?.expiry_slot;
    const registered = nameDetails?.registered_slot || 0;
    const expiryDisplay = expiry ? formatSlotExpiry(expiry, registered) : '—';

    return `
        <div class="id-section">
            <div class="id-section-head">
                <span><i class="fas fa-at"></i> .molt Name</span>
            </div>
            <div class="id-section-body">
                <div class="id-name-display">${escHtml(name)}</div>
                <div class="id-name-expiry">Expires ${expiryDisplay}</div>
                <div class="id-section-actions">
                    <button class="id-link-btn" onclick="showRenewNameModal()"><i class="fas fa-redo"></i> Renew</button>
                    <button class="id-link-btn" onclick="showTransferNameModal()"><i class="fas fa-exchange-alt"></i> Transfer</button>
                    <button class="id-link-btn id-link-danger" onclick="showReleaseNameModal()"><i class="fas fa-trash-alt"></i> Release</button>
                </div>
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
            const label = v.voucher_name ? escHtml(v.voucher_name) + '.molt' : fmtAddr(v.voucher, 8);
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
        <div class="id-section">
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
function renderAgentSection(endpoint, availability, rateMolt, metadata) {
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
                        <span class="id-kv-value">${rateMolt} MOLT/req</span>
                    </div>
                </div>
            </div>
        </div>
    `;
}

// ============================================================================
// SLOT EXPIRY FORMATTER (same logic as explorer)
// ============================================================================
function formatSlotExpiry(expirySlot, registeredSlot) {
    const SLOTS_PER_SEC = 2;
    const now = Date.now() / 1000;
    const currentSlot = Math.floor(now * SLOTS_PER_SEC);
    const secsUntil = (expirySlot - currentSlot) / SLOTS_PER_SEC;
    const expiryDate = new Date((now + secsUntil) * 1000);
    const monthYear = expiryDate.toLocaleString(undefined, { month: 'short', year: 'numeric' });
    
    const totalSlots = expirySlot - (registeredSlot || 0);
    const years = Math.round(totalSlots / 63_072_000);
    return `${monthYear} (~${years}yr)`;
}

// ============================================================================
// MODALS — Identity Actions
// ============================================================================

// ── Register Identity ──
async function showRegisterIdentityModal() {
    const values = await showPasswordModal({
        title: 'Register MoltyID',
        message: 'Create your on-chain identity. Choose a display name and agent type.<br><small style="color:var(--text-muted);">This is free — only the 0.0001 MOLT transaction fee applies.</small>',
        icon: 'fas fa-fingerprint',
        confirmText: 'Register',
        fields: [
            { id: 'displayName', label: 'Display Name', type: 'text', placeholder: 'e.g. CryptoBuilder' },
            { id: 'agentType', label: 'Agent Type', type: 'select',
              options: AGENT_TYPES.map(t => ({ value: t.value, label: `${t.label} — ${t.desc}` })) },
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
        // Retry loading identity — the tx may take 1-3 blocks to be indexed
        await retryLoadIdentity(8, 2000);
    } catch (e) {
        showToast('Registration failed: ' + e.message);
    }
}

// ── Edit Profile (Agent Type) ──
async function showEditProfileModal() {
    const current = _identityCache?.profile?.identity?.agent_type || 9;
    const values = await showPasswordModal({
        title: 'Update Agent Type',
        message: 'Change your agent classification.',
        icon: 'fas fa-id-badge',
        confirmText: 'Update',
        fields: [
            { id: 'agentType', label: 'Agent Type', type: 'select',
              options: AGENT_TYPES.map(t => ({ value: t.value, label: `${t.label} — ${t.desc}`, selected: t.value === current })) },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;
    
    try {
        showToast('Updating agent type...');
        const tx = await buildContractCall('update_agent_type', { agent_type: parseInt(values.agentType) }, values.password);
        await rpc.sendTransaction(tx);
        showToast('Agent type updated!');
        _identityCache = null;
        await loadIdentity();
    } catch (e) {
        showToast('Update failed: ' + e.message);
    }
}

// ── Register .molt Name ──
async function showRegisterNameModal() {
    const values = await showPasswordModal({
        title: 'Register .molt Name',
        message: `
            <div class="id-pricing-table">
                <div class="id-pricing-row"><span>5+ chars</span><span>20 MOLT/year</span></div>
                <div class="id-pricing-row id-pricing-muted"><span>4 chars</span><span>100 MOLT/year <em>(auction only)</em></span></div>
                <div class="id-pricing-row id-pricing-muted"><span>3 chars</span><span>500 MOLT/year <em>(auction only)</em></span></div>
            </div>
            <small>Names are lowercase, 3-32 chars (a-z, 0-9, hyphens). Duration: 1-10 years.<br>Premium short names (3-4 chars) can only be acquired through the auction system.</small>
        `,
        icon: 'fas fa-at',
        confirmText: 'Register Name',
        fields: [
            { id: 'name', label: 'Name (without .molt)', type: 'text', placeholder: 'myname' },
            { id: 'duration', label: 'Duration (years)', type: 'number', placeholder: '1' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.name) return;
    
    const name = values.name.toLowerCase().replace(/\.molt$/, '');
    const duration = Math.max(1, Math.min(10, parseInt(values.duration) || 1));
    const costPerYear = getNameCostPerYear(name.length);
    const totalCost = costPerYear * duration;
    
    // Validate name format
    if (name.length < 3 || name.length > 32 || !/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(name)) {
        showToast('Invalid name: 3-32 chars, a-z 0-9 hyphens, no leading/trailing hyphens');
        return;
    }
    
    // Premium short names are auction-only
    if (name.length <= 4) {
        showToast(`${name.length}-char names are premium and can only be acquired through auctions`);
        return;
    }
    
    try {
        showToast(`Registering ${name}.molt for ${duration}yr (${totalCost} MOLT)...`);
        const tx = await buildContractCall('register_name', {
            name: name,
            duration_years: duration
        }, values.password, totalCost);
        await rpc.sendTransaction(tx);
        showToast(`${name}.molt registered!`);
        _identityCache = null;
        await retryLoadIdentity(5, 1200);
    } catch (e) {
        showToast('Name registration failed: ' + e.message);
    }
}

// ── Renew .molt Name ──
async function showRenewNameModal() {
    const currentName = _identityCache?.moltName;
    if (!currentName) { showToast('No name to renew'); return; }
    
    const name = currentName.replace(/\.molt$/, '');
    const costPerYear = getNameCostPerYear(name.length);
    
    const values = await showPasswordModal({
        title: `Renew ${name}.molt`,
        message: `Cost: ${costPerYear} MOLT per additional year.`,
        icon: 'fas fa-redo',
        confirmText: 'Renew Name',
        fields: [
            { id: 'years', label: 'Additional Years', type: 'number', placeholder: '1' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;
    
    const years = Math.max(1, Math.min(10, parseInt(values.years) || 1));
    const totalCost = costPerYear * years;
    
    try {
        showToast(`Renewing for ${years}yr (${totalCost} MOLT)...`);
        const tx = await buildContractCall('renew_name', {
            name: name,
            additional_years: years
        }, values.password, totalCost);
        await rpc.sendTransaction(tx);
        showToast('Name renewed!');
        _identityCache = null;
        await loadIdentity();
    } catch (e) {
        showToast('Renewal failed: ' + e.message);
    }
}

// ── Transfer .molt Name ──
async function showTransferNameModal() {
    const currentName = _identityCache?.moltName;
    if (!currentName) { showToast('No name to transfer'); return; }
    
    const name = currentName.replace(/\.molt$/, '');
    
    const values = await showPasswordModal({
        title: `Transfer ${name}.molt`,
        message: 'Transfer ownership to another address. This is irreversible.',
        icon: 'fas fa-exchange-alt',
        confirmText: 'Transfer Name',
        fields: [
            { id: 'recipient', label: 'Recipient Address', type: 'text', placeholder: 'Base58 address' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.recipient) return;
    
    // AUDIT-FIX W-6: Validate recipient address format before building tx
    if (!MoltCrypto.isValidAddress(values.recipient)) {
        showToast('Invalid recipient address — must be a valid Base58 address');
        return;
    }
    
    try {
        showToast('Transferring name...');
        const tx = await buildContractCall('transfer_name', {
            name: name,
            new_owner: values.recipient
        }, values.password);
        await rpc.sendTransaction(tx);
        showToast('Name transferred!');
        _identityCache = null;
        await loadIdentity();
    } catch (e) {
        showToast('Transfer failed: ' + e.message);
    }
}

// ── Release .molt Name ──
async function showReleaseNameModal() {
    const currentName = _identityCache?.moltName;
    if (!currentName) { showToast('No name to release'); return; }
    
    const name = currentName.replace(/\.molt$/, '');
    
    const confirmed = await showConfirmModal({
        title: `Release ${name}.molt?`,
        message: 'This will permanently release your .molt name. It can be re-registered by anyone. This action cannot be undone.',
        icon: 'fas fa-exclamation-triangle',
        confirmText: 'Release Name',
        cancelText: 'Keep Name',
        danger: true
    });
    if (!confirmed) return;
    
    const values = await showPasswordModal({
        title: 'Confirm Release',
        message: `You are about to release <strong>${escHtml(name)}.molt</strong>.`,
        icon: 'fas fa-trash-alt',
        confirmText: 'Sign & Release',
        fields: [
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password) return;
    
    try {
        showToast('Releasing name...');
        const tx = await buildContractCall('release_name', { name: name }, values.password);
        await rpc.sendTransaction(tx);
        showToast('Name released');
        _identityCache = null;
        await loadIdentity();
    } catch (e) {
        showToast('Release failed: ' + e.message);
    }
}

// ── Add Skill ──
async function showAddSkillModal() {
    const values = await showPasswordModal({
        title: 'Add Skill',
        message: 'Add a skill to your identity profile.',
        icon: 'fas fa-tools',
        confirmText: 'Add Skill',
        fields: [
            { id: 'skillName', label: 'Skill Name', type: 'text', placeholder: 'e.g. Rust, Trading, Security' },
            { id: 'proficiency', label: 'Proficiency (1-100)', type: 'number', placeholder: '50' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.skillName) return;
    
    const proficiency = Math.max(1, Math.min(100, parseInt(values.proficiency) || 50));
    
    try {
        showToast('Adding skill...');
        const tx = await buildContractCall('add_skill', {
            name: values.skillName,
            proficiency: proficiency
        }, values.password);
        await rpc.sendTransaction(tx);
        showToast('Skill added!');
        _identityCache = null;
        await loadIdentity();
    } catch (e) {
        showToast('Failed: ' + e.message);
    }
}

// ── Vouch for Someone ──
async function showVouchModal() {
    const values = await showPasswordModal({
        title: 'Vouch for Identity',
        message: 'Vouch for another MoltyID holder. Both parties must have registered identities.',
        icon: 'fas fa-handshake',
        confirmText: 'Vouch',
        fields: [
            { id: 'vouchee', label: 'Address to Vouch For', type: 'text', placeholder: 'Base58 address' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
        ]
    });
    if (!values || !values.password || !values.vouchee) return;
    
    // AUDIT-FIX W-6: Validate vouchee address format before building tx
    if (!MoltCrypto.isValidAddress(values.vouchee)) {
        showToast('Invalid address — must be a valid Base58 address');
        return;
    }
    
    try {
        showToast('Sending vouch...');
        const tx = await buildContractCall('vouch', { vouchee: values.vouchee }, values.password);
        await rpc.sendTransaction(tx);
        showToast('Vouch sent!');
        _identityCache = null;
        await loadIdentity();
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
    
    const values = await showPasswordModal({
        title: 'Agent Service Configuration',
        message: 'Configure how other agents discover and interact with your identity.',
        icon: 'fas fa-satellite-dish',
        confirmText: 'Save Changes',
        fields: [
            { id: 'endpoint', label: 'Service Endpoint URL', type: 'text', placeholder: 'https://api.example.com/agent', value: currentEndpoint },
            { id: 'rate', label: 'Rate (MOLT per request)', type: 'number', placeholder: '0.001', value: currentRate },
            { id: 'availability', label: 'Availability', type: 'select',
              options: [
                  { value: 'online', label: 'Online', selected: currentAvailability === 'online' },
                  { value: 'offline', label: 'Offline', selected: currentAvailability !== 'online' }
              ] },
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
                await rpc.sendTransaction(tx);
            });
        }
        
        // Update rate if changed
        const newRateMolt = parseFloat(values.rate || '0');
        const newRateShells = Math.floor(newRateMolt * 1_000_000_000);
        const oldRateShells = Number(agent.rate || 0);
        if (newRateShells !== oldRateShells) {
            tasks.push(async () => {
                const tx = await buildContractCall('set_rate', { molt_per_unit: newRateShells }, values.password);
                await rpc.sendTransaction(tx);
            });
        }
        
        // Update availability if changed
        const newAvailNum = values.availability === 'online' ? 1 : 0;
        const oldAvailNum = currentAvailability === 'online' ? 1 : 0;
        if (newAvailNum !== oldAvailNum) {
            tasks.push(async () => {
                const tx = await buildContractCall('set_availability', { status: newAvailNum }, values.password);
                await rpc.sendTransaction(tx);
            });
        }
        
        if (tasks.length === 0) {
            showToast('No changes to save');
            return;
        }
        
        showToast('Saving agent configuration...');
        // Execute sequentially (each needs fresh blockhash)
        for (const task of tasks) {
            await task();
        }
        showToast('Agent service updated!');
        _identityCache = null;
        await loadIdentity();
    } catch (e) {
        showToast('Update failed: ' + e.message);
    }
}

