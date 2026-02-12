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

function formatHash(hash, full = false) {
    if (!hash) return 'N/A';
    if (full) return hash;
    return hash.substring(0, 8) + '...' + hash.substring(hash.length - 6);
}

function copyToClipboard(text) {
    navigator.clipboard.writeText(text).then(() => {
        console.log('Copied:', text);
    });
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
        const maxReputation = validators.reduce((max, v) => Math.max(max, v.reputation || 0), 0);
        
        // Render validators
        table.innerHTML = validators.map((validator, index) => {
            const stake = validator.stake || 0;
            const reputation = validator.reputation || 0;
            const blocksProduced = validator.blocks_proposed || validator.blocks_produced || 0;
            const votingPower = totalStake > 0 ? ((stake / totalStake) * 100).toFixed(2) : '0.00';
            const lastActiveSlot = validator.last_active_slot || validator.lastActiveSlot || 0;
            const isOnline = currentSlot - lastActiveSlot <= 100;
            const reputationScale = maxReputation > 0 ? (reputation / maxReputation) * 100 : 0;
            
            return `
            <tr>
                <td><span style="font-weight: 700; color: var(--text-muted);">${index + 1}</span></td>
                <td>
                    <span class="hash-short">${formatHash(validator.pubkey)}</span>
                    <i class="fas fa-copy copy-hash" onclick="copyToClipboard('${validator.pubkey}')" title="Copy address"></i>
                </td>
                <td><span style="font-family: 'JetBrains Mono', monospace; font-weight: 600;">${formatMolt(stake)}</span></td>
                <td>
                    <div style="display: flex; align-items: center; gap: 0.5rem;">
                        <div style="flex: 1; background: var(--bg-darker); height: 6px; border-radius: 3px; overflow: hidden;">
                            <div style="background: var(--primary); height: 100%; width: ${Math.min(reputationScale, 100)}%;"></div>
                        </div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.85rem;">${reputation.toFixed(4)}</span>
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
