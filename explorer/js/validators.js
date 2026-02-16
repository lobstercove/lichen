// Validators Page Logic
// Uses RPC_URL and MoltChainRPC class from explorer.js (loaded first)

// RPC client instance created in explorer.js, reuse it here
// No need to redeclare MoltChainRPC class

// Utility Functions
function formatNumber(num) {
    return num.toLocaleString();
}

function formatMolt(shells) {
    const molt = shells / 1_000_000_000;
    const raw = molt.toLocaleString(undefined, {
        minimumFractionDigits: 0,
        maximumFractionDigits: 9,
    });
    return raw + ' MOLT';
}

// formatHash is provided by utils.js

function copyToClipboard(text) {
    navigator.clipboard.writeText(text).then(() => {
        console.log('Copied:', text);
    });
}

function trustTierFromReputation(score) {
    const rep = Number(score || 0);
    if (rep >= 950) return 'Legendary';
    if (rep >= 800) return 'Elite';
    if (rep >= 600) return 'Established';
    if (rep >= 400) return 'Trusted';
    if (rep >= 200) return 'Verified';
    if (rep >= 100) return 'Newcomer';
    return 'Probation';
}

async function loadValidators() {
    const table = document.getElementById('validatorsTable');
    if (!table) return;
    
    try {
        if (typeof rpc === 'undefined') {
            table.innerHTML = '<tr><td colspan="7" style="text-align:center; color: #FF6B6B;">RPC client not available</td></tr>';
            return;
        }

        const validatorsResult = await rpc.getValidators();
        const validators = validatorsResult && validatorsResult.validators ? validatorsResult.validators : validatorsResult;
        const validatorCount = validatorsResult && validatorsResult.count !== undefined
            ? validatorsResult.count
            : (validators ? validators.length : 0);

        if (!validators || validators.length === 0) {
            table.innerHTML = '<tr><td colspan="7" style="text-align:center; color: var(--text-muted);">No validators found</td></tr>';
            return;
        }

        const currentSlot = await rpc.getSlot();
        
        // Update stats
        document.getElementById('totalValidators').textContent = validatorCount;
        
        const totalStake = validators.reduce((sum, v) => sum + (v.stake || 0), 0);
        document.getElementById('totalStake').textContent = formatMolt(totalStake);
        const MAX_REPUTATION = 1000; // absolute scale — reputation ranges 50..1000
        
        const nameMap = typeof batchResolveMoltNames === 'function'
            ? await batchResolveMoltNames(validators.map(v => v.pubkey).filter(Boolean))
            : {};

        // Render validators
        table.innerHTML = validators.map((validator, index) => {
            const stake = validator.stake || 0;
            const reputation = validator.reputation || 0;
            const blocksProduced = validator.blocks_proposed || validator.blocks_produced || 0;
            const votingPower = totalStake > 0 ? ((stake / totalStake) * 100).toFixed(2) : '0.00';
            const lastActiveSlot = validator.last_active_slot || validator.lastActiveSlot || 0;
            const isOnline = currentSlot - lastActiveSlot <= 100;
            const reputationScale = (reputation / MAX_REPUTATION) * 100;
            const moltName = nameMap[validator.pubkey] || null;
            const tier = trustTierFromReputation(reputation);
            const addressLabel = moltName
                ? `${moltName}.molt`
                : formatHash(validator.pubkey);
            
            return `
            <tr>
                <td><span style="font-weight: 700; color: var(--text-muted);">${index + 1}</span></td>
                <td>
                    <span class="hash-short" title="${validator.pubkey}">${addressLabel}</span>
                    <i class="fas fa-copy copy-hash" onclick="copyToClipboard('${validator.pubkey}')" title="Copy address"></i>
                </td>
                <td><span style="font-family: 'JetBrains Mono', monospace; font-weight: 600;">${formatMolt(stake)}</span></td>
                <td>
                    <div style="display: flex; align-items: center; gap: 0.5rem;">
                        <div style="flex: 1; background: var(--bg-darker); height: 6px; border-radius: 3px; overflow: hidden;">
                            <div style="background: var(--primary); height: 100%; width: ${Math.min(reputationScale, 100)}%;"></div>
                        </div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.85rem;">${reputation.toFixed(4)}</span>
                        <span class="pill pill-info">${tier}</span>
                    </div>
                </td>
                <td><span class="pill pill-info">${formatNumber(blocksProduced)}</span></td>
                <td>
                    <div style="display: flex; align-items: center; gap: 0.5rem;">
                        <div style="flex: 1; background: var(--bg-darker); height: 6px; border-radius: 3px; overflow: hidden;">
                            <div style="background: var(--success); height: 100%; width: ${votingPower}%;"></div>
                        </div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.85rem;">${votingPower}%</span>
                    </div>
                </td>
                <td><span class="pill ${isOnline ? 'pill-success' : 'pill-error'}"><i class="fas fa-circle"></i> ${isOnline ? 'Online' : 'Offline'}</span></td>
            </tr>
        `}).join('');
        
    } catch (error) {
        console.error('Failed to load validators:', error);
        table.innerHTML = '<tr><td colspan="7" style="text-align:center; color: #FF6B6B;">Failed to load validators (RPC error)</td></tr>';
    }
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    loadValidators();

    let validatorPolling = null;

    const startPolling = () => {
        if (validatorPolling) return;
        validatorPolling = setInterval(loadValidators, 10000);
    };

    const stopPolling = () => {
        if (validatorPolling) {
            clearInterval(validatorPolling);
            validatorPolling = null;
        }
    };

    if (typeof ws !== 'undefined') {
        ws.onOpen(() => {
            stopPolling();
            ws.subscribe('subscribeSlots', () => loadValidators());
        });

        ws.onClose(() => {
            startPolling();
        });

        ws.connect();
        setTimeout(() => {
            if (!ws.isConnected()) {
                startPolling();
            }
        }, 2000);
    } else {
        startPolling();
    }
});
