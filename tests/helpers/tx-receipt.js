'use strict';

function receiptFailure(receipt) {
    if (!receipt || typeof receipt !== 'object') return 'transaction receipt is missing';
    const error = receipt.error ?? receipt.err ?? null;
    if (error !== null && error !== undefined && error !== '') {
        return typeof error === 'string' ? error : JSON.stringify(error);
    }
    if (receipt.success === false || String(receipt.status || '').toLowerCase() === 'failed') {
        return 'transaction receipt reports failure';
    }
    return null;
}

function requireSuccessfulReceipt(receipt, signature = '') {
    const failure = receiptFailure(receipt);
    if (failure) {
        const suffix = signature ? ` (${signature})` : '';
        throw new Error(`transaction failed${suffix}: ${failure}`);
    }
    return receipt;
}

async function waitForSuccessfulTransaction(
    rpc,
    signature,
    timeoutMs = 60_000,
    pollMs = 250,
) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        try {
            const receipt = await rpc('getTransaction', [signature]);
            if (receipt) return requireSuccessfulReceipt(receipt, signature);
        } catch (error) {
            const message = String(error?.message || error || '');
            if (!/not found|not indexed|unknown transaction/i.test(message)) throw error;
        }
        await new Promise((resolve) => setTimeout(resolve, pollMs));
    }
    throw new Error(`transaction receipt timed out (${signature})`);
}

module.exports = {
    receiptFailure,
    requireSuccessfulReceipt,
    waitForSuccessfulTransaction,
};
