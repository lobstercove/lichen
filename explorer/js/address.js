// MoltChain Explorer - Address Detail Page
// Displays detailed information about a specific address/account
// NOTE: bs58decode, formatHash, formatAddress, escapeHtml, copyToClipboard,
//       safeCopy, getMoltRpcUrl are provided by shared/utils.js

let currentAddress = null;
let txNextCursor = null;    // before_slot for next page
let txCursorStack = [];     // stack for previous pages
let txCurrentBeforeSlot = undefined;
const TX_PAGE_SIZE = 50;
let activeAddressTab = 'overview';
let currentMoltyProfile = null;
let currentAccountData = null;
let moltyIdProgramAddress = null;

const AGENT_TYPE_OPTIONS = [
    { value: 0, label: 'General ⚡' },
    { value: 1, label: 'Trading 📈' },
    { value: 2, label: 'Development 💻' },
    { value: 3, label: 'Analysis 🔬' },
    { value: 4, label: 'Creative 🎨' },
    { value: 5, label: 'Infrastructure 🏗' },
    { value: 6, label: 'Governance 🏛' },
    { value: 7, label: 'Oracle 🔮' },
    { value: 8, label: 'Storage 💾' }
];

function isSystemProgramOwner(owner) {
    const SYS = typeof SYSTEM_PROGRAM_ID !== 'undefined'
        ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111';
    return owner === 'SystemProgram11111111111111111111111111'
        || owner === '11111111111111111111111111111111'
        || owner === SYS;
}

// ===== MoltChain Address to EVM Conversion =====
function moltchainToEvmAddress(base58Pubkey) {
    try {
        if (!base58Pubkey || base58Pubkey.trim() === '' ||
            base58Pubkey === '11111111111111111111111111111111') {
            return '0x' + '0'.repeat(40);
        }
        if (typeof bs58decode === 'undefined') return null;
        if (typeof keccak_256 === 'undefined') return null;
        const pubkeyBytes = bs58decode(base58Pubkey);
        const hashHex = keccak_256(pubkeyBytes);
        return '0x' + hashHex.slice(-40);
    } catch (error) {
        console.error('Error converting address to EVM:', error);
        return null;
    }
}

// ===== RPC helper =====
// Delegates to the shared MoltChainRPC instance from explorer.js.
// Falls back to direct fetch if rpc is unavailable (standalone testing).
async function rpcCall(method, params = []) {
    if (typeof rpc !== 'undefined' && rpc && typeof rpc.call === 'function') {
        return rpc.call(method, params);
    }
    const response = await fetch(getMoltRpcUrl(), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: Date.now(), method, params })
    });
    const data = await response.json();
    if (data.error) throw new Error(data.error.message);
    return data.result;
}

// ===== Initialize =====
document.addEventListener('DOMContentLoaded', () => {
    const urlParams = new URLSearchParams(window.location.search);
    currentAddress = urlParams.get('address') || urlParams.get('addr');
    const initialTab = (urlParams.get('tab') || '').toLowerCase();
    if (!currentAddress) { showError('No address provided'); return; }
    setTokensTabVisibility(true);
    setStakingTabVisibility(false);
    setupAddressTabs();
    if (['overview', 'tokens', 'identity', 'staking', 'transactions', 'data'].includes(initialTab)) {
        switchAddressTab(initialTab);
    }
    loadAddressData();
    setupSearch();
});

// ===== Load Address Data =====
async function loadAddressData() {
    try {
        let accountData;
        try {
            accountData = await fetchAccountFromRPC(currentAddress);
            if (!accountData) accountData = createEmptyAccountData(currentAddress);
        } catch (e) {
            console.warn('RPC not available:', e);
            accountData = createEmptyAccountData(currentAddress);
        }
        currentAccountData = accountData;
        await applyValidatorType(accountData);
        displayAddressData(accountData);
        await loadMoltyIdentityData(currentAddress);

        // If it's a validator, load staking rewards
        const accountTypeLabel = String(accountData.type || '').toLowerCase();
        if (accountTypeLabel.includes('validator')) {
            const hasStakingData = await loadValidatorRewards(currentAddress);
            setStakingTabVisibility(hasStakingData);
        } else {
            setStakingTabVisibility(false);
        }

        // If it's a treasury, load treasury stats
        if (accountData.type === 'Treasury') {
            loadTreasuryStats(currentAddress);
        }

        if (accountData.executable) {
            await loadRegistryInfo(accountData.base58);
            await loadContractAbi(accountData.base58);
        } else {
            clearRegistryInfo();
            hideContractAbi();
        }
        loadTransactionHistory(currentAddress);
    } catch (error) {
        console.error('Error loading address:', error);
        showError('Failed to load address data');
    }
}

// ===== Tabs & Actions =====
function setupAddressTabs() {
    document.querySelectorAll('.address-tab').forEach(button => {
        button.addEventListener('click', () => switchAddressTab(button.dataset.tab));
    });
}

function switchAddressTab(tabName) {
    activeAddressTab = tabName;
    document.querySelectorAll('.address-tab').forEach(button => {
        button.classList.toggle('active', button.dataset.tab === tabName);
    });
    document.querySelectorAll('.address-pane').forEach(pane => {
        pane.classList.toggle('active', pane.dataset.pane === tabName);
    });
}

function setStakingTabVisibility(visible) {
    const stakingTab = document.querySelector('.address-tab[data-tab="staking"]');
    const stakingPane = document.querySelector('.address-pane[data-pane="staking"]');
    const shouldShow = !!visible;

    if (stakingTab) {
        stakingTab.style.display = shouldShow ? '' : 'none';
        stakingTab.setAttribute('aria-hidden', shouldShow ? 'false' : 'true');
        if (!shouldShow) stakingTab.classList.remove('active');
    }

    if (stakingPane) {
        stakingPane.style.display = shouldShow ? '' : 'none';
        if (!shouldShow) stakingPane.classList.remove('active');
    }

    if (!shouldShow && activeAddressTab === 'staking') {
        switchAddressTab('overview');
    }
}

function setTokensTabVisibility(visible) {
    const tokensTab = document.querySelector('.address-tab[data-tab="tokens"]');
    const tokensPane = document.querySelector('.address-pane[data-pane="tokens"]');
    const shouldShow = !!visible;

    if (tokensTab) {
        tokensTab.style.display = shouldShow ? '' : 'none';
        tokensTab.setAttribute('aria-hidden', shouldShow ? 'false' : 'true');
        if (!shouldShow) tokensTab.classList.remove('active');
    }

    if (tokensPane) {
        tokensPane.style.display = shouldShow ? '' : 'none';
        if (!shouldShow) tokensPane.classList.remove('active');
    }

    if (!shouldShow && activeAddressTab === 'tokens') {
        switchAddressTab('overview');
    }
}

function enforceAddressViewOnlyMode() {
    document.querySelectorAll('.address-summary-actions, .address-receive-box, .identity-actions').forEach((el) => {
        el.remove();
    });
    document.querySelectorAll('[data-identity-action], #registerIdentityBtn').forEach((el) => {
        el.remove();
    });
}

// ===== MoltyID =====
async function loadMoltyIdentityData(address) {
    try {
        const [profileResult, nameResult] = await Promise.all([
            rpcCall('getMoltyIdProfile', [address]).catch(() => null),
            rpcCall('reverseMoltName', [address]).catch(() => null)
        ]);

        const profile = profileResult || {};
        // RPC returns identity fields at the top level of the profile, not nested
        const identity = profile.identity || (profile.name || profile.molt_name ? profile : null);
        const reputation = profile.reputation || {};
        const skills = Array.isArray(profile.skills) ? profile.skills : [];
        const vouches = profile.vouches || { received: [], given: [] };
        const achievements = Array.isArray(profile.achievements) ? profile.achievements : [];
        const moltName = typeof nameResult === 'string'
            ? nameResult
            : (nameResult?.name || identity?.name || identity?.molt_name || null);

        let nameDetails = null;
        if (moltName) {
            nameDetails = await rpcCall('resolveMoltName', [moltName]).catch(() => null);
        }

        currentMoltyProfile = {
            identity,
            reputation,
            skills,
            vouches,
            achievements,
            agent: profile.agent || {},
            contributions: profile.contributions || {},
            moltName,
            nameDetails
        };
        renderSummaryIdentity(currentMoltyProfile);
        renderIdentityPane(currentMoltyProfile);
    } catch (error) {
        console.warn('Failed to load MoltyID data:', error);
        currentMoltyProfile = null;
        renderSummaryIdentity(null);
        renderIdentityPane(null);
    }
}

function getTrustTierLabel(reputation) {
    const score = Number(reputation) || 0;
    if (score >= 950) return 'Legendary';
    if (score >= 800) return 'Elite';
    if (score >= 600) return 'Established';
    if (score >= 400) return 'Trusted';
    if (score >= 200) return 'Verified';
    if (score >= 100) return 'Newcomer';
    return 'Probation';
}

function renderSummaryIdentity(profile) {
    const displayNameEl = document.getElementById('displayName');
    const tierBadgeEl = document.getElementById('trustTierBadge');
    if (!displayNameEl || !tierBadgeEl) return;

    const hasIdentity = !!profile?.identity;
    const reputationScore = Number(profile?.reputation?.score || profile?.reputation?.reputation || profile?.identity?.reputation || 0);
    const name = profile?.moltName || profile?.identity?.display_name || profile?.identity?.name;
    const displayMoltName = name ? (name.endsWith('.molt') ? name : `${name}.molt`) : null;

    displayNameEl.textContent = hasIdentity
        ? (displayMoltName || 'Identity Registered')
        : 'No MoltyID Identity';

    if (hasIdentity) {
        const tierLabel = getTrustTierLabel(reputationScore);
        tierBadgeEl.textContent = `${tierLabel} • ${formatNumber(reputationScore)} rep`;
        tierBadgeEl.className = 'badge success';
    } else {
        tierBadgeEl.textContent = 'No Identity';
        tierBadgeEl.className = 'badge';
    }
}

function renderIdentityPane(profile) {
    const content = document.getElementById('identityContent');
    if (!content) return;

    const identity = profile?.identity;
    if (!identity) {
        content.innerHTML = `
            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-id-badge"></i> Identity</h2>
                </div>
                <div class="detail-card-body">
                    <div class="empty-state">
                        <i class="fas fa-user-plus"></i>
                        <div>No MoltyID Found</div>
                        <small style="color: var(--text-muted); font-size: 0.9rem; display:block; margin-top:0.5rem;">
                            Register your identity to unlock .molt names, reputation, skills, and agent discovery.
                        </small>
                    </div>
                </div>
            </div>
        `;
        enforceAddressViewOnlyMode();
        return;
    }

    const reputationScore = Number(profile?.reputation?.score || profile?.reputation?.reputation || identity?.reputation || 0);
    const reputationTierName = profile?.reputation?.tier_name || getTrustTierLabel(reputationScore);
    const reputationTier = Number(profile?.reputation?.tier ?? trustTierNumber(reputationScore));
    const tier = getTrustTierLabel(reputationScore);
    const skills = Array.isArray(profile?.skills) ? profile.skills : [];
    const achievements = Array.isArray(profile?.achievements) ? profile.achievements : [];
    const vouchesReceived = Array.isArray(profile?.vouches?.received) ? profile.vouches.received : [];
    const vouchesGiven = Array.isArray(profile?.vouches?.given) ? profile.vouches.given : [];
    const maxRep = 10000;
    const repPct = Math.max(0, Math.min(100, (reputationScore / maxRep) * 100));
    const nextTierInfo = getNextTierInfo(reputationScore);
    const ladder = trustTierLadder(reputationTier);
    const registeredAt = formatTimestamp(identity.created_at || profile.created_at);
    const updatedAt = formatTimestamp(identity.updated_at || profile.updated_at);
    const rawName = profile?.moltName || identity?.name || 'Unnamed';
    const displayName = escapeHtml(rawName.endsWith('.molt') ? rawName : rawName + '.molt');
    const agentType = escapeHtml(String(identity.agent_type_name || identity.agent_type || 'Unknown'));
    const availability = escapeHtml(String(profile?.agent?.availability_name || 'offline'));
    const rateMolt = (Number(profile?.agent?.rate || 0) / 1_000_000_000).toFixed(6);
    const metadataText = escapeHtml(JSON.stringify(profile?.agent?.metadata || {}, null, 2));

    const contributions = profile?.contributions || {};
    const contributionItems = [
        ['Successful Txs', Number(contributions.successful_txs || 0)],
        ['Governance Votes', Number(contributions.governance_votes || 0)],
        ['Programs Deployed', Number(contributions.programs_deployed || 0)],
        ['Uptime Hours', Number(contributions.uptime_hours || 0)],
        ['Peer Endorsements', Number(contributions.peer_endorsements || 0)],
        ['Failed Txs', Number(contributions.failed_txs || 0)],
        ['Slashing Events', Number(contributions.slashing_events || 0)]
    ];

    const skillsHtml = skills.length
        ? skills.slice(0, 12).map(skill => {
            const skillName = escapeHtml(String(skill.name || skill.skill || 'Unnamed'));
            const proficiency = Number(skill.proficiency || skill.level || 0);
            const level = Math.max(0, Math.min(5, Math.round(proficiency / 20) || proficiency));
            const attCount = Number(skill.attestation_count || skill.attestations || 0);
            const pct = Math.max(0, Math.min(100, (level / 5) * 100));
            return `
                <div class="skill-card">
                    <div class="skill-name">${skillName}</div>
                    <div class="skill-meta">Lvl ${level}/5 • ${attCount} attestations</div>
                    <div class="skill-progress"><span style="width:${pct}%;"></span></div>
                    <div class="address-inline-actions">
                        <button class="btn btn-secondary btn-small" data-identity-action="attest-skill" data-skill="${skillName}">Attest</button>
                    </div>
                </div>
            `;
        }).join('')
        : `<div class="empty-state"><i class="fas fa-tools"></i><div>No skills listed</div></div>`;

    const vouchReceivedHtml = vouchesReceived.length
        ? vouchesReceived.slice(0, 20).map(v => {
            const name = v.voucher_name ? `${escapeHtml(v.voucher_name)}.molt` : formatAddress(v.voucher);
            return `<span class="identity-chip"><strong>${name}</strong></span>`;
        }).join('')
        : `<span class="identity-chip">No received vouches</span>`;

    const vouchGivenHtml = vouchesGiven.length
        ? vouchesGiven.slice(0, 20).map(v => {
            const name = v.vouchee_name ? `${escapeHtml(v.vouchee_name)}.molt` : formatAddress(v.vouchee);
            return `<span class="identity-chip"><strong>${name}</strong></span>`;
        }).join('')
        : `<span class="identity-chip">No given vouches</span>`;

    const achievedIds = new Set(achievements.map(a => Number(a.id)).filter(Boolean));
    const achievedHtml = achievements.length
        ? achievements.map(a => `<span class="badge success"><i class="fas fa-trophy" style="margin-right:4px;font-size:0.7em;"></i>${escapeHtml(String(a.name || `Achievement #${a.id}`))}</span>`).join(' ')
        : `<span class="identity-chip">No achievements yet</span>`;
    // Real achievement definitions from MoltyID contract
    const allAchievements = [
        { id: 1, name: 'First Transaction', icon: 'fas fa-exchange-alt' },
        { id: 2, name: 'Governance Voter', icon: 'fas fa-vote-yea' },
        { id: 3, name: 'Builder', icon: 'fas fa-code' },
        { id: 4, name: 'Trusted (500+ rep)', icon: 'fas fa-shield-alt' },
        { id: 5, name: 'Veteran (1,000+ rep)', icon: 'fas fa-medal' },
        { id: 6, name: 'Legend (5,000+ rep)', icon: 'fas fa-crown' },
        { id: 7, name: 'Endorsed (10+ vouches)', icon: 'fas fa-handshake' },
        { id: 8, name: 'Graduated', icon: 'fas fa-graduation-cap' },
    ];
    const lockedHtml = allAchievements
        .filter(a => !achievedIds.has(a.id))
        .map(a => `<span class="identity-chip" style="opacity:0.5;"><i class="${a.icon}" style="font-size:0.7em;margin-right:4px;"></i>${a.name}</span>`)
        .join(' ');

    const primaryName = profile?.moltName
        ? escapeHtml(profile.moltName.endsWith('.molt') ? profile.moltName : profile.moltName + '.molt')
        : displayName;
    const expirySlot = profile?.nameDetails?.expiry_slot;
    const registeredSlot = profile?.nameDetails?.registered_slot || 0;
    const expiryDisplay = expirySlot ? formatSlotExpiry(expirySlot, registeredSlot) : 'Unknown';
    const hasName = !!profile?.moltName;
    const namesHtml = hasName
        ? `<div class="identity-kv-grid">
        <div class="identity-kv">
            <div class="identity-kv-label">Registered Name</div>
            <div class="identity-kv-value">${primaryName}</div>
        </div>
        <div class="identity-kv">
            <div class="identity-kv-label">Expiry</div>
            <div class="identity-kv-value">${expiryDisplay}</div>
        </div>
    </div>
    <small style="color:var(--text-muted);display:block;margin-top:0.5rem;">Each address holds one .molt name. Names can be released and re-registered. Registration: 1–10 years.</small>`
        : `<div class="empty-state" style="padding:1rem;"><i class="fas fa-at"></i><div>No .molt name registered</div>
    <small style="color:var(--text-muted);margin-top:0.3rem;display:block;">Each address can hold one .molt name (1–10 year registration).</small></div>`;

    content.innerHTML = `
        <div class="identity-card-stack">
            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-id-badge"></i> Profile</h2>
                </div>
                <div class="detail-card-body">
                    <div class="identity-header-row">
                        <div style="flex:1;">
                            <div class="identity-title">${displayName}</div>
                            <div class="identity-kv-grid" style="margin-top:0.75rem;">
                                <div class="identity-kv">
                                    <div class="identity-kv-label">Agent Type</div>
                                    <div class="identity-kv-value"><span class="pill">${agentType}</span></div>
                                </div>
                                <div class="identity-kv">
                                    <div class="identity-kv-label">Trust Tier</div>
                                    <div class="identity-kv-value"><span class="badge success">${reputationTierName}</span> <span style="color:var(--text-muted);font-size:0.85rem;">${formatNumber(reputationScore)} rep</span></div>
                                </div>
                                <div class="identity-kv">
                                    <div class="identity-kv-label">Registered</div>
                                    <div class="identity-kv-value">${registeredAt}</div>
                                </div>
                                <div class="identity-kv">
                                    <div class="identity-kv-label">Last Updated</div>
                                    <div class="identity-kv-value">${updatedAt}</div>
                                </div>
                                <div class="identity-kv">
                                    <div class="identity-kv-label">Status</div>
                                    <div class="identity-kv-value">${identity.is_active
                                        ? '<span class="badge success"><i class="fas fa-check-circle" style="margin-right:4px;"></i>Active</span>'
                                        : '<span class="badge">Inactive</span>'}</div>
                                </div>
                                <div class="identity-kv">
                                    <div class="identity-kv-label">Availability</div>
                                    <div class="identity-kv-value">${availability === 'online'
                                        ? '<span class="badge success"><i class="fas fa-circle" style="font-size:0.5em;vertical-align:middle;margin-right:4px;"></i>Online</span>'
                                        : '<span class="pill" style="opacity:0.7;"><i class="fas fa-circle" style="font-size:0.5em;vertical-align:middle;margin-right:4px;"></i>Registered</span>'}</div>
                                </div>
                            </div>
                        </div>
                        <div class="identity-actions">
                            <button class="btn btn-secondary" data-identity-action="edit-profile">Edit Profile</button>
                            <button class="btn btn-secondary" data-identity-action="set-endpoint">Set Endpoint</button>
                            <button class="btn btn-secondary" data-identity-action="set-availability">Set Availability</button>
                        </div>
                    </div>
                </div>
            </div>

            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-chart-line"></i> Reputation</h2>
                </div>
                <div class="detail-card-body">
                    <div class="identity-rep-score">${formatNumber(reputationScore)} <span style="font-size:0.7em;color:var(--text-muted);font-weight:400;">/ ${formatNumber(maxRep)} (Legendary)</span></div>
                    <div class="identity-progress-wrap"><div class="identity-progress-bar" style="width:${repPct}%;"></div></div>
                    <div class="identity-tier-ladder">Trust Tier: ${ladder}</div>
                    <div class="identity-meta-line">${nextTierInfo}</div>
                    <div class="identity-kv-grid" style="margin-top:1rem;">
                        ${contributionItems.map(([label, value]) => `
                            <div class="identity-kv">
                                <div class="identity-kv-label">${label}</div>
                                <div class="identity-kv-value">${formatNumber(value)}</div>
                            </div>
                        `).join('')}
                    </div>
                </div>
            </div>

            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-tools"></i> Skills & Attestations</h2>
                </div>
                <div class="detail-card-body">
                    <div class="skill-grid">${skillsHtml}</div>
                    <div class="identity-actions" style="margin-top:1rem;">
                        <button class="btn btn-secondary" data-identity-action="attest-skill">Attest Skill</button>
                    </div>
                </div>
            </div>

            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-handshake"></i> Vouches</h2>
                </div>
                <div class="detail-card-body">
                    <div class="identity-kv-label">Vouched By (${vouchesReceived.length})</div>
                    <div class="identity-chip-list" style="margin-bottom:0.9rem;">${vouchReceivedHtml}</div>
                    <div class="identity-kv-label">Vouched For (${vouchesGiven.length})</div>
                    <div class="identity-chip-list">${vouchGivenHtml}</div>
                    <div class="identity-actions" style="margin-top:1rem;">
                        <button class="btn btn-secondary" data-identity-action="vouch">Vouch for this identity</button>
                    </div>
                </div>
            </div>

            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-award"></i> Achievements</h2>
                </div>
                <div class="detail-card-body">
                    <div class="identity-kv-label">Earned</div>
                    <div class="identity-chip-list" style="margin-bottom:0.9rem;">${achievedHtml}</div>
                    <div class="identity-kv-label">Locked</div>
                    <div class="identity-chip-list">${lockedHtml || '<span class="identity-chip">All unlocked</span>'}</div>
                </div>
            </div>

            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-at"></i> .molt Names</h2>
                </div>
                <div class="detail-card-body">
                    ${namesHtml}
                </div>
            </div>

            <div class="detail-card">
                <div class="detail-card-header">
                    <h2><i class="fas fa-satellite-dish"></i> Agent Service Directory</h2>
                    <small style="color:var(--text-muted);font-weight:400;">How other agents and services discover and interact with this identity on-chain.</small>
                </div>
                <div class="detail-card-body">
                    <div class="identity-kv-grid">
                        <div class="identity-kv">
                            <div class="identity-kv-label">Endpoint</div>
                            <div class="identity-kv-value">${escapeHtml(profile?.agent?.endpoint || 'Not set')}</div>
                        </div>
                        <div class="identity-kv">
                            <div class="identity-kv-label">Availability</div>
                            <div class="identity-kv-value">${availability}</div>
                        </div>
                        <div class="identity-kv">
                            <div class="identity-kv-label">Rate</div>
                            <div class="identity-kv-value">${rateMolt} MOLT/request</div>
                        </div>
                        <div class="identity-kv">
                            <div class="identity-kv-label">Metadata</div>
                            <div class="identity-kv-value"><pre class="code-block" style="margin:0; max-height:180px;">${metadataText}</pre></div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    `;
    enforceAddressViewOnlyMode();
}

function trustTierNumber(score) {
    if (score >= 950) return 6;
    if (score >= 800) return 5;
    if (score >= 600) return 4;
    if (score >= 400) return 3;
    if (score >= 200) return 2;
    if (score >= 100) return 1;
    return 0;
}

function getNextTierInfo(score) {
    const milestones = [
        { name: 'Newcomer', score: 100 },
        { name: 'Verified', score: 200 },
        { name: 'Trusted', score: 400 },
        { name: 'Established', score: 600 },
        { name: 'Elite', score: 800 },
        { name: 'Legendary', score: 950 }
    ];
    const next = milestones.find(entry => score < entry.score);
    if (!next) return 'Top tier reached.';
    return `Next tier: ${next.name} at ${formatNumber(next.score)} (${formatNumber(next.score - score)} points away)`;
}

function trustTierLadder(currentTier) {
    const tiers = ['Probation','Newcomer','Verified','Trusted','Established','Elite','Legendary'];
    const colors = ['#95a5a6','#6c7a89','#3498db','#2ecc71','#f1c40f','#e67e22','#e74c3c'];
    return tiers.map((name, idx) => {
        const active = idx === currentTier;
        const filled = idx <= currentTier;
        const color = filled ? colors[idx] : '#3a3a4a';
        const weight = active ? 'font-weight:700;' : '';
        const border = active ? `border:2px solid ${colors[idx]};` : '';
        return `<span style="display:inline-block;padding:2px 8px;border-radius:4px;font-size:0.75rem;color:${filled ? '#fff' : '#666'};background:${color};${weight}${border}margin-right:4px;">${name}</span>`;
    }).join('<span style="color:#555;margin:0 2px;">›</span>');
}

function formatTimestamp(value) {
    const n = Number(value || 0);
    if (!n) return 'Genesis';
    const ms = n > 1_000_000_000_000 ? n : n * 1000;
    const date = new Date(ms);
    if (Number.isNaN(date.getTime())) return 'Genesis';
    return date.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

// Convert a slot number to an approximate human-readable expiry date.
// MoltChain: 2.5 slots/sec = 400ms per slot. SLOTS_PER_YEAR = 78,840,000.
function formatSlotExpiry(expirySlot, registeredSlot) {
    const SLOTS_PER_YEAR = 78_840_000;
    const MS_PER_SLOT = 400;
    const slot = Number(expirySlot || 0);
    if (!slot) return 'Unknown';

    const regSlot = Number(registeredSlot || 0);
    const durationSlots = slot - regSlot;
    const durationYears = Math.round(durationSlots / SLOTS_PER_YEAR);

    // Estimate the actual expiry date:
    // Approximate genesis time as (now - currentSlot * 500ms)
    // Then expiryDate = genesisTime + expirySlot * 500ms
    // For simplicity, estimate remainingSlots from now:
    const nowMs = Date.now();
    // We don't know the current slot precisely, but we can estimate:
    // remaining time ≈ (expirySlot - regSlot) * MS_PER_SLOT from registration time
    // For genesis (regSlot=0), expiryDate ≈ genesisTime + slot * 500ms
    // Assume genesis was very recent → expiryDate ≈ now + slot * 500ms
    const approxDate = new Date(nowMs + (slot * MS_PER_SLOT) - (regSlot * MS_PER_SLOT));
    const dateStr = approxDate.toLocaleDateString(undefined, { year: 'numeric', month: 'short' });

    return `${dateStr} (~${durationYears}yr)`;
}

function showAddressToast(message, timeout = 3200) {
    const toast = document.getElementById('addressToast');
    if (!toast) return;
    toast.textContent = message;
    toast.style.display = 'block';
    if (showAddressToast._timer) clearTimeout(showAddressToast._timer);
    showAddressToast._timer = setTimeout(() => {
        toast.style.display = 'none';
    }, timeout);
}

function closeActionModal() {
    const modal = document.getElementById('addressActionModal');
    if (!modal) return;
    modal.style.display = 'none';
    modal.innerHTML = '';
    modal.setAttribute('aria-hidden', 'true');
}

function openActionModal({ title, icon = 'fas fa-pen', bodyHtml, confirmText = 'Confirm', onConfirm }) {
    const modal = document.getElementById('addressActionModal');
    if (!modal) return;

    modal.innerHTML = `
        <div class="modal-card" role="dialog" aria-modal="true">
            <div class="modal-header">
                <h3><i class="${icon}"></i> ${escapeHtml(title)}</h3>
                <button class="copy-icon" id="addressModalCloseBtn"><i class="fas fa-times"></i></button>
            </div>
            <div class="modal-body">${bodyHtml}</div>
            <div class="modal-footer">
                <button class="btn btn-secondary" id="addressModalCancelBtn">Cancel</button>
                <button class="btn btn-primary" id="addressModalConfirmBtn">${escapeHtml(confirmText)}</button>
            </div>
        </div>
    `;

    modal.style.display = 'flex';
    modal.setAttribute('aria-hidden', 'false');

    const closeButtons = [
        document.getElementById('addressModalCloseBtn'),
        document.getElementById('addressModalCancelBtn')
    ];
    closeButtons.forEach(button => button?.addEventListener('click', closeActionModal));

    modal.addEventListener('click', (event) => {
        if (event.target === modal) closeActionModal();
    }, { once: true });

    const confirmBtn = document.getElementById('addressModalConfirmBtn');
    if (confirmBtn) {
        confirmBtn.addEventListener('click', async () => {
            confirmBtn.disabled = true;
            try {
                const shouldClose = await onConfirm();
                if (shouldClose !== false) closeActionModal();
            } finally {
                if (document.body.contains(confirmBtn)) confirmBtn.disabled = false;
            }
        });
    }
}

function getWalletStateFromStorage() {
    try {
        const raw = localStorage.getItem('moltWalletState');
        if (!raw) return null;
        return JSON.parse(raw);
    } catch (error) {
        return null;
    }
}

function getActiveWalletFromStorage() {
    const state = getWalletStateFromStorage();
    if (!state || !Array.isArray(state.wallets)) return null;
    return state.wallets.find(wallet => wallet.id === state.activeWalletId) || null;
}

function ensureWalletAvailable() {
    const wallet = getActiveWalletFromStorage();
    if (!wallet || !wallet.address || !wallet.encryptedKey) {
        showAddressToast('No active wallet found. Open Wallet to create/import and unlock.');
        return null;
    }
    return wallet;
}

async function requestWalletPassword(actionText) {
    return new Promise((resolve) => {
        openActionModal({
            title: 'Sign Transaction',
            icon: 'fas fa-key',
            confirmText: 'Sign & Send',
            bodyHtml: `
                <p class="modal-note">${escapeHtml(actionText)}</p>
                <div class="form-group">
                    <label for="txPasswordInput">Wallet Password</label>
                    <input type="password" id="txPasswordInput" placeholder="Enter password to sign">
                </div>
            `,
            onConfirm: async () => {
                const password = document.getElementById('txPasswordInput')?.value || '';
                if (!password) {
                    showAddressToast('Password is required.');
                    return false;
                }
                resolve(password);
                return true;
            }
        });

        const cancel = document.getElementById('addressModalCancelBtn');
        const close = document.getElementById('addressModalCloseBtn');
        const overlay = document.getElementById('addressActionModal');
        cancel?.addEventListener('click', () => resolve(null), { once: true });
        close?.addEventListener('click', () => resolve(null), { once: true });
        overlay?.addEventListener('click', (event) => {
            if (event.target === overlay) resolve(null);
        }, { once: true });
    });
}

async function getMoltyIdProgramAddress() {
    if (moltyIdProgramAddress) return moltyIdProgramAddress;
    const symbols = ['YID', 'yid', 'MOLTYID'];
    for (const symbol of symbols) {
        try {
            const result = await rpcCall('getSymbolRegistry', [symbol]);
            const program = result?.program || result?.address || result?.pubkey;
            if (program) {
                moltyIdProgramAddress = program;
                return moltyIdProgramAddress;
            }
        } catch (error) {
            continue;
        }
    }
    throw new Error('MoltyID program not found in symbol registry');
}

function buildTransferInstruction(fromAddress, toAddress, amountMolt) {
    const fromPubkey = bs58decode(fromAddress);
    const toPubkey = bs58decode(toAddress);
    const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]
    const amountShells = Math.floor(Number(amountMolt) * 1_000_000_000);
    const instructionData = new Uint8Array(9);
    instructionData[0] = 0;
    const view = new DataView(instructionData.buffer);
    view.setBigUint64(1, BigInt(amountShells), true);

    return {
        program_id: Array.from(systemProgram),
        accounts: [Array.from(fromPubkey), Array.from(toPubkey)],
        data: Array.from(instructionData)
    };
}

function buildContractCallInstruction({ callerAddress, contractAddress, functionName, args, value = 0 }) {
    const callerPubkey = bs58decode(callerAddress);
    const contractPubkey = bs58decode(contractAddress);
    const contractProgramId = new Uint8Array(32).fill(0xFF);

    const callArgs = JSON.stringify(args || {});
    const payload = JSON.stringify({
        Call: {
            function: functionName,
            args: Array.from(new TextEncoder().encode(callArgs)),
            value
        }
    });

    return {
        program_id: Array.from(contractProgramId),
        accounts: [Array.from(callerPubkey), Array.from(contractPubkey)],
        data: Array.from(new TextEncoder().encode(payload))
    };
}

function bytesToBase64(bytes) {
    let binary = '';
    const chunkSize = 0x8000;
    for (let i = 0; i < bytes.length; i += chunkSize) {
        binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
    }
    return btoa(binary);
}

async function signAndSendInstructions(wallet, password, instructions) {
    const latestBlock = await rpcCall('getLatestBlock', []);
    const message = {
        instructions,
        blockhash: latestBlock.hash
    };

    const privateKey = await MoltCrypto.decryptPrivateKey(wallet.encryptedKey, password);
    const messageBytes = serializeMessageBincode(message);
    const signature = await MoltCrypto.signTransaction(privateKey, messageBytes);

    const transaction = {
        signatures: [Array.from(signature)],
        message
    };
    const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
    const txBase64 = bytesToBase64(txBytes);
    return rpcCall('sendTransaction', [txBase64]);
}

async function resolveRecipient(value) {
    const input = String(value || '').trim();
    if (!input) throw new Error('Recipient is required');
    if (input.endsWith('.molt')) {
        const label = input.slice(0, -5);
        const result = await rpcCall('resolveMoltName', [label]);
        if (!result?.owner) throw new Error('Name could not be resolved');
        return { address: result.owner, display: input };
    }
    if (!MoltCrypto.isValidAddress(input)) throw new Error('Invalid recipient address');
    return { address: input, display: input };
}

async function openSendModal() {
    const wallet = ensureWalletAvailable();
    if (!wallet) return;

    openActionModal({
        title: 'Send MOLT',
        icon: 'fas fa-paper-plane',
        confirmText: 'Sign & Send',
        bodyHtml: `
            <div class="form-group">
                <label for="sendRecipientInput">Recipient (.molt or address)</label>
                <input id="sendRecipientInput" placeholder="alice.molt or MoLT...">
            </div>
            <div class="form-group">
                <label for="sendAmountInput">Amount (MOLT)</label>
                <input id="sendAmountInput" type="number" min="0" step="0.000001" placeholder="0.0">
            </div>
            <div class="form-group">
                <label for="sendMemoInput">Memo (optional)</label>
                <input id="sendMemoInput" maxlength="140" placeholder="Memo">
            </div>
            <p class="modal-note">The memo is kept in the UI flow for compatibility and not attached to transaction data.</p>
        `,
        onConfirm: async () => {
            try {
                const recipientInput = document.getElementById('sendRecipientInput')?.value || '';
                const amount = Number(document.getElementById('sendAmountInput')?.value || 0);
                if (!amount || amount <= 0) throw new Error('Enter a valid amount');

                const recipient = await resolveRecipient(recipientInput);
                const password = await requestWalletPassword(`Send ${amount} MOLT to ${recipient.display}`);
                if (!password) return false;

                const instruction = buildTransferInstruction(wallet.address, recipient.address, amount);
                showAddressToast('Sending transaction...');
                const signature = await signAndSendInstructions(wallet, password, [instruction]);
                showAddressToast(`Sent successfully: ${String(signature).slice(0, 16)}...`);
                await loadAddressData();
                return true;
            } catch (error) {
                showAddressToast(`Send failed: ${error.message}`);
                return false;
            }
        }
    });
}

async function openRegisterIdentityModal() {
    const wallet = ensureWalletAvailable();
    if (!wallet) return;
    if (wallet.address !== currentAddress) {
        showAddressToast('Identity registration is only available for the active wallet address.');
        return;
    }

    const agentTypeOptions = AGENT_TYPE_OPTIONS.map(option =>
        `<option value="${option.value}">${escapeHtml(option.label)}</option>`
    ).join('');

    openActionModal({
        title: 'Create Your MoltyID Identity',
        icon: 'fas fa-id-badge',
        confirmText: 'Create Identity',
        bodyHtml: `
            <div class="form-group">
                <label for="identityNameInput">Display Name</label>
                <input id="identityNameInput" minlength="3" maxlength="64" placeholder="My Agent">
            </div>
            <div class="form-group">
                <label for="identityTypeInput">Agent Type</label>
                <select id="identityTypeInput">${agentTypeOptions}</select>
            </div>
            <div class="form-group">
                <label for="identityMoltNameInput">Optional .molt name</label>
                <input id="identityMoltNameInput" placeholder="trading_bot">
            </div>
            <div class="form-group">
                <label for="identityMoltYearsInput">Name Duration</label>
                <select id="identityMoltYearsInput">
                    <option value="1">1 year</option>
                    <option value="2">2 years</option>
                    <option value="5">5 years</option>
                </select>
            </div>
            <p class="modal-note">Initial reputation starts at 100. Registration is gas-only.</p>
        `,
        onConfirm: async () => {
            try {
                const name = String(document.getElementById('identityNameInput')?.value || '').trim();
                const agentType = Number(document.getElementById('identityTypeInput')?.value || 0);
                const optionalName = String(document.getElementById('identityMoltNameInput')?.value || '').trim().toLowerCase();
                const years = Number(document.getElementById('identityMoltYearsInput')?.value || 1);

                if (name.length < 3 || name.length > 64) {
                    throw new Error('Display name must be 3-64 characters');
                }

                const moltyIdAddress = await getMoltyIdProgramAddress();
                const password = await requestWalletPassword(`Register identity for ${wallet.address}`);
                if (!password) return false;

                const registerInstruction = buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'register_identity',
                    args: {
                        owner: wallet.address,
                        agent_type: agentType,
                        name
                    },
                    value: 0
                });

                showAddressToast('Submitting identity registration...');
                await signAndSendInstructions(wallet, password, [registerInstruction]);

                if (optionalName) {
                    const nameInstruction = buildContractCallInstruction({
                        callerAddress: wallet.address,
                        contractAddress: moltyIdAddress,
                        functionName: 'register_name',
                        args: {
                            name: optionalName,
                            duration_years: years
                        },
                        value: 0
                    });
                    showAddressToast(`Registering ${optionalName}.molt...`);
                    await signAndSendInstructions(wallet, password, [nameInstruction]);
                }

                showAddressToast('Identity action submitted successfully.');
                await loadAddressData();
                return true;
            } catch (error) {
                showAddressToast(`Identity registration failed: ${error.message}`);
                return false;
            }
        }
    });
}

async function openRegisterNameModal() {
    const wallet = ensureWalletAvailable();
    if (!wallet) return;
    if (wallet.address !== currentAddress) {
        showAddressToast('Name registration is only available for the active wallet identity page.');
        return;
    }

    openActionModal({
        title: 'Register .molt Name',
        icon: 'fas fa-at',
        confirmText: 'Register Name',
        bodyHtml: `
            <div class="form-group">
                <label for="nameLabelInput">Name label</label>
                <input id="nameLabelInput" placeholder="trading_bot">
            </div>
            <div class="form-group">
                <label for="nameYearsInput">Duration</label>
                <select id="nameYearsInput">
                    <option value="1">1 year</option>
                    <option value="2">2 years</option>
                    <option value="5">5 years</option>
                </select>
            </div>
            <div class="address-inline-actions">
                <button class="btn btn-secondary btn-small" id="checkNameBtn">Check Availability</button>
                <span id="nameCheckResult" class="modal-note"></span>
            </div>
        `,
        onConfirm: async () => {
            try {
                const label = String(document.getElementById('nameLabelInput')?.value || '').trim().toLowerCase();
                const years = Number(document.getElementById('nameYearsInput')?.value || 1);
                if (!label || label.length < 3 || label.length > 64) throw new Error('Name must be 3-64 characters');

                const availability = await rpcCall('resolveMoltName', [label]).catch(() => null);
                if (availability?.owner) throw new Error(`${label}.molt is already registered`);

                const moltyIdAddress = await getMoltyIdProgramAddress();
                const password = await requestWalletPassword(`Register ${label}.molt`);
                if (!password) return false;

                const instruction = buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'register_name',
                    args: {
                        name: label,
                        duration_years: years
                    },
                    value: 0
                });

                await signAndSendInstructions(wallet, password, [instruction]);
                showAddressToast(`Submitted registration for ${label}.molt`);
                await loadAddressData();
                return true;
            } catch (error) {
                showAddressToast(`Name registration failed: ${error.message}`);
                return false;
            }
        }
    });

    const checkBtn = document.getElementById('checkNameBtn');
    checkBtn?.addEventListener('click', async () => {
        const label = String(document.getElementById('nameLabelInput')?.value || '').trim().toLowerCase();
        const resultEl = document.getElementById('nameCheckResult');
        if (!resultEl) return;
        if (!label) {
            resultEl.textContent = 'Enter a name first.';
            return;
        }
        resultEl.textContent = 'Checking...';
        try {
            const result = await rpcCall('resolveMoltName', [label]);
            resultEl.textContent = result?.owner
                ? `Taken by ${formatHash(result.owner)}`
                : `${label}.molt is available`;
        } catch (error) {
            resultEl.textContent = `${label}.molt is available`;
        }
    });
}

async function openVouchModal() {
    const wallet = ensureWalletAvailable();
    if (!wallet) return;
    if (wallet.address === currentAddress) {
        showAddressToast('You cannot vouch for your own identity.');
        return;
    }

    openActionModal({
        title: 'Vouch for Identity',
        icon: 'fas fa-handshake',
        confirmText: 'Vouch',
        bodyHtml: `<p class="modal-note">This submits a vouch from <span title="${escapeHtml(wallet.address)}">${escapeHtml(formatHash(wallet.address))}</span> to <span title="${escapeHtml(currentAddress)}">${escapeHtml(formatHash(currentAddress))}</span>.</p>`,
        onConfirm: async () => {
            try {
                const moltyIdAddress = await getMoltyIdProgramAddress();
                const password = await requestWalletPassword('Sign vouch transaction');
                if (!password) return false;

                const instruction = buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'vouch',
                    args: {
                        voucher: wallet.address,
                        vouchee: currentAddress
                    },
                    value: 0
                });

                await signAndSendInstructions(wallet, password, [instruction]);
                showAddressToast('Vouch submitted.');
                await loadAddressData();
                return true;
            } catch (error) {
                showAddressToast(`Vouch failed: ${error.message}`);
                return false;
            }
        }
    });
}

async function openAttestSkillModal(initialSkillName = '') {
    const wallet = ensureWalletAvailable();
    if (!wallet) return;
    if (wallet.address === currentAddress) {
        showAddressToast('Use attestations for other identities.');
        return;
    }

    const knownSkills = (currentMoltyProfile?.skills || []).map(skill => String(skill.name || skill.skill || '').trim()).filter(Boolean);
    const skillOptions = knownSkills.map(skill => `<option value="${escapeHtml(skill)}">${escapeHtml(skill)}</option>`).join('');

    openActionModal({
        title: 'Attest Skill',
        icon: 'fas fa-certificate',
        confirmText: 'Attest',
        bodyHtml: `
            <div class="form-group">
                <label for="attestSkillInput">Skill</label>
                ${skillOptions ? `<select id="attestSkillInput">${skillOptions}</select>` : '<input id="attestSkillInput" placeholder="Skill name">'}
            </div>
            <div class="form-group">
                <label for="attestLevelInput">Level (1-5)</label>
                <input id="attestLevelInput" type="number" min="1" max="5" step="1" value="5">
            </div>
        `,
        onConfirm: async () => {
            try {
                const skill = String(document.getElementById('attestSkillInput')?.value || '').trim();
                const level = Number(document.getElementById('attestLevelInput')?.value || 0);
                if (!skill) throw new Error('Skill is required');
                if (level < 1 || level > 5) throw new Error('Level must be between 1 and 5');

                const moltyIdAddress = await getMoltyIdProgramAddress();
                const password = await requestWalletPassword(`Attest ${skill} at level ${level}`);
                if (!password) return false;

                const instruction = buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'attest_skill',
                    args: {
                        attester: wallet.address,
                        target: currentAddress,
                        skill,
                        level
                    },
                    value: 0
                });

                await signAndSendInstructions(wallet, password, [instruction]);
                showAddressToast('Skill attestation submitted.');
                await loadAddressData();
                return true;
            } catch (error) {
                showAddressToast(`Attestation failed: ${error.message}`);
                return false;
            }
        }
    });

    const skillInput = document.getElementById('attestSkillInput');
    if (skillInput && initialSkillName) {
        skillInput.value = initialSkillName;
    }
}

async function openEditProfileModal() {
    const wallet = ensureWalletAvailable();
    if (!wallet) return;
    if (wallet.address !== currentAddress) {
        showAddressToast('Profile updates are only available for the active wallet identity.');
        return;
    }

    const agentTypeOptions = AGENT_TYPE_OPTIONS.map(option => {
        const selected = Number(currentMoltyProfile?.identity?.agent_type || 0) === option.value ? 'selected' : '';
        return `<option value="${option.value}" ${selected}>${escapeHtml(option.label)}</option>`;
    }).join('');

    const metadataDefault = JSON.stringify(currentMoltyProfile?.agent?.metadata || {}, null, 2);
    const availabilityValue = Number(currentMoltyProfile?.agent?.availability || 0);

    openActionModal({
        title: 'Edit Identity Profile',
        icon: 'fas fa-user-edit',
        confirmText: 'Save Changes',
        bodyHtml: `
            <div class="form-group">
                <label for="profileTypeInput">Agent Type</label>
                <select id="profileTypeInput">${agentTypeOptions}</select>
            </div>
            <div class="form-group">
                <label for="profileEndpointInput">Endpoint</label>
                <input id="profileEndpointInput" value="${escapeHtml(currentMoltyProfile?.agent?.endpoint || '')}" placeholder="https://api.example.com/agent">
            </div>
            <div class="form-group">
                <label for="profileAvailabilityInput">Availability</label>
                <select id="profileAvailabilityInput">
                    <option value="1" ${availabilityValue === 1 ? 'selected' : ''}>Available</option>
                    <option value="0" ${availabilityValue !== 1 ? 'selected' : ''}>Offline</option>
                </select>
            </div>
            <div class="form-group">
                <label for="profileRateInput">Rate (MOLT/request)</label>
                <input id="profileRateInput" type="number" min="0" step="0.000001" value="${Number(currentMoltyProfile?.agent?.rate || 0) / 1_000_000_000}">
            </div>
            <div class="form-group">
                <label for="profileMetadataInput">Metadata (JSON)</label>
                <textarea id="profileMetadataInput">${escapeHtml(metadataDefault)}</textarea>
            </div>
        `,
        onConfirm: async () => {
            try {
                const type = Number(document.getElementById('profileTypeInput')?.value || 0);
                const endpoint = String(document.getElementById('profileEndpointInput')?.value || '').trim();
                const availability = Number(document.getElementById('profileAvailabilityInput')?.value || 0);
                const rateMolt = Number(document.getElementById('profileRateInput')?.value || 0);
                const metadataText = String(document.getElementById('profileMetadataInput')?.value || '{}').trim();
                const metadata = metadataText ? JSON.parse(metadataText) : {};

                const moltyIdAddress = await getMoltyIdProgramAddress();
                const instructions = [];

                instructions.push(buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'update_agent_type',
                    args: { owner: wallet.address, agent_type: type },
                    value: 0
                }));

                instructions.push(buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'set_endpoint',
                    args: { owner: wallet.address, endpoint },
                    value: 0
                }));

                instructions.push(buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'set_metadata',
                    args: { owner: wallet.address, metadata },
                    value: 0
                }));

                instructions.push(buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'set_availability',
                    args: { owner: wallet.address, availability },
                    value: 0
                }));

                instructions.push(buildContractCallInstruction({
                    callerAddress: wallet.address,
                    contractAddress: moltyIdAddress,
                    functionName: 'set_rate',
                    args: { owner: wallet.address, rate: Math.floor(rateMolt * 1_000_000_000) },
                    value: 0
                }));

                const password = await requestWalletPassword('Sign profile update transaction');
                if (!password) return false;

                for (const instruction of instructions) {
                    await signAndSendInstructions(wallet, password, [instruction]);
                }

                showAddressToast('Profile updates submitted.');
                await loadAddressData();
                return true;
            } catch (error) {
                showAddressToast(`Profile update failed: ${error.message}`);
                return false;
            }
        }
    });
}

function bindIdentityActionButtons() {
    document.querySelectorAll('[data-identity-action]').forEach(button => {
        button.addEventListener('click', async (event) => {
            const action = event.currentTarget?.dataset?.identityAction;
            if (!action) return;

            if (action === 'register-name') {
                await openRegisterNameModal();
                return;
            }
            if (action === 'vouch') {
                await openVouchModal();
                return;
            }
            if (action === 'attest-skill') {
                await openAttestSkillModal(event.currentTarget?.dataset?.skill || '');
                return;
            }
            if (action === 'edit-profile' || action === 'set-endpoint' || action === 'set-availability' || action === 'set-rate') {
                await openEditProfileModal();
            }
        });
    });
}

// ===== Registry helpers =====
function setRegistryRowsVisible(visible) {
    document.querySelectorAll('.registry-row').forEach(row => {
        row.style.display = visible ? 'flex' : 'none';
    });
}
function clearRegistryInfo() {
    setRegistryRowsVisible(false);
    ['registrySymbol','registryName','registryTemplate','registryOwner','registryMetadata'].forEach(id => {
        const el = document.getElementById(id);
        if (el) el.textContent = '-';
    });
}
function formatRegistryMetadata(entry) {
    if (!entry?.metadata) return '-';
    const items = [];
    const md = entry.metadata;
    if (entry.template === 'token') {
        if (md.decimals !== undefined) items.push(`decimals: ${md.decimals}`);
        if (md.supply !== undefined) items.push(`supply: ${md.supply}`);
        if (md.mintable !== undefined) items.push(`mintable: ${md.mintable}`);
        if (md.burnable !== undefined) items.push(`burnable: ${md.burnable}`);
    }
    if (entry.template === 'nft') {
        if (md.max_supply !== undefined) items.push(`max_supply: ${md.max_supply}`);
        if (md.royalty_bps !== undefined) items.push(`royalty_bps: ${md.royalty_bps}`);
    }
    Object.entries(md).forEach(([k, v]) => {
        if (!items.some(i => i.startsWith(`${k}:`))) items.push(`${k}: ${v}`);
    });
    return items.length ? items.join(' | ') : '-';
}
async function loadRegistryInfo(programId) {
    try {
        const entry = await rpcCall('getSymbolRegistryByProgram', [programId]);
        setRegistryRowsVisible(true);
        if (!entry) {
            document.getElementById('registrySymbol').textContent = 'Not registered';
            ['registryName','registryTemplate','registryOwner','registryMetadata'].forEach(id => {
                const el = document.getElementById(id);
                if (el) el.textContent = '-';
            });
            return;
        }
        document.getElementById('registrySymbol').textContent = entry.symbol || '-';
        document.getElementById('registryName').textContent = entry.name || '-';
        document.getElementById('registryTemplate').textContent = entry.template || '-';
        document.getElementById('registryOwner').textContent = entry.owner ? formatHash(entry.owner) : '-';
        if (entry.owner) document.getElementById('registryOwner').title = entry.owner;
        const metaEl = document.getElementById('registryMetadata');
        if (metaEl) metaEl.textContent = formatRegistryMetadata(entry);
    } catch (error) {
        setRegistryRowsVisible(true);
        document.getElementById('registrySymbol').textContent = 'Unavailable';
    }
}

// ===== Genesis Account Labels (loaded dynamically from RPC) =====
let KNOWN_ADDRESSES = {};
let _genesisAccountsLoaded = false;

async function loadGenesisAccounts() {
    if (_genesisAccountsLoaded) return;
    try {
        const result = await rpcCall('getGenesisAccounts', []);
        const accounts = result?.accounts || [];
        for (const acc of accounts) {
            if (acc.pubkey && acc.label) {
                KNOWN_ADDRESSES[acc.pubkey] = acc.label;
            }
        }
        _genesisAccountsLoaded = true;
    } catch (e) {
        console.warn('Failed to load genesis accounts:', e);
    }
}

// ===== Validator detection + account type =====
async function applyValidatorType(data) {
    // Ensure genesis accounts are loaded
    await loadGenesisAccounts();
    // Check known addresses first
    if (KNOWN_ADDRESSES[data.base58]) {
        data.type = KNOWN_ADDRESSES[data.base58];
        return;
    }
    if (data.executable) { data.type = 'Program'; return; }
    try {
        const validators = await rpcCall('getValidators', []);
        const list = Array.isArray(validators) ? validators : (validators?.validators || []);
        if (list.some(v => v.pubkey === data.base58)) data.type = 'Validator';
    } catch (e) { /* ignore */ }
}

// ===== Validator Rewards =====
async function loadValidatorRewards(address) {
    try {
        const rewards = await rpcCall('getStakingRewards', [address]);
        if (!rewards) {
            const emptyCard = document.getElementById('validatorRewardsCard');
            if (emptyCard) emptyCard.style.display = 'none';
            return false;
        }

        const card = document.getElementById('validatorRewardsCard');
        if (card) card.style.display = 'block';

        const totalEarned = rewards.total_rewards || 0;
        const pending = rewards.pending_rewards || 0;
        const claimed = rewards.claimed_rewards || 0;
        const rate = rewards.reward_rate || '0';
        const debt = rewards.bootstrap_debt || 0;
        const earned = rewards.total_debt_repaid || rewards.earned_amount || 0;
        const vesting = rewards.vesting_progress || 0;
        const blocksProduced = rewards.blocks_produced || 0;
        const rewardRateNumeric = parseFloat(rate) || 0;

        const hasMeaningfulData =
            Number(totalEarned) > 0 ||
            Number(pending) > 0 ||
            Number(claimed) > 0 ||
            Number(debt) > 0 ||
            Number(earned) > 0 ||
            Number(vesting) > 0 ||
            Number(blocksProduced) > 0 ||
            rewardRateNumeric > 0;

        if (!hasMeaningfulData) {
            if (card) card.style.display = 'none';
            return false;
        }

        const fmt = (v) => {
            const molt = typeof v === 'number' ? v / 1_000_000_000 : parseFloat(v) || 0;
            return molt.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 }) + ' MOLT';
        };

        document.getElementById('rewardsTotalEarned').textContent = fmt(totalEarned);
        document.getElementById('rewardsPending').textContent = fmt(pending);
        document.getElementById('rewardsClaimed').textContent = fmt(claimed);

        // Calculate actual reward rate from total rewards / blocks produced
        let actualRate;
        if (blocksProduced > 0 && Number(totalEarned) > 0) {
            const totalMolt = Number(totalEarned) / 1_000_000_000;
            actualRate = (totalMolt / blocksProduced).toFixed(4) + ' MOLT/block';
        } else {
            actualRate = rate + ' MOLT/block';
        }
        document.getElementById('rewardsRate').textContent = actualRate;

        const blocksEl = document.getElementById('rewardsBlocksProduced');
        if (blocksEl) blocksEl.textContent = blocksProduced.toLocaleString();

        // Debt section
        const debtMolt = typeof debt === 'number' ? debt / 1_000_000_000 : parseFloat(debt) || 0;
        const earnedMolt = typeof earned === 'number' ? earned / 1_000_000_000 : parseFloat(earned) || 0;
        document.getElementById('rewardsDebt').textContent = debtMolt.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 }) + ' MOLT';
        document.getElementById('rewardsDebtRepaid').textContent = earnedMolt.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 }) + ' MOLT';

        const vestingPct = Math.min(100, Math.max(0, (typeof vesting === 'number' ? vesting * 100 : parseFloat(vesting) * 100) || 0));
        const vestingLabel = vestingPct > 0 && vestingPct < 0.1 ? '< 0.1%' : vestingPct.toFixed(1) + '%';
        document.getElementById('rewardsVestingText').textContent = vestingLabel;
        // Give the progress bar a visible minimum width (3%) if there's any progress at all
        const barWidth = vestingPct > 0 ? Math.max(3, vestingPct) : 0;
        document.getElementById('vestingProgressBar').style.width = barWidth + '%';
        return true;
    } catch (e) {
        console.warn('Failed to load staking rewards:', e);
        const emptyCard = document.getElementById('validatorRewardsCard');
        if (emptyCard) emptyCard.style.display = 'none';
        return false;
    }
}

// ===== Treasury Stats =====
async function loadTreasuryStats(address) {
    try {
        // Fetch treasury's transaction history to count airdrops
        const result = await rpcCall('getTransactionsByAddress', [address, { limit: 500 }]);
        const transactions = result?.transactions || (Array.isArray(result) ? result : []);

        let airdropCount = 0;
        let totalAirdropped = 0;
        let feeRevenue = 0;
        const uniqueRecipients = new Set();

        for (const tx of transactions) {
            if (tx.type === 'Airdrop' || tx.type === 'airdrop' || tx.memo === 'faucet_airdrop') {
                airdropCount++;
                totalAirdropped += tx.amount || 0;
                if (tx.to) uniqueRecipients.add(tx.to);
            }
            // Count incoming fee revenue (treasury is a recipient of fee splits)
            if (tx.to === address && tx.type !== 'Airdrop' && tx.type !== 'airdrop') {
                feeRevenue += tx.amount || 0;
            }
        }

        const card = document.getElementById('treasuryStatsCard');
        if (!card) {
            // Create the card dynamically
            const container = document.getElementById('overviewPane') || document.querySelector('.container');
            const newCard = document.createElement('div');
            newCard.id = 'treasuryStatsCard';
            newCard.className = 'detail-card';
            newCard.innerHTML = `
                <div class="detail-card-header">
                    <h3><i class="fas fa-landmark"></i> Treasury Overview</h3>
                </div>
                <div class="detail-card-body">
                    <div class="detail-row"><span class="detail-label">Role</span><span class="detail-value">Network treasury - receives fee revenue, funds faucet airdrops</span></div>
                    <div class="detail-row"><span class="detail-label">Faucet Airdrops</span><span class="detail-value" id="treasuryAirdrops">${airdropCount}</span></div>
                    <div class="detail-row"><span class="detail-label">Total Airdropped</span><span class="detail-value" id="treasuryTotalAirdropped">${formatNumber(totalAirdropped)} MOLT</span></div>
                    <div class="detail-row"><span class="detail-label">Unique Recipients</span><span class="detail-value" id="treasuryRecipients">${uniqueRecipients.size}</span></div>
                    <div class="detail-row"><span class="detail-label">Fee Revenue (est.)</span><span class="detail-value" id="treasuryFeeRevenue">${formatNumber(feeRevenue)} MOLT</span></div>
                </div>
            `;
            // Insert after the first detail card
            const firstCard = container.querySelector('.detail-card');
            if (firstCard?.nextSibling) {
                container.insertBefore(newCard, firstCard.nextSibling);
            } else {
                container.appendChild(newCard);
            }
        }
    } catch (e) {
        console.warn('Failed to load treasury stats:', e);
    }
}

// ===== Fetch Account from RPC =====
async function fetchAccountFromRPC(address) {
    // Parallel fetch: balance + account + tx count + token accounts
    const [balanceData, accountData, txCountData, tokenData] = await Promise.all([
        rpcCall('getBalance', [address]).catch(() => null),
        rpcCall('getAccount', [address]).catch(() => null),
        rpcCall('getAccountTxCount', [address]).catch(() => null),
        rpcCall('getTokenAccounts', [address]).catch(() => null),
    ]);

    if (!balanceData) return null;

    const txCount = txCountData?.count || 0;
    const tokens = tokenData?.accounts || [];

    return {
        address: accountData?.pubkey || address,
        base58: accountData?.pubkey || address,
        evm: accountData?.evm_address || moltchainToEvmAddress(address),
        shells: balanceData.shells,
        molt: parseFloat(balanceData.molt),
        spendable: parseFloat(balanceData.spendable_molt),
        staked: parseFloat(balanceData.staked_molt),
        locked: parseFloat(balanceData.locked_molt),
        reefStaked: parseFloat(balanceData.reef_staked_molt || '0'),
        reefValue: parseFloat(balanceData.reef_value_molt || '0'),
        owner: accountData?.owner || (typeof SYSTEM_PROGRAM_ID !== 'undefined' ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111'),
        executable: accountData?.executable || false,
        data_len: accountData?.data_len || 0,
        active: balanceData.shells > 0,
        txCount,
        tokens,
        type: accountData?.executable ? 'Program' : 'User'
    };
}

function createEmptyAccountData(address) {
    return {
        address, base58: address,
        evm: moltchainToEvmAddress(address) || 'Unavailable',
        shells: 0, molt: 0, spendable: 0, staked: 0, locked: 0,
        reefStaked: 0, reefValue: 0,
        data: [], owner: typeof SYSTEM_PROGRAM_ID !== 'undefined' ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111',
        executable: false, rentEpoch: 0, txCount: 0, tokens: [], type: 'User', active: false
    };
}

// ===== Display Address Data =====
function displayAddressData(data) {
    const statusEl = document.getElementById('addressStatus');
    if (data.active) {
        statusEl.innerHTML = '<i class="fas fa-check-circle"></i> Active';
        statusEl.className = 'detail-status';
    } else {
        statusEl.innerHTML = '<i class="fas fa-times-circle"></i> Inactive';
        statusEl.className = 'detail-status failed';
    }

    document.getElementById('addressBalance').textContent = `${formatNumber(data.molt)} MOLT`;
    document.getElementById('tokenBalance').textContent =
        data.tokens?.length > 0 ? `${data.tokens.length} tokens` : '0 tokens';
    document.getElementById('txCount').textContent = formatNumber(data.txCount);
    document.getElementById('accountType').textContent = data.type || 'User';

    document.getElementById('addressBase58').textContent = formatHash(data.base58);
    document.getElementById('addressBase58').setAttribute('data-full', data.base58);
    document.getElementById('addressBase58').title = data.base58;
    document.getElementById('addressEVM').textContent = data.evm ? formatHash(data.evm) : 'Unavailable';
    if (data.evm) document.getElementById('addressEVM').title = data.evm;
    document.getElementById('balanceMolt').textContent = `${formatNumber(data.molt)} MOLT`;
    document.getElementById('balanceShells').textContent = `${formatNumber(data.shells)} shells`;

    document.getElementById('spendableMolt').textContent = `${formatNumber(data.spendable)} MOLT`;
    document.getElementById('stakedMolt').textContent = `${formatNumber(data.staked)} MOLT`;
    document.getElementById('lockedMolt').textContent = `${formatNumber(data.locked)} MOLT`;

    // ReefStake liquid staking display
    let reefStakedEl = document.getElementById('reefStakedMolt');
    if (!reefStakedEl) {
        // Inject ReefStake row after staked row
        const stakedEl = document.getElementById('stakedMolt');
        if (stakedEl) {
            const parentRow = stakedEl.closest('.detail-row') || stakedEl.parentElement;
            if (parentRow && parentRow.parentElement) {
                const reefRow = document.createElement('div');
                reefRow.className = parentRow.className;
                reefRow.innerHTML = `
                    <div class="detail-label">ReefStake (Liquid)</div>
                    <div class="detail-value" id="reefStakedMolt">0 MOLT</div>
                `;
                parentRow.parentElement.insertBefore(reefRow, parentRow.nextSibling);
                reefStakedEl = document.getElementById('reefStakedMolt');
            }
        }
    }
    if (reefStakedEl) {
        const reefVal = data.reefValue || data.reefStaked || 0;
        reefStakedEl.textContent = reefVal > 0 ? `${formatNumber(reefVal)} MOLT` : '0 MOLT';
    }

    const ownerEl = document.getElementById('ownerProgram');
    const isSystemOwner = isSystemProgramOwner(data.owner);
    ownerEl.textContent = isSystemOwner ? 'System Program' : formatHash(data.owner);
    if (!isSystemOwner) ownerEl.title = data.owner;
    const ownerLink = document.getElementById('ownerLink');
    if (!isSystemOwner) {
        ownerLink.href = `address.html?address=${data.owner}`;
        ownerLink.style.display = 'inline-flex';
    } else {
        ownerLink.style.display = 'none';
    }

    document.getElementById('executableStatus').innerHTML =
        data.executable
            ? '<span class="badge success">Yes</span>'
            : '<span class="badge">No</span>';

    const dataSize = data.data_len || (data.data ? data.data.length : 0);
    document.getElementById('dataSize').textContent = dataSize > 0 ? formatBytes(dataSize) : '0 bytes';
    document.getElementById('rentEpoch').textContent = data.rentEpoch || '0';

    // Token balances — always show tokens tab, render empty state if none
    setTokensTabVisibility(true);
    document.getElementById('tokensCard').style.display = 'block';
    displayTokenBalances(data.tokens || []);

    document.getElementById('rawData').textContent = JSON.stringify(data, null, 2);

    const summaryAddress = document.getElementById('summaryAddress');
    const summaryEvmAddress = document.getElementById('summaryEvmAddress');
    const summaryBalance = document.getElementById('summaryBalance');
    const summarySpendable = document.getElementById('summarySpendable');
    const summaryStaked = document.getElementById('summaryStaked');
    const summaryLocked = document.getElementById('summaryLocked');

    if (summaryAddress) {
        summaryAddress.textContent = formatHash(data.base58);
        summaryAddress.title = data.base58;
    }
    if (summaryEvmAddress) {
        summaryEvmAddress.textContent = data.evm ? formatHash(data.evm) : 'Unavailable';
        if (data.evm) summaryEvmAddress.title = data.evm;
    }
    if (summaryBalance) summaryBalance.textContent = `${formatNumber(data.molt)} MOLT`;
    if (summarySpendable) summarySpendable.textContent = `Spendable: ${formatNumber(data.spendable)} MOLT`;
    if (summaryStaked) {
        const totalStaked = (data.staked || 0) + (data.reefValue || data.reefStaked || 0);
        summaryStaked.textContent = `Staked: ${formatNumber(totalStaked)} MOLT`;
    }
    if (summaryLocked) summaryLocked.textContent = `Locked: ${formatNumber(data.locked)} MOLT`;
    enforceAddressViewOnlyMode();
}

function displayTokenBalances(tokens) {
    const tbody = document.getElementById('tokensTable');
    if (!tbody) return;
    tbody.innerHTML = '';

    if (!tokens || tokens.length === 0) {
        tbody.innerHTML = `
            <tr>
                <td colspan="4" class="empty-state">
                    <i class="fas fa-info-circle"></i> No token balances found for this address
                </td>
            </tr>
        `;
        return;
    }

    tokens.forEach(token => {
        const mintAddr = token.mint || '';
        const symbol = escapeHtml(token.symbol || 'Unknown');
        const name = escapeHtml(token.name || symbol);
        const decimals = Number(token.decimals || 9);
        const rawBalance = Number(token.balance || 0);
        const uiAmount = token.ui_amount !== undefined
            ? Number(token.ui_amount)
            : rawBalance / Math.pow(10, decimals);

        const row = document.createElement('tr');
        row.innerHTML = `
            <td>
                <a href="contract.html?address=${mintAddr}">${name}</a>
                <div style="font-size:0.75rem; color:var(--text-muted); font-family:var(--font-mono);">
                    ${formatAddress(mintAddr)}
                </div>
            </td>
            <td><strong>${symbol}</strong></td>
            <td>${uiAmount.toLocaleString(undefined, { minimumFractionDigits: 0, maximumFractionDigits: decimals })}</td>
            <td>—</td>
        `;
        tbody.appendChild(row);
    });
}

// ===== Transaction History with Cursor Pagination =====
async function loadTransactionHistory(address, beforeSlot) {
    try {
        const opts = { limit: TX_PAGE_SIZE };
        if (beforeSlot !== undefined && beforeSlot !== null) {
            opts.before_slot = beforeSlot;
        }
        txCurrentBeforeSlot = beforeSlot;

        const result = await rpcCall('getTransactionsByAddress', [address, opts]);
        const transactions = result?.transactions || result?.items || result?.data || (Array.isArray(result) ? result : []);
        txNextCursor = result?.next_before_slot || null;

        displayTransactions(transactions);

        const historyCount = transactions.length;
        document.getElementById('historyCount').textContent = historyCount;

        // Update pagination UI
        updateTxPagination();

    } catch (error) {
        console.error('Error loading transactions:', error);
        document.getElementById('transactionsTable').innerHTML = `
            <tr><td colspan="7" class="empty-state">
                <i class="fas fa-exclamation-triangle"></i> Failed to load transactions
            </td></tr>`;
    }
}

function updateTxPagination() {
    let paginationEl = document.getElementById('addressTxPagination');
    if (!paginationEl) {
        // Create pagination controls if they don't exist
        const table = document.getElementById('transactionsTable');
        if (!table) return;
        const container = table.closest('.detail-card-body') || table.parentElement;
        paginationEl = document.createElement('div');
        paginationEl.id = 'addressTxPagination';
        paginationEl.style.cssText = 'display: flex; justify-content: center; align-items: center; gap: 1rem; padding: 1rem 0;';
        container.appendChild(paginationEl);
    }

    const pageNum = txCursorStack.length + 1;
    const hasPrev = txCursorStack.length > 0;
    const hasNext = !!txNextCursor;

    paginationEl.innerHTML = `
        <button class="btn btn-sm" onclick="prevTxPage()" ${hasPrev ? '' : 'disabled'}>
            <i class="fas fa-chevron-left"></i> Prev
        </button>
        <span style="color: var(--text-secondary);">Page ${pageNum}</span>
        <button class="btn btn-sm" onclick="nextTxPage()" ${hasNext ? '' : 'disabled'}>
            Next <i class="fas fa-chevron-right"></i>
        </button>
    `;
}

function nextTxPage() {
    if (!txNextCursor) return;
    txCursorStack.push(txCurrentBeforeSlot);
    loadTransactionHistory(currentAddress, txNextCursor);
}

function prevTxPage() {
    if (txCursorStack.length === 0) return;
    const cursor = txCursorStack.pop();
    loadTransactionHistory(currentAddress, cursor);
}

// ===== Display Transactions =====
function displayTransactions(transactions) {
    const tbody = document.getElementById('transactionsTable');

    if (transactions.length === 0) {
        tbody.innerHTML = `
            <tr><td colspan="7" class="empty-state">
                <i class="fas fa-inbox"></i>
                <div>No transactions yet</div>
                <small style="color: var(--text-muted); font-size: 0.9rem;">
                    This address hasn't made or received any transactions
                </small>
            </td></tr>`;
        return;
    }

    tbody.innerHTML = '';
    transactions.forEach(tx => {
        const row = document.createElement('tr');
        const txFrom = tx.from || tx.sender || tx.signer || null;
        const txTo = tx.to || tx.recipient || tx.receiver || null;
        const isOutgoing = txFrom === currentAddress || txFrom?.toLowerCase?.() === currentAddress?.toLowerCase?.();
        const slotRaw = tx.slot !== undefined && tx.slot !== null ? tx.slot : tx.block;
        const slot = Number(slotRaw);
        const txHash = tx.hash || tx.signature || tx.txid || tx.id || '-';
        const txType = tx.type || tx.kind || 'Unknown';
        // Display-friendly type names
        const txTypeDisplayMap = {
            'ReefStakeDeposit': 'ReefStake',
            'ReefStakeUnstake': 'ReefStake Unstake',
            'ReefStakeClaim': 'ReefStake Claim',
            'ReefStakeTransfer': 'stMOLT Transfer',
            'Shield': 'Shield',
            'Unshield': 'Unshield',
            'ShieldedTransfer': 'Shielded Transfer',
            'DeployContract': 'Deploy',
            'SetContractABI': 'Set ABI',
            'FaucetAirdrop': 'Airdrop',
            'RegisterSymbol': 'Reg. Symbol',
            'RegisterEvmAddress': 'EVM Reg.',
            'CreateAccount': 'Create Account',
            'CreateCollection': 'Collection',
            'MintNFT': 'Mint NFT',
            'TransferNFT': 'NFT Transfer',
            'ClaimUnstake': 'Claim Unstake',
            'GrantRepay': 'Grant Repay',
            'GenesisTransfer': 'Genesis',
            'GenesisMint': 'Mint',
        };
        const txTypeDisplay = txTypeDisplayMap[txType] || txType;
        const rawAmount = tx.amount ?? tx.value ?? (tx.amount_shells !== undefined ? Number(tx.amount_shells) / 1_000_000_000 : 0);
        const txAmount = Number(rawAmount || 0);
        let effectiveOutgoing = isOutgoing;
        if (txType === 'Unshield') {
            effectiveOutgoing = false;
        } else if (txType === 'Shield') {
            effectiveOutgoing = true;
        }
        const otherAddress = txType === 'ShieldedTransfer'
            ? null
            : (effectiveOutgoing ? txTo : txFrom);
        const direction = txType === 'ShieldedTransfer'
            ? 'PRIVATE'
            : (effectiveOutgoing ? 'OUT' : 'IN');
        const directionClass = txType === 'ShieldedTransfer'
            ? ''
            : (effectiveOutgoing ? 'negative' : 'positive');
        const signedPrefix = txType === 'ShieldedTransfer'
            ? ''
            : (effectiveOutgoing ? '-' : '+');
        const amountDisplay = txType === 'ShieldedTransfer'
            ? 'Hidden'
            : `${signedPrefix}${formatNumber(txAmount)} MOLT`;
        const success = tx.success !== undefined
            ? !!tx.success
            : String(tx.status || '').toLowerCase() !== 'failed';

        const blockCell = Number.isFinite(slot) && slot >= 0
            ? `<a href="block.html?slot=${slot}" class="table-link">${slot === 0 ? 'Genesis' : formatNumber(slot)}</a>`
            : '-';
        const counterpartyCell = otherAddress
            ? `<a href="address.html?address=${otherAddress}" class="table-link">${formatAddress(otherAddress)}</a>`
            : '-';

        row.innerHTML = `
            <td><a href="transaction.html?hash=${txHash}" class="table-link" title="${txHash}">${formatHash(txHash)}</a></td>
            <td>${blockCell}</td>
            <td>${formatTime(tx.timestamp)}</td>
            <td>
                <span class="badge ${directionClass}">${direction}</span>
                ${counterpartyCell}
            </td>
            <td><span class="badge">${txTypeDisplay}</span></td>
            <td class="${directionClass}">${amountDisplay}</td>
            <td>${success
                ? '<span class="badge success"><i class="fas fa-check"></i></span>'
                : '<span class="badge failed"><i class="fas fa-times"></i></span>'}</td>
        `;
        tbody.appendChild(row);
    });
}

// ===== Copy to Clipboard (explicit event) =====
function copyAddressToClipboard(elementId, event) {
    const element = document.getElementById(elementId);
    if (!element) return;
    // Prefer the full address stored in data attribute over displayed (potentially truncated) text
    const text = element.getAttribute('data-full') || element.textContent;
    navigator.clipboard.writeText(text).then(() => {
        const button = event?.target?.closest('button');
        if (button) {
            const originalHTML = button.innerHTML;
            button.innerHTML = '<i class="fas fa-check"></i> Copied!';
            button.style.background = 'var(--success)';
            button.style.color = 'white';
            button.style.borderColor = 'var(--success)';
            setTimeout(() => {
                button.innerHTML = originalHTML;
                button.style.background = '';
                button.style.color = '';
                button.style.borderColor = '';
            }, 2000);
        }
    }).catch(err => console.error('Failed to copy:', err));
}

// ===== Search =====
function setupSearch() {
    const searchInput = document.getElementById('searchInput');
    if (!searchInput) return;
    searchInput.addEventListener('keypress', async (e) => {
        if (e.key === 'Enter') {
            const query = searchInput.value.trim();
            if (!query) return;
            if (typeof navigateExplorerSearch === 'function') {
                await navigateExplorerSearch(query);
                return;
            }
            window.location.href = `address.html?address=${query}`;
        }
    });
}

// ===== Error Display =====
function showError(message) {
    const safeMessage = escapeHtml(message);
    document.querySelector('.detail-header').innerHTML = `
        <div class="breadcrumb">
            <a href="index.html"><i class="fas fa-home"></i> Home</a>
            <i class="fas fa-chevron-right"></i><span>Error</span>
        </div>
        <h1 class="detail-title"><i class="fas fa-exclamation-triangle"></i> Error</h1>
        <div class="detail-status failed"><i class="fas fa-times-circle"></i> ${safeMessage}</div>
    `;
}

// ===== Contract ABI =====
async function loadContractAbi(programId) {
    try {
        const abi = await rpcCall('getContractAbi', [programId]);
        if (!abi || abi.error || !abi.functions?.length) { hideContractAbi(); return; }
        displayContractAbi(abi);
    } catch (error) { hideContractAbi(); }
}
function hideContractAbi() {
    const card = document.getElementById('abiCard');
    if (card) card.style.display = 'none';
}
function displayContractAbi(abi) {
    let card = document.getElementById('abiCard');
    if (!card) {
        card = document.createElement('div');
        card.id = 'abiCard';
        card.className = 'detail-card';
        const container = document.querySelector('.container');
        if (container) container.appendChild(card);
    }
    card.style.display = 'block';
    const funcRows = abi.functions.map(fn => {
        const params = (fn.params || []).map(p => `${p.name}: ${p.type || p.param_type}`).join(', ');
        const ret = fn.returns ? ` → ${fn.returns.type || fn.returns.return_type}` : '';
        const badge = fn.readonly ? '<span class="badge info" style="margin-left: 4px;">view</span>' : '';
        return `<tr><td><code>${fn.name}</code>${badge}</td><td style="font-family: monospace; font-size: 0.85rem;">(${params})${ret}</td><td style="color: var(--text-secondary); font-size: 0.85rem;">${fn.description || '-'}</td></tr>`;
    }).join('');
    const eventRows = (abi.events || []).map(ev => {
        const fields = (ev.fields || []).map(f => `${f.name}: ${f.type || f.field_type}`).join(', ');
        return `<tr><td><code>${ev.name}</code></td><td>(${fields})</td><td>${ev.description || '-'}</td></tr>`;
    }).join('');
    card.innerHTML = `
        <div class="detail-card-header"><h3><i class="fas fa-file-code"></i> Contract ABI</h3>
        <span class="badge success">v${abi.version || '?'} · ${abi.functions.length} functions${abi.template ? ` · ${abi.template}` : ''}</span></div>
        <div class="detail-card-body">
            <table class="data-table"><thead><tr><th>Function</th><th>Signature</th><th>Description</th></tr></thead><tbody>${funcRows}</tbody></table>
            ${eventRows ? `<h4 style="margin-top: 1rem;"><i class="fas fa-bell"></i> Events</h4><table class="data-table"><thead><tr><th>Event</th><th>Fields</th><th>Description</th></tr></thead><tbody>${eventRows}</tbody></table>` : ''}
        </div>`;
}
