// Transaction Detail Page - Lichen Explorer
// Uses `rpc` instance from explorer.js (loaded before this file)
// NOTE: formatHash, formatAddress, formatNumber, formatLicn, copyToClipboard,
//       escapeHtml, safeCopy, formatTimeFull, formatSpores are provided
//       by shared/utils.js (loaded before this file)
// Protocol constants (SPORES_PER_LICN, BASE_FEE_SPORES, FEE_SPLIT, ZK_COMPUTE_FEE)
// are defined in shared/utils.js

function bytesToHex(bytes) {
    if (!Array.isArray(bytes)) return '';
    return bytes.map(b => Number(b).toString(16).padStart(2, '0')).join('');
}

function normalizeHexString(value) {
    if (typeof value !== 'string' || value.length === 0) return null;
    return value.startsWith('0x') ? value : `0x${value}`;
}

function bindStaticControls() {
    document.getElementById('copyTxHashBtn')?.addEventListener('click', () => {
        copyToClipboard((document.getElementById('txHash')?.dataset?.full) || '');
    });
    document.getElementById('copyProofRootBtn')?.addEventListener('click', () => {
        copyToClipboard((document.getElementById('proofRoot')?.dataset?.full) || '');
    });
    document.getElementById('copyTxRawDataBtn')?.addEventListener('click', () => {
        copyToClipboard(document.getElementById('rawData')?.textContent || '');
    });
}

function bindSignatureCopyButtons() {
    document.querySelectorAll('#signaturesList .copy-icon[data-copy]').forEach((button) => {
        if (button.dataset.bound === '1') return;
        button.addEventListener('click', () => {
            safeCopy(button);
        });
        button.dataset.bound = '1';
    });
}

function describeSignature(signature) {
    if (!signature) {
        return { signatureText: 'N/A', copyText: 'N/A', schemeVersion: null, publicKeyText: null };
    }

    if (typeof signature === 'string') {
        return { signatureText: signature, copyText: signature, schemeVersion: null, publicKeyText: null };
    }

    if (Array.isArray(signature)) {
        const hex = '0x' + bytesToHex(signature);
        return { signatureText: hex, copyText: hex, schemeVersion: null, publicKeyText: null };
    }

    if (typeof signature === 'object') {
        return {
            signatureText: normalizeHexString(signature.sig || signature.signature) || formatHash(signature),
            copyText: JSON.stringify(signature),
            schemeVersion: signature.scheme_version ?? signature.schemeVersion ?? signature.public_key?.scheme_version ?? signature.publicKey?.schemeVersion ?? null,
            publicKeyText: normalizeHexString(
                signature.public_key?.bytes
                || signature.publicKey?.bytes
                || signature.public_key_bytes
                || signature.publicKeyBytes
            ),
        };
    }

    const fallback = String(signature);
    return { signatureText: fallback, copyText: fallback, schemeVersion: null, publicKeyText: null };
}

function readU64LE(bytes, offset) {
    if (!Array.isArray(bytes) || bytes.length < offset + 8) return null;
    let out = 0n;
    for (let i = 0; i < 8; i++) {
        out |= BigInt(bytes[offset + i]) << BigInt(i * 8);
    }
    if (out > BigInt(Number.MAX_SAFE_INTEGER)) return null;
    return Number(out);
}

function decodeShieldedInstruction(inst) {
    const SYSTEM_ID = typeof SYSTEM_PROGRAM_ID !== 'undefined'
        ? SYSTEM_PROGRAM_ID
        : '11111111111111111111111111111111';
    if (!inst || inst.program_id !== SYSTEM_ID || !Array.isArray(inst.data) || inst.data.length === 0) {
        return null;
    }

    const opcode = inst.data[0];
    if (opcode === 23 && inst.data.length >= 169) {
        const amountSpores = readU64LE(inst.data, 1);
        return {
            label: 'Shield',
            rows: [
                ['Opcode', '23 (Shield)'],
                ['Amount', amountSpores != null ? `${formatLicn(amountSpores)} (${formatSpores(amountSpores)})` : 'Unknown'],
                ['Commitment', `<code>0x${bytesToHex(inst.data.slice(9, 41))}</code>`],
                ['Proof', `${inst.data.length - 41} bytes`],
            ],
        };
    }

    if (opcode === 24 && inst.data.length >= 233) {
        const amountSpores = readU64LE(inst.data, 1);
        return {
            label: 'Unshield',
            rows: [
                ['Opcode', '24 (Unshield)'],
                ['Amount', amountSpores != null ? `${formatLicn(amountSpores)} (${formatSpores(amountSpores)})` : 'Unknown'],
                ['Nullifier', `<code>0x${bytesToHex(inst.data.slice(9, 41))}</code>`],
                ['Merkle Root', `<code>0x${bytesToHex(inst.data.slice(41, 73))}</code>`],
                ['Recipient Input (Fr)', `<code>0x${bytesToHex(inst.data.slice(73, 105))}</code>`],
                ['Proof', `${inst.data.length - 105} bytes`],
            ],
        };
    }

    if (opcode === 25 && inst.data.length >= 289) {
        return {
            label: 'ShieldedTransfer',
            rows: [
                ['Opcode', '25 (ShieldedTransfer)'],
                ['Nullifier A', `<code>0x${bytesToHex(inst.data.slice(1, 33))}</code>`],
                ['Nullifier B', `<code>0x${bytesToHex(inst.data.slice(33, 65))}</code>`],
                ['Output Commitment C', `<code>0x${bytesToHex(inst.data.slice(65, 97))}</code>`],
                ['Output Commitment D', `<code>0x${bytesToHex(inst.data.slice(97, 129))}</code>`],
                ['Merkle Root', `<code>0x${bytesToHex(inst.data.slice(129, 161))}</code>`],
                ['Proof', `${inst.data.length - 161} bytes`],
            ],
        };
    }

    return null;
}

function isShieldedType(typeRaw) {
    return typeRaw === 'Shield' || typeRaw === 'Unshield' || typeRaw === 'ShieldedTransfer';
}

function redactShieldedTransaction(tx) {
    if (!tx || typeof tx !== 'object') return tx;
    const clone = JSON.parse(JSON.stringify(tx));
    clone.from = null;
    clone.to = null;
    if (clone.message && Array.isArray(clone.message.instructions)) {
        clone.message.instructions = clone.message.instructions.map((inst) => {
            const instClone = { ...inst };
            if (Array.isArray(instClone.accounts) && instClone.accounts.length > 0) {
                instClone.accounts = [`<redacted:${instClone.accounts.length}>`];
            }
            return instClone;
        });
    }
    return clone;
}

// Get transaction hash from URL
function getTxHash() {
    const params = new URLSearchParams(window.location.search);
    return params.get('sig') || params.get('tx') || params.get('hash') || params.get('signature');
}

// Load and display transaction
async function loadTransaction() {
    const txHash = getTxHash();

    if (!txHash) {
        document.getElementById('txHash').textContent = 'Invalid';
        document.getElementById('txStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> Transaction not found';
        return;
    }

    // Detect airdrop signatures (format: airdrop-<timestamp_ms>)
    if (txHash.startsWith('airdrop-')) {
        displayAirdrop(txHash);
        return;
    }

    if (!rpc) {
        document.getElementById('txHash').textContent = txHash;
        document.getElementById('txStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> RPC unavailable';
        return;
    }

    const tx = await rpc.getTransaction(txHash);
    if (!tx) {
        document.getElementById('txHash').textContent = txHash;
        document.getElementById('txStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> Transaction not found';
        return;
    }

    // Update page
    await displayTransaction(tx);
}

function upsertParticipants(from, to, nameMap = {}, opts = {}) {
    const {
        fromTitle = 'From',
        toTitle = 'To',
        fromOverride = null,
        toOverride = null,
        feePayer = null,
    } = opts;

    const grid = document.querySelector('.detail-card .detail-card-body .detail-grid');
    const amountEl = document.getElementById('detailAmount');
    const amountRow = amountEl ? amountEl.closest('.detail-row') : null;
    if (!grid || !amountRow) return;

    let fromRow = document.getElementById('detailFromRow');
    let toRow = document.getElementById('detailToRow');

    if (!fromRow) {
        fromRow = document.createElement('div');
        fromRow.className = 'detail-row';
        fromRow.id = 'detailFromRow';
        amountRow.insertAdjacentElement('afterend', fromRow);
    }
    if (!toRow) {
        toRow = document.createElement('div');
        toRow.className = 'detail-row';
        toRow.id = 'detailToRow';
        fromRow.insertAdjacentElement('afterend', toRow);
    }

    const fromLabel = (typeof formatAddressWithLichenName === 'function' && from)
        ? formatAddressWithLichenName(from, nameMap[from], { includeAddressInLabel: true })
        : (from || 'N/A');
    const toLabel = (typeof formatAddressWithLichenName === 'function' && to)
        ? formatAddressWithLichenName(to, nameMap[to], { includeAddressInLabel: true })
        : (to || 'N/A');
    const fromIsAddress = typeof isLikelyLicnAddress === 'function' ? isLikelyLicnAddress(from) : false;
    const toIsAddress = typeof isLikelyLicnAddress === 'function' ? isLikelyLicnAddress(to) : false;

    const fromValue = fromOverride ?? (from ? (fromIsAddress ? `<a href="address.html?address=${from}" class="detail-link">${fromLabel}</a>` : fromLabel) : 'N/A');
    const toValue = toOverride ?? (to ? (toIsAddress ? `<a href="address.html?address=${to}" class="detail-link">${toLabel}</a>` : toLabel) : 'N/A');

    fromRow.innerHTML = `
        <div class="detail-label">${fromTitle}</div>
        <div class="detail-value">${fromValue}</div>
    `;
    toRow.innerHTML = `
        <div class="detail-label">${toTitle}</div>
        <div class="detail-value">${toValue}</div>
    `;

    let feePayerRow = document.getElementById('detailFeePayerRow');
    if (feePayer) {
        if (!feePayerRow) {
            feePayerRow = document.createElement('div');
            feePayerRow.className = 'detail-row';
            feePayerRow.id = 'detailFeePayerRow';
            toRow.insertAdjacentElement('afterend', feePayerRow);
        }
        const feePayerLabel = (typeof formatAddressWithLichenName === 'function')
            ? formatAddressWithLichenName(feePayer, nameMap[feePayer], { includeAddressInLabel: true })
            : feePayer;
        feePayerRow.innerHTML = `
            <div class="detail-label">Fee Payer</div>
            <div class="detail-value"><a href="address.html?address=${feePayer}" class="detail-link">${feePayerLabel}</a></div>
        `;
    } else if (feePayerRow) {
        feePayerRow.remove();
    }
}

// Display airdrop details (airdrops are off-chain treasury operations, not indexed transactions)
async function displayAirdrop(txHash) {
    const params = new URLSearchParams(window.location.search);
    let recipient = params.get('to') || null;
    let amountLicn = parseFloat(params.get('amount')) || null;

    // Try fetching from faucet backend API (has airdrop history)
    if (!recipient || !amountLicn) {
        try {
            const faucetUrl = (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.faucet) ? LICHEN_CONFIG.faucet : 'http://localhost:9100';
            const resp = await fetch(`${faucetUrl}/faucet/airdrop/${encodeURIComponent(txHash)}`);
            if (resp.ok) {
                const record = await resp.json();
                if (!recipient && record.recipient) recipient = record.recipient;
                if (!amountLicn && record.amount_licn) amountLicn = record.amount_licn;
            }
        } catch (e) { /* faucet API unavailable */ }
    }

    if (!recipient) recipient = 'Unknown';
    if (!amountLicn) amountLicn = null;

    const timestampMs = parseInt(txHash.replace('airdrop-', ''), 10);
    const timestampSec = Math.floor(timestampMs / 1000);
    const amountDisplay = amountLicn
        ? formatLicn(Math.round(amountLicn * SPORES_PER_LICN))
        : 'Unknown';

    // Header
    document.getElementById('txHash').textContent = formatHash(txHash);
    document.getElementById('txHash').dataset.full = txHash;
    const statusBadge = '<span class="badge badge-success"><i class="fas fa-check-circle"></i> Success</span>';
    document.getElementById('txStatus').innerHTML = statusBadge;
    document.getElementById('detailStatus').innerHTML = statusBadge;

    // Block — airdrops have no block
    document.getElementById('blockLink').textContent = 'N/A (off-chain)';
    document.getElementById('detailBlockLink').textContent = 'N/A (off-chain)';

    // Timestamp
    document.getElementById('txTimestamp').textContent = formatTimeFull(timestampSec);
    document.getElementById('detailTimestamp').textContent = formatTimeFull(timestampSec);

    // Type
    document.getElementById('txType').textContent = 'Airdrop';
    document.getElementById('detailType').textContent = 'Airdrop';

    // Amount
    document.getElementById('txAmount').textContent = amountDisplay;
    document.getElementById('detailAmount').textContent = amountDisplay;

    let nameMap = {};
    try {
        if (typeof batchResolveLichenNames === 'function') {
            nameMap = await Promise.race([
                batchResolveLichenNames([recipient]),
                new Promise(r => setTimeout(() => r({}), 3000))
            ]);
        }
    } catch (e) { /* name resolution unavailable */ }
    upsertParticipants('Treasury', recipient, nameMap);

    // Fee — airdrops are fee-free
    document.getElementById('txFee').textContent = '0 LICN';
    document.getElementById('totalFee').textContent = '0 LICN (fee-free airdrop)';
    document.getElementById('baseFee').textContent = '0 LICN (airdrop — no fee)';
    document.getElementById('priorityFee').textContent = '0 LICN (no priority fee)';
    document.getElementById('computeBudget').textContent = 'N/A';
    document.getElementById('computeUnitPrice').textContent = 'N/A';
    document.getElementById('computeUnits').textContent = 'N/A';
    document.getElementById('feeNote').textContent = 'Airdrops are direct treasury operations with no transaction fees';
    document.getElementById('feeBurned').textContent = '0 LICN';
    document.getElementById('feeProducer').textContent = '0 LICN';
    document.getElementById('feeVoters').textContent = '0 LICN';
    document.getElementById('feeCommunity').textContent = '0 LICN';

    // Recent blockhash
    document.getElementById('recentBlockhash').textContent = 'N/A';

    // Instructions — show airdrop details instead
    document.getElementById('instructionCount').textContent = '1';
    const recipientDisplay = (typeof formatAddressWithLichenName === 'function' && recipient !== 'Unknown')
        ? formatAddressWithLichenName(recipient, nameMap[recipient], { includeAddressInLabel: true })
        : recipient;
    const recipientLink = recipient !== 'Unknown'
        ? `<a href="address.html?address=${recipient}" class="detail-link">${recipientDisplay}</a>`
        : 'Unknown';
    document.getElementById('instructionsList').innerHTML = `
        <div class="instruction-item">
            <div class="instruction-header">
                <strong><i class="fas fa-parachute-box"></i> Airdrop Details</strong>
            </div>
            <div class="detail-grid">
                <div class="detail-row">
                    <div class="detail-label">Type</div>
                    <div class="detail-value">Testnet Faucet Airdrop</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Source</div>
                    <div class="detail-value">Treasury</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Recipient</div>
                    <div class="detail-value">${recipientLink}</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Amount</div>
                    <div class="detail-value">${amountLicn} LICN</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Note</div>
                    <div class="detail-value">Airdrop is a treasury-funded transfer processed through consensus. No fees are charged.</div>
                </div>
            </div>
        </div>
    `;

    // Signatures
    document.getElementById('signatureCount').textContent = '1';
    document.getElementById('signaturesList').innerHTML = '<div class="empty-state"><i class="fas fa-info-circle"></i> Airdrop transaction signed by treasury</div>';

    // Raw data
    document.getElementById('rawData').textContent = JSON.stringify({
        type: 'Airdrop',
        signature: txHash,
        recipient: recipient,
        amount_licn: amountLicn,
        amount_spores: amountLicn ? Math.round(amountLicn * 1_000_000_000) : null,
        timestamp: timestampMs,
        source: 'Treasury',
        fee: 0,
        note: 'Treasury-funded airdrop via requestAirdrop RPC'
    }, null, 2);
}

async function displayTransaction(tx) {
    const hash = tx.signature;
    const status = tx.status || 'Success';
    const typeRaw = tx.type || 'Unknown';
    // Display-friendly type names
    const typeDisplayMap = {
        'MossStakeDeposit': 'MossStake Deposit',
        'MossStakeUnstake': 'MossStake Unstake',
        'MossStakeClaim': 'MossStake Claim',
        'MossStakeTransfer': 'MossStake Transfer',
        'Shield': 'Shield',
        'Unshield': 'Unshield',
        'ShieldedTransfer': 'Shielded Transfer',
        'DeployContract': 'Deploy Contract',
        'SetContractABI': 'Set Contract ABI',
        'FaucetAirdrop': 'Faucet Airdrop',
        'RegisterSymbol': 'Register Symbol',
        'RegisterEvmAddress': 'EVM Registration',
        'CreateAccount': 'Create Account',
        'CreateCollection': 'Create Collection',
        'MintNFT': 'Mint NFT',
        'TransferNFT': 'Transfer NFT',
        'ClaimUnstake': 'Claim Unstake',
        'GrantRepay': 'Grant Repay',
        'GenesisTransfer': 'Genesis Transfer',
        'GenesisMint': 'Genesis Mint',
    };
    const type = typeDisplayMap[typeRaw] || typeRaw;
    const shieldedTx = isShieldedType(typeRaw);
    const slot = tx.slot;
    const timestamp = tx.block_time || Math.floor(Date.now() / 1000);
    const fee = tx.fee_spores !== undefined ? tx.fee_spores : (tx.fee ?? BASE_FEE_SPORES);
    const amountSpores = tx.amount_spores !== undefined
        ? tx.amount_spores
        : Math.round((tx.amount || 0) * SPORES_PER_LICN);
    const amountDisplay = typeRaw === 'ShieldedTransfer'
        ? 'Hidden'
        : tx.token_symbol
            ? formatNumber(tx.token_amount || 0) + ' ' + tx.token_symbol
            : amountSpores > 0
                ? formatLicn(amountSpores)
                : '-';
    const recentBlockhash = tx.message.recent_blockhash || tx.message.blockhash;
    const instructions = tx.message.instructions || [];
    const signatures = tx.signatures || [];
    const isFeeFree = fee === 0;
    const firstInstructionAccounts = instructions[0]?.accounts || [];
    let fromAddress = tx.from || firstInstructionAccounts[0] || null;
    let toAddress = tx.to || firstInstructionAccounts[1] || null;

    if (typeRaw === 'Shield') {
        toAddress = null;
    } else if (typeRaw === 'Unshield') {
        fromAddress = null;
        toAddress = tx.to || firstInstructionAccounts[0] || null;
    } else if (typeRaw === 'ShieldedTransfer') {
        fromAddress = null;
        toAddress = null;
    }

    const instructionAccounts = instructions.flatMap(inst => inst.accounts || []);
    const nameMap = typeof batchResolveLichenNames === 'function'
        ? await batchResolveLichenNames([fromAddress, toAddress, ...instructionAccounts].filter(Boolean))
        : {};

    // Header
    document.getElementById('txHash').textContent = formatHash(hash);
    document.getElementById('txHash').dataset.full = hash;

    // Status
    const statusBadge = status === 'Success'
        ? '<span class="badge badge-success"><i class="fas fa-check-circle"></i> Success</span>'
        : '<span class="badge badge-error"><i class="fas fa-times-circle"></i> Failed</span>';

    document.getElementById('txStatus').innerHTML = statusBadge;
    document.getElementById('detailStatus').innerHTML = statusBadge;

    // Block link
    if (slot !== undefined && slot !== null) {
        document.getElementById('blockLink').textContent = '#' + formatNumber(slot);
        document.getElementById('blockLink').href = `block.html?slot=${slot}`;
        document.getElementById('detailBlockLink').textContent = '#' + formatNumber(slot);
        document.getElementById('detailBlockLink').href = `block.html?slot=${slot}`;
    } else {
        document.getElementById('blockLink').textContent = '-';
        document.getElementById('detailBlockLink').textContent = '-';
    }

    // Timestamp
    document.getElementById('txTimestamp').textContent = formatTimeFull(timestamp);
    document.getElementById('detailTimestamp').textContent = formatTimeFull(timestamp);

    document.getElementById('txType').textContent = type;
    document.getElementById('detailType').textContent = type;
    document.getElementById('txAmount').textContent = amountDisplay;
    document.getElementById('detailAmount').textContent = amountDisplay;
    if (typeRaw === 'Shield') {
        upsertParticipants(fromAddress, toAddress, nameMap, {
            toOverride: 'Shielded Pool (private)',
        });
    } else if (typeRaw === 'Unshield') {
        upsertParticipants(fromAddress, toAddress, nameMap, {
            fromOverride: 'Shielded Pool (private)',
        });
    } else if (typeRaw === 'ShieldedTransfer') {
        upsertParticipants(fromAddress, toAddress, nameMap, {
            fromOverride: 'Shielded Note(s) (private)',
            toOverride: 'Shielded Note(s) (private)',
            feePayer: firstInstructionAccounts[0] || null,
        });
    } else {
        upsertParticipants(fromAddress, toAddress, nameMap);
    }

    // Fee details
    const baseFeeSpores = tx.base_fee_spores || fee;
    const priorityFeeSpores = tx.priority_fee_spores || 0;
    const computeBudget = tx.compute_budget || 200000;
    const computeUnitPrice = tx.compute_unit_price || 0;

    document.getElementById('txFee').textContent = formatLicn(fee);
    document.getElementById('totalFee').textContent = formatLicn(fee) + ' (' + formatSpores(fee) + ')';
    document.getElementById('computeUnits').textContent = formatNumber(tx.compute_units || 0) + ' CU';
    document.getElementById('baseFee').textContent = isFeeFree
        ? '0.000000000 LICN (fee-free system tx)'
        : formatLicn(baseFeeSpores) + ' (' + formatSpores(baseFeeSpores) + ')';
    document.getElementById('priorityFee').textContent = priorityFeeSpores > 0
        ? formatLicn(priorityFeeSpores) + ' (' + formatSpores(priorityFeeSpores) + ')'
        : '0 LICN (no priority fee)';
    document.getElementById('computeBudget').textContent = formatNumber(computeBudget) + ' CU';
    document.getElementById('computeUnitPrice').textContent = computeUnitPrice > 0
        ? formatNumber(computeUnitPrice) + ' μspores/CU'
        : '0 μspores/CU (default)';
    const zkTypeMap = { 'Shield': 'shield', 'Unshield': 'unshield', 'ShieldedTransfer': 'transfer' };
    const zkComputeFee = zkTypeMap[typeRaw] ? (ZK_COMPUTE_FEE[zkTypeMap[typeRaw]] || 0) : 0;
    document.getElementById('feeNote').textContent = isFeeFree
        ? (typeRaw === 'FaucetAirdrop'
            ? 'Faucet airdrops are fee-free treasury operations'
            : 'System transactions are fee-free')
        : priorityFeeSpores > 0
            ? `Base fee ${formatSpores(baseFeeSpores)} + priority fee ${formatSpores(priorityFeeSpores)} (${formatNumber(computeUnitPrice)} μspores/CU × ${formatNumber(computeBudget)} CU)`
            : zkComputeFee > 0
                ? `Fee includes base fee (${formatSpores(BASE_FEE_SPORES)}) + shielded verification compute (${formatSpores(zkComputeFee)}).`
                : 'Fee split is applied to this transaction';

    // Fee split: base portion uses standard 40/30/10/10/10, priority portion uses 50/50 burn/producer
    const baseBurned = Math.floor(baseFeeSpores * FEE_SPLIT.burned);
    const baseProducer = Math.floor(baseFeeSpores * FEE_SPLIT.producer);
    const baseVoters = Math.floor(baseFeeSpores * FEE_SPLIT.voters);
    const baseValidatorPool = Math.floor(baseFeeSpores * FEE_SPLIT.treasury);
    const baseCommunity = baseFeeSpores - baseBurned - baseProducer - baseVoters - baseValidatorPool;
    const priorityBurned = Math.floor(priorityFeeSpores / 2);
    const priorityProducer = priorityFeeSpores - priorityBurned;

    const feeBurned = baseBurned + priorityBurned;
    const feeProducer = baseProducer + priorityProducer;
    const feeVoters = baseVoters;
    const feeValidatorPool = baseValidatorPool;
    const feeCommunity = baseCommunity;

    const pct = (v) => Math.round(v * 100) + '%';
    document.getElementById('feeBurnedLabel').textContent = 'Fee Burned (' + pct(FEE_SPLIT.burned) + ')';
    document.getElementById('feeProducerLabel').textContent = 'Fee to Producer (' + pct(FEE_SPLIT.producer) + ')';
    document.getElementById('feeVotersLabel').textContent = 'Fee to Voters (' + pct(FEE_SPLIT.voters) + ')';
    document.getElementById('feeValidatorPoolLabel').textContent = 'Fee to Validator Pool (' + pct(FEE_SPLIT.treasury) + ')';
    document.getElementById('feeCommunityLabel').textContent = 'Fee to Community (' + pct(FEE_SPLIT.community) + ')';

    document.getElementById('feeBurned').textContent = formatLicn(feeBurned) + ' (' + pct(FEE_SPLIT.burned) + ')';
    document.getElementById('feeProducer').textContent = formatLicn(feeProducer) + ' (' + pct(FEE_SPLIT.producer) + ')';
    document.getElementById('feeVoters').textContent = formatLicn(feeVoters) + ' (' + pct(FEE_SPLIT.voters) + ')';
    document.getElementById('feeValidatorPool').textContent = formatLicn(feeValidatorPool) + ' (' + pct(FEE_SPLIT.treasury) + ')';
    document.getElementById('feeCommunity').textContent = formatLicn(feeCommunity) + ' (' + pct(FEE_SPLIT.community) + ')';

    // Recent blockhash
    document.getElementById('recentBlockhash').textContent = formatHash(recentBlockhash);
    document.getElementById('recentBlockhash').dataset.full = recentBlockhash;

    // Instructions
    document.getElementById('instructionCount').textContent = instructions.length;
    const instructionsList = document.getElementById('instructionsList');

    if (instructions.length === 0) {
        instructionsList.innerHTML = '<div class="empty-state"><i class="fas fa-inbox"></i> No instructions</div>';
    } else {
        instructionsList.innerHTML = instructions.map((inst, idx) => {
            const shielded = decodeShieldedInstruction(inst);
            const shieldedRows = shielded
                ? shielded.rows.map(([k, v]) => `
                    <div class="detail-row">
                        <div class="detail-label">${k}</div>
                        <div class="detail-value">${v}</div>
                    </div>
                `).join('')
                : '';

            return `
            <div class="instruction-item">
                <div class="instruction-header">
                    <strong>Instruction #${idx + 1}${shielded ? ` · ${shielded.label}` : ''}</strong>
                </div>
                <div class="detail-grid">
                    <div class="detail-row">
                        <div class="detail-label">Program ID</div>
                        <div class="detail-value">
                            <code title="${inst.program_id}">${formatHash(inst.program_id)}</code>
                            <a href="address.html?address=${inst.program_id}" class="detail-link">
                                <i class="fas fa-external-link-alt"></i>
                            </a>
                        </div>
                    </div>
                    <div class="detail-row">
                        <div class="detail-label">Accounts</div>
                        <div class="detail-value">
                            ${shieldedTx
                    ? '<div><code>Redacted for shielded transaction privacy</code></div>'
                    : inst.accounts.map(acc => {
                        const accountDisplay = (typeof formatAddressWithLichenName === 'function')
                            ? formatAddressWithLichenName(acc, nameMap[acc], { includeAddressInLabel: true })
                            : acc;
                        return `
                                    <div>
                                        <code>${accountDisplay}</code>
                                        <a href="address.html?address=${acc}" class="detail-link">
                                            <i class="fas fa-external-link-alt"></i>
                                        </a>
                                    </div>
                                    `;
                    }).join('')}
                        </div>
                    </div>
                    <div class="detail-row">
                        <div class="detail-label">Data</div>
                        <div class="detail-value">
                            <code>${inst.data.length} bytes: [${inst.data.slice(0, 20).join(', ')}${inst.data.length > 20 ? '...' : ''}]</code>
                        </div>
                    </div>
                    ${shieldedRows}
                </div>
            </div>
        `;
        }).join('');
    }

    // Signatures
    document.getElementById('signatureCount').textContent = signatures.length;
    const signaturesList = document.getElementById('signaturesList');

    if (signatures.length === 0) {
        signaturesList.innerHTML = '<div class="empty-state"><i class="fas fa-inbox"></i> No signatures</div>';
    } else {
        signaturesList.innerHTML = signatures.map((sig, idx) => {
            const signatureInfo = describeSignature(sig);
            const safeSignature = typeof escapeHtml === 'function'
                ? escapeHtml(signatureInfo.signatureText)
                : signatureInfo.signatureText;
            const safeCopy = typeof escapeHtml === 'function'
                ? escapeHtml(signatureInfo.copyText)
                : signatureInfo.copyText;
            const schemeRow = signatureInfo.schemeVersion != null ? `
                    <div class="detail-row">
                        <div class="detail-label">Scheme</div>
                        <div class="detail-value">ML-DSA-65 v${signatureInfo.schemeVersion}</div>
                    </div>
                ` : '';
            const publicKeyRow = signatureInfo.publicKeyText ? `
                    <div class="detail-row">
                        <div class="detail-label">Verifying Key</div>
                        <div class="detail-value"><code title="${signatureInfo.publicKeyText}">${formatHash(signatureInfo.publicKeyText)}</code></div>
                    </div>
                ` : '';
            return `
                <div class="signature-item">
                    <div class="detail-grid">
                        <div class="detail-row">
                            <div class="detail-label">Signature #${idx + 1}</div>
                            <div class="detail-value">
                                <code title="${safeSignature}">${formatHash(signatureInfo.signatureText)}</code>
                                <button class="copy-icon" data-copy="${safeCopy}">
                                    <i class="fas fa-copy"></i>
                                </button>
                            </div>
                        </div>
                        ${schemeRow}
                        ${publicKeyRow}
                    </div>
                </div>
            `;
        }).join('');
    }

    bindSignatureCopyButtons();

    // Raw data
    const rawTx = shieldedTx ? redactShieldedTransaction(tx) : tx;
    document.getElementById('rawData').textContent = JSON.stringify(rawTx, null, 2);

    // Merkle Inclusion Proof — fetch asynchronously (non-blocking)
    if (hash && rpc && slot !== undefined && slot !== null) {
        loadMerkleProof(hash);
    }
}

async function loadMerkleProof(signature) {
    try {
        const resp = await rpc.call('getTransactionProof', [signature]);
        if (!resp || resp.error || !resp.root) return;

        const card = document.getElementById('merkleProofCard');
        if (!card) return;
        card.style.display = '';

        document.getElementById('proofRoot').textContent = formatHash(resp.root);
        document.getElementById('proofRoot').dataset.full = resp.root;
        document.getElementById('proofTxIndex').textContent = resp.tx_index;
        const proof = resp.proof || [];
        document.getElementById('proofDepth').textContent = proof.length === 0
            ? '0 (single-transaction block)'
            : proof.length + ' level' + (proof.length > 1 ? 's' : '');

        if (proof.length === 0) {
            document.getElementById('proofPath').innerHTML = '<em>Root equals leaf hash (only transaction in block)</em>';
        } else {
            document.getElementById('proofPath').innerHTML = proof.map((step, i) => {
                const dir = step.direction === 'left' ? '← left' : '→ right';
                return `<div style="margin-bottom:4px;"><code>${formatHash(step.hash)}</code> <span class="badge badge-secondary" style="font-size:0.75rem;">${dir}</span></div>`;
            }).join('');
        }

        // Show verification badge
        const badge = document.getElementById('proofVerifyBadge');
        if (badge) {
            badge.style.display = '';
            badge.className = 'badge badge-success';
            badge.innerHTML = '<i class="fas fa-check-circle"></i> Proof available';
        }
    } catch (e) {
        // Proof not available — silently skip
    }
}

// Search functionality
document.getElementById('searchInput')?.addEventListener('keypress', async (e) => {
    if (e.key === 'Enter') {
        const query = e.target.value.trim();
        if (query) {
            if (typeof navigateExplorerSearch === 'function') {
                await navigateExplorerSearch(query);
                return;
            }
            window.location.href = `address.html?address=${query}`;
        }
    }
});

// Initialize
window.addEventListener('DOMContentLoaded', () => {
    bindStaticControls();
    loadTransaction();
});
