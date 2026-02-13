// MoltChain Explorer - Address Detail Page
// Displays detailed information about a specific address/account

// Inline Base58 decoder (no external dependency needed)
const bs58 = (() => {
    const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
    const BASE_MAP = new Uint8Array(256).fill(255);
    for (let i = 0; i < ALPHABET.length; i++) BASE_MAP[ALPHABET.charCodeAt(i)] = i;
    return {
        decode(str) {
            if (!str || str.length === 0) return new Uint8Array(0);
            const bytes = [0];
            for (let i = 0; i < str.length; i++) {
                const val = BASE_MAP[str.charCodeAt(i)];
                if (val === 255) throw new Error('Invalid base58 character: ' + str[i]);
                let carry = val;
                for (let j = 0; j < bytes.length; j++) {
                    carry += bytes[j] * 58;
                    bytes[j] = carry & 0xff;
                    carry >>= 8;
                }
                while (carry > 0) { bytes.push(carry & 0xff); carry >>= 8; }
            }
            let zeros = 0;
            while (zeros < str.length && str[zeros] === '1') zeros++;
            const result = new Uint8Array(zeros + bytes.length);
            for (let i = 0; i < bytes.length; i++) result[zeros + i] = bytes[bytes.length - 1 - i];
            return result;
        }
    };
})();

let currentAddress = null;
let txNextCursor = null;    // before_slot for next page
let txCursorStack = [];     // stack for previous pages
const TX_PAGE_SIZE = 50;

function getRpcUrl() {
    return typeof getExplorerRpcUrl === 'function'
        ? getExplorerRpcUrl()
        : 'http://localhost:8899';
}

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
        if (typeof bs58 === 'undefined' || !bs58?.decode) return null;
        if (typeof keccak_256 === 'undefined') return null;
        const pubkeyBytes = bs58.decode(base58Pubkey);
        const hashHex = keccak_256(pubkeyBytes);
        return '0x' + hashHex.slice(-40);
    } catch (error) {
        console.error('Error converting address to EVM:', error);
        return null;
    }
}

// ===== RPC helper =====
async function rpcCall(method, params = []) {
    const response = await fetch(getRpcUrl(), {
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
    if (!currentAddress) { showError('No address provided'); return; }
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
        await applyValidatorType(accountData);
        displayAddressData(accountData);

        // If it's a validator, load staking rewards
        if (accountData.type === 'Validator') {
            loadValidatorRewards(currentAddress);
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
        document.getElementById('registryOwner').textContent = entry.owner ? formatHash(entry.owner, 16) : '-';
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
        if (!rewards) return;

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

        const fmt = (v) => {
            const molt = typeof v === 'number' ? v / 1_000_000_000 : parseFloat(v) || 0;
            return formatNumber(molt) + ' MOLT';
        };

        document.getElementById('rewardsTotalEarned').textContent = fmt(totalEarned);
        document.getElementById('rewardsPending').textContent = fmt(pending);
        document.getElementById('rewardsClaimed').textContent = fmt(claimed);
        document.getElementById('rewardsRate').textContent = rate + ' MOLT/block';

        const blocksEl = document.getElementById('rewardsBlocksProduced');
        if (blocksEl) blocksEl.textContent = blocksProduced.toLocaleString();

        // Debt section
        const debtMolt = typeof debt === 'number' ? debt / 1_000_000_000 : parseFloat(debt) || 0;
        const earnedMolt = typeof earned === 'number' ? earned / 1_000_000_000 : parseFloat(earned) || 0;
        document.getElementById('rewardsDebt').textContent = formatNumber(debtMolt) + ' MOLT';
        document.getElementById('rewardsDebtRepaid').textContent = formatNumber(earnedMolt) + ' MOLT';

        const vestingPct = Math.min(100, Math.max(0, (typeof vesting === 'number' ? vesting * 100 : parseFloat(vesting) * 100) || 0));
        document.getElementById('rewardsVestingText').textContent = vestingPct.toFixed(1) + '%';
        document.getElementById('vestingProgressBar').style.width = vestingPct + '%';
    } catch (e) {
        console.warn('Failed to load staking rewards:', e);
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
            const container = document.querySelector('.container');
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

    document.getElementById('addressBase58').textContent = formatHash(data.base58, 16);
    document.getElementById('addressEVM').textContent = data.evm || 'Unavailable';
    document.getElementById('balanceMolt').textContent = `${formatNumber(data.molt)} MOLT`;
    document.getElementById('balanceShells').textContent = `${formatNumber(data.shells)} shells`;

    document.getElementById('spendableMolt').textContent = `${formatNumber(data.spendable)} MOLT`;
    document.getElementById('stakedMolt').textContent = `${formatNumber(data.staked)} MOLT`;
    document.getElementById('lockedMolt').textContent = `${formatNumber(data.locked)} MOLT`;

    const ownerEl = document.getElementById('ownerProgram');
    const isSystemOwner = isSystemProgramOwner(data.owner);
    ownerEl.textContent = isSystemOwner ? 'System Program' : formatHash(data.owner, 16);
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

    // Token balances
    if (data.tokens?.length > 0) {
        document.getElementById('tokensCard').style.display = 'block';
        displayTokenBalances(data.tokens);
    }

    document.getElementById('rawData').textContent = JSON.stringify(data, null, 2);
}

function displayTokenBalances(tokens) {
    const tbody = document.getElementById('tokensTable');
    tbody.innerHTML = '';
    tokens.forEach(token => {
        const row = document.createElement('tr');
        row.innerHTML = `
            <td><a href="address.html?address=${token.mint}" class="table-link">${formatHash(token.mint, 8)}</a></td>
            <td><strong>${token.symbol || 'Unknown'}</strong></td>
            <td>${formatNumber(token.ui_amount || token.balance)}</td>
            <td>${formatNumber(token.valueMolt || 0)} MOLT</td>
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

        const result = await rpcCall('getTransactionsByAddress', [address, opts]);
        const transactions = result?.transactions || (Array.isArray(result) ? result : []);
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
    txCursorStack.push(txNextCursor);
    loadTransactionHistory(currentAddress, txNextCursor);
}

function prevTxPage() {
    if (txCursorStack.length === 0) return;
    txCursorStack.pop();
    const cursor = txCursorStack.length > 0 ? txCursorStack[txCursorStack.length - 1] : undefined;
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
        const isOutgoing = tx.from === currentAddress || tx.from?.toLowerCase() === currentAddress?.toLowerCase();
        const direction = isOutgoing ? 'OUT' : 'IN';
        const directionClass = isOutgoing ? 'negative' : 'positive';
        const otherAddress = isOutgoing ? tx.to : tx.from;
        const slot = tx.slot !== undefined ? tx.slot : tx.block;

        row.innerHTML = `
            <td><a href="transaction.html?hash=${tx.hash}" class="table-link">${formatHash(tx.hash, 16)}</a></td>
            <td><a href="block.html?slot=${slot}" class="table-link">${formatNumber(slot)}</a></td>
            <td>${formatTime(tx.timestamp)}</td>
            <td>
                <span class="badge ${directionClass}">${direction}</span>
                <a href="address.html?address=${otherAddress}" class="table-link">${formatHash(otherAddress, 8)}</a>
            </td>
            <td><span class="badge">${tx.type}</span></td>
            <td class="${directionClass}">${isOutgoing ? '-' : '+'}${formatNumber(tx.amount)} MOLT</td>
            <td>${tx.success
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
    const text = element.textContent;
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
    searchInput.addEventListener('keypress', (e) => {
        if (e.key === 'Enter') {
            const query = searchInput.value.trim();
            if (!query) return;
            if (query.match(/^\d+$/)) window.location.href = `block.html?slot=${query}`;
            else if (query.length === 64) window.location.href = `transaction.html?hash=${query}`;
            else window.location.href = `address.html?address=${query}`;
        }
    });
}

// ===== Error Display =====
function showError(message) {
    document.querySelector('.detail-header').innerHTML = `
        <div class="breadcrumb">
            <a href="index.html"><i class="fas fa-home"></i> Home</a>
            <i class="fas fa-chevron-right"></i><span>Error</span>
        </div>
        <h1 class="detail-title"><i class="fas fa-exclamation-triangle"></i> Error</h1>
        <div class="detail-status failed"><i class="fas fa-times-circle"></i> ${message}</div>
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
