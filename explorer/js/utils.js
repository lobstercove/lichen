// MoltChain Explorer - Utilities (thin wrapper)
// Common functions (formatNumber, formatHash, formatAddress, formatMolt, formatTime,
// timeAgo, formatBytes, formatSlot, copyToClipboard, safeCopy, showToast, escapeHtml,
// readLeU64, serializeMessageBincode, formatMoltShells) are provided by ../shared/utils.js.
// This file adds explorer-specific helpers only.

function formatValidator(validator) {
    if (validator === '11111111111111111111111111111111' ||
        validator === '1111111111111111111111111111111111111111') {
        return '<span class="pill pill-info" style="background: var(--bg-secondary);">Genesis</span>';
    }
    return formatAddress(validator);
}

function resolveTxAmountShells(tx, instruction) {
    if (tx.amount_shells !== undefined) return tx.amount_shells;
    if (tx.amount !== undefined) return Math.round(tx.amount * 1_000_000_000);
    const SYSTEM_ID = typeof SYSTEM_PROGRAM_ID !== 'undefined'
        ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111';
    if (instruction && instruction.program_id === SYSTEM_ID) {
        const data = instruction.data || [];
        if (data.length >= 9) return readLeU64(data.slice(1, 9));
    }
    return null;
}

function resolveTxType(tx, instruction) {
    if (tx.type) return tx.type === 'DebtRepay' ? 'GrantRepay' : tx.type;
    const SYSTEM_ID = typeof SYSTEM_PROGRAM_ID !== 'undefined'
        ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111';
    if (instruction && instruction.program_id === SYSTEM_ID) {
        const opcode = instruction.data && instruction.data.length > 0 ? instruction.data[0] : null;
        if (opcode === 2) return 'Reward';
        if (opcode === 3) return 'GrantRepay';
        if (opcode === 4) return 'GenesisTransfer';
        if (opcode === 5) return 'GenesisMint';
        return 'Transfer';
    }
    if (instruction) return 'Contract';
    return 'Unknown';
}
