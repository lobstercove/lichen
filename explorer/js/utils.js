// Lichen Explorer - Utilities (thin wrapper)
// Common functions (formatNumber, formatHash, formatAddress, formatLicn, formatTime,
// timeAgo, formatBytes, formatSlot, copyToClipboard, safeCopy, showToast, escapeHtml,
// readLeU64, serializeMessageBincode, formatLicnSpores) are provided by ../shared/utils.js.
// This file adds explorer-specific helpers only.

function formatValidator(validator) {
    if (validator === '11111111111111111111111111111111' ||
        validator === '1111111111111111111111111111111111111111') {
        return '<span class="pill pill-info" style="background: var(--bg-secondary);">Genesis</span>';
    }
    return formatAddress(validator);
}

function resolveTxAmountSpores(tx, instruction) {
    if (tx.amount_spores !== undefined) return tx.amount_spores;
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
    if (tx.type) {
        return typeof normalizeTxType === 'function' ? normalizeTxType(tx.type) : tx.type;
    }
    const SYSTEM_ID = typeof SYSTEM_PROGRAM_ID !== 'undefined'
        ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111';
    if (instruction && instruction.program_id === SYSTEM_ID) {
        const opcode = instruction.data && instruction.data.length > 0 ? instruction.data[0] : null;
        const OPCODE_MAP = {
            1: 'CreateAccount', 2: 'Reward', 3: 'GrantRepay',
            4: 'GenesisTransfer', 5: 'GenesisMint', 6: 'CreateCollection',
            7: 'MintNFT', 8: 'TransferNFT', 9: 'Stake', 10: 'Unstake',
            11: 'ClaimUnstake', 12: 'RegisterEvmAddress',
            13: 'MossStakeDeposit', 14: 'MossStakeUnstake',
            15: 'MossStakeClaim', 16: 'MossStakeTransfer',
            17: 'DeployContract', 18: 'SetContractABI',
            19: 'FaucetAirdrop', 20: 'RegisterSymbol',
            21: 'ProposeGovernedTransfer', 22: 'ApproveGovernedTransfer',
            23: 'Shield', 24: 'Unshield', 25: 'ShieldedTransfer',
            26: 'RegisterValidator', 27: 'SlashValidator',
            28: 'DurableNonce', 29: 'GovernanceParamChange',
            30: 'OracleAttestation', 31: 'DeregisterValidator',
        };
        return OPCODE_MAP[opcode] || 'Transfer';
    }
    if (instruction) return 'Contract';
    return 'Unknown';
}
