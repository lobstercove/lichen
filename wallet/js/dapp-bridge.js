(function initWalletDappBridge() {
    const params = new URLSearchParams(window.location.search);
    if (params.get('bridge') !== 'popup' || !window.opener) {
        return;
    }

    const RETURN_TO_URL = params.get('returnTo');
    const REQUESTED_NETWORK = normalizeRequestedNetwork(params.get('network'));

    const REQUEST_TARGET = 'LICHEN_WEB_WALLET_BRIDGE';
    const RESPONSE_TARGET = 'LICHEN_WEB_WALLET_RESPONSE';
    const EVENT_TARGET = 'LICHEN_WEB_WALLET_EVENT';
    const APPROVED_ORIGINS_KEY = 'lichen_wallet_dapp_permissions';
    const APPROVED_ORIGINS_META_KEY = 'lichen_wallet_dapp_permissions_meta';
    const APPROVED_ORIGIN_TTL_MS = 30 * 24 * 60 * 60 * 1000;
    const REQUEST_TTL_MS = 10 * 60 * 1000;
    const FINALIZED_RESPONSE_TTL_MS = 5 * 60 * 1000;
    const LOOP_INTERVAL_MS = 500;
    const CLOSE_DELAY_MS = 150;
    const HINT_ID = 'walletDappBridgeHint';
    const SIGNING_METHODS = new Set(['licn_signMessage', 'licn_signTransaction', 'licn_sendTransaction']);
    const INTERACTIVE_METHODS = new Set(['licn_requestAccounts', 'licn_signMessage', 'licn_signTransaction', 'licn_sendTransaction']);
    const pendingRequests = new Map();
    const finalizedResponses = new Map();

    let activeApprovalId = null;
    let sessionOrigin = null;
    let sessionSource = null;
    let lastSnapshot = null;
    let requestedNetworkApplied = !REQUESTED_NETWORK;
    let requestedNetworkPromise = null;

    function escapeBridgeHtml(value) {
        return String(value ?? '')
            .replaceAll('&', '&amp;')
            .replaceAll('<', '&lt;')
            .replaceAll('>', '&gt;')
            .replaceAll('"', '&quot;')
            .replaceAll("'", '&#39;');
    }

    function loadJson(key, fallback) {
        try {
            const raw = localStorage.getItem(key);
            if (!raw) return fallback;
            const parsed = JSON.parse(raw);
            return parsed && typeof parsed === 'object' ? parsed : fallback;
        } catch {
            return fallback;
        }
    }

    function saveJson(key, value) {
        localStorage.setItem(key, JSON.stringify(value));
    }

    function isLoopbackOrigin(origin) {
        try {
            const url = new URL(origin);
            return url.hostname === 'localhost' || url.hostname === '127.0.0.1';
        } catch {
            return false;
        }
    }

    function getReturnToOrigin() {
        if (!RETURN_TO_URL) {
            return '';
        }

        try {
            return new URL(RETURN_TO_URL, window.location.origin).origin;
        } catch {
            return '';
        }
    }

    function buildTrustedOrigins() {
        const origins = new Set();
        const appKeys = ['dex', 'programs', 'marketplace', 'explorer', 'developers', 'website', 'monitoring', 'faucet'];

        for (const appKey of appKeys) {
            try {
                const appUrl = typeof LICHEN_CONFIG !== 'undefined' ? LICHEN_CONFIG[appKey] : null;
                if (appUrl) {
                    origins.add(new URL(appUrl, window.location.origin).origin);
                }
            } catch {
                // Ignore malformed or unavailable app URLs.
            }
        }

        const returnToOrigin = getReturnToOrigin();
        if (returnToOrigin && isLoopbackOrigin(window.location.origin) && isLoopbackOrigin(returnToOrigin)) {
            origins.add(returnToOrigin);
        }

        return origins;
    }

    const TRUSTED_ORIGINS = buildTrustedOrigins();

    function isTrustedOrigin(origin) {
        return TRUSTED_ORIGINS.has(origin);
    }

    function normalizeMethod(method) {
        const key = String(method || '').trim();
        const aliases = {
            licn_connect: 'licn_requestAccounts',
            licn_getPermissions: 'licn_permissions',
            wallet_getPermissions: 'licn_permissions',
            wallet_revokePermissions: 'licn_disconnect',
            licn_get_provider_state: 'licn_getProviderState',
            licn_request_accounts: 'licn_requestAccounts',
            licn_sign_message: 'licn_signMessage',
            licn_sign_transaction: 'licn_signTransaction',
            licn_send_transaction: 'licn_sendTransaction',
            personal_sign: 'licn_signMessage',
            eth_sign: 'licn_signMessage',
            eth_signTransaction: 'licn_signTransaction',
            eth_sendTransaction: 'licn_sendTransaction',
        };
        return aliases[key] || key;
    }

    function normalizeRequestedNetwork(value) {
        const network = String(value || '').trim();
        if (!network) {
            return '';
        }
        const allowedNetworks = new Set(['mainnet', 'testnet', 'local-testnet', 'local-mainnet']);
        return allowedNetworks.has(network) ? network : '';
    }

    function getRuntimeSelectedNetwork() {
        if (typeof getSelectedNetwork === 'function') {
            return getSelectedNetwork();
        }
        return walletState?.network || 'testnet';
    }

    function getCurrentNetwork() {
        if (REQUESTED_NETWORK && !requestedNetworkApplied) {
            return REQUESTED_NETWORK;
        }
        return getRuntimeSelectedNetwork();
    }

    async function ensureRequestedNetwork() {
        if (!REQUESTED_NETWORK || requestedNetworkApplied) {
            return true;
        }

        if (getRuntimeSelectedNetwork() === REQUESTED_NETWORK) {
            requestedNetworkApplied = true;
            return true;
        }

        try {
            localStorage.setItem('lichen_wallet_network', REQUESTED_NETWORK);
        } catch {
            // Ignore localStorage failures.
        }

        if (walletState && typeof walletState === 'object') {
            walletState.network = REQUESTED_NETWORK;
        }

        const networkSelect = document.getElementById('networkSelect');
        if (networkSelect && networkSelect.querySelector(`option[value="${REQUESTED_NETWORK}"]`)) {
            networkSelect.value = REQUESTED_NETWORK;
        }

        if (typeof switchNetwork !== 'function') {
            return false;
        }

        if (!requestedNetworkPromise) {
            requestedNetworkPromise = Promise.resolve()
                .then(() => switchNetwork(REQUESTED_NETWORK))
                .then(() => {
                    requestedNetworkApplied = true;
                    requestedNetworkPromise = null;
                    return true;
                })
                .catch((error) => {
                    requestedNetworkPromise = null;
                    console.warn('Failed to switch wallet bridge network:', error);
                    return false;
                });
        }

        return requestedNetworkPromise;
    }

    function getCurrentChainId(network) {
        const value = String(network || getCurrentNetwork()).trim();
        if (value === 'mainnet') return '0x2710';
        if (value === 'testnet') return '0x2711';
        return '0x539';
    }

    function getActiveWalletSafe() {
        try {
            return typeof getActiveWallet === 'function' ? getActiveWallet() : null;
        } catch {
            return null;
        }
    }

    function hasWallet() {
        return Array.isArray(walletState?.wallets) && walletState.wallets.length > 0;
    }

    function readApprovedOriginsRaw() {
        const value = loadJson(APPROVED_ORIGINS_KEY, []);
        return Array.isArray(value) ? value : [];
    }

    function readApprovedOriginsMeta() {
        return loadJson(APPROVED_ORIGINS_META_KEY, {});
    }

    function pruneApprovedOrigins(now = Date.now()) {
        const origins = readApprovedOriginsRaw();
        const meta = readApprovedOriginsMeta();
        const activeOrigins = [];
        const nextMeta = { ...meta };
        let changed = false;

        for (const entry of origins) {
            const origin = String(entry || '').trim();
            if (!origin) {
                changed = true;
                continue;
            }
            const expiresAt = Number(nextMeta[origin] || 0);
            if (expiresAt > 0 && expiresAt <= now) {
                delete nextMeta[origin];
                changed = true;
                continue;
            }
            if (!activeOrigins.includes(origin)) {
                activeOrigins.push(origin);
            } else {
                changed = true;
            }
        }

        for (const origin of Object.keys(nextMeta)) {
            if (!activeOrigins.includes(origin)) {
                delete nextMeta[origin];
                changed = true;
            }
        }

        if (changed) {
            saveJson(APPROVED_ORIGINS_KEY, activeOrigins);
            saveJson(APPROVED_ORIGINS_META_KEY, nextMeta);
        }

        return { origins: activeOrigins, meta: nextMeta };
    }

    function isOriginApproved(origin) {
        if (!origin) return false;
        const { origins } = pruneApprovedOrigins();
        return origins.includes(origin);
    }

    function approveOrigin(origin) {
        if (!origin) return;
        const { origins, meta } = pruneApprovedOrigins();
        if (!origins.includes(origin)) {
            origins.push(origin);
        }
        meta[origin] = Date.now() + APPROVED_ORIGIN_TTL_MS;
        saveJson(APPROVED_ORIGINS_KEY, origins);
        saveJson(APPROVED_ORIGINS_META_KEY, meta);
    }

    function revokeOrigin(origin) {
        if (!origin) return;
        const { origins, meta } = pruneApprovedOrigins();
        const nextOrigins = origins.filter((entry) => entry !== origin);
        delete meta[origin];
        saveJson(APPROVED_ORIGINS_KEY, nextOrigins);
        saveJson(APPROVED_ORIGINS_META_KEY, meta);
    }

    function buildPermissionsResult(origin, state) {
        if (!state.connected || !state.accounts.length) {
            return [];
        }

        return [{
            parentCapability: 'eth_accounts',
            caveats: [{
                type: 'filterResponse',
                value: state.accounts,
            }],
            date: Date.now(),
            invoker: origin,
        }];
    }

    function buildProviderState(origin) {
        const wallet = getActiveWalletSafe();
        const approved = isOriginApproved(origin);
        const network = getCurrentNetwork();
        const isLocked = Boolean(walletState?.isLocked);
        const connected = approved && Boolean(wallet);
        const activeAddress = connected && !isLocked ? String(wallet.address || '').trim() : '';

        return {
            connected,
            origin,
            chainId: getCurrentChainId(network),
            network,
            activeAddress,
            accounts: activeAddress ? [activeAddress] : [],
            isLocked,
            version: '0.1.0',
            providerType: 'web-wallet',
        };
    }

    function postToSource(source, origin, payload) {
        if (!source || !origin) return;
        try {
            source.postMessage(payload, origin);
        } catch {
            // Ignore cross-window messaging failures.
        }
    }

    function sendResponse(request, response) {
        if (!request || request.responded) return;

        request.responded = true;
        request.respondedAt = Date.now();
        pendingRequests.delete(request.id);
        finalizedResponses.set(request.id, {
            response,
            respondedAt: request.respondedAt,
            source: request.source,
            origin: request.origin,
        });

        postToSource(request.source, request.origin, {
            target: RESPONSE_TARGET,
            id: request.id,
            response,
        });
    }

    function schedulePopupClose() {
        setTimeout(() => {
            try {
                window.close();
            } catch {
                // Ignore popup close failures.
            }
        }, CLOSE_DELAY_MS);
    }

    function emitProviderEvent(eventName, payload) {
        if (!sessionSource || !sessionOrigin) return;
        postToSource(sessionSource, sessionOrigin, {
            target: EVENT_TARGET,
            event: eventName,
            payload,
        });
    }

    function pruneFinalizedResponses(now = Date.now()) {
        for (const [requestId, entry] of finalizedResponses.entries()) {
            if (!entry || now - Number(entry.respondedAt || 0) > FINALIZED_RESPONSE_TTL_MS) {
                finalizedResponses.delete(requestId);
            }
        }
    }

    function upsertPendingRequest(event) {
        const { id, payload } = event.data || {};
        if (!id || !payload || typeof payload !== 'object') {
            return null;
        }

        const finalized = finalizedResponses.get(id);
        if (finalized) {
            postToSource(event.source, event.origin, {
                target: RESPONSE_TARGET,
                id,
                response: finalized.response,
            });
            return null;
        }

        const existing = pendingRequests.get(id);
        if (existing) {
            existing.origin = event.origin;
            existing.source = event.source;
            existing.payload = payload;
            return existing;
        }

        const request = {
            id,
            payload,
            origin: event.origin,
            source: event.source,
            createdAt: Date.now(),
            processing: false,
            responded: false,
            noticeState: '',
        };

        pendingRequests.set(id, request);
        return request;
    }

    function shouldAwaitWalletSetup(request) {
        return INTERACTIVE_METHODS.has(normalizeMethod(request?.payload?.method));
    }

    function decodeBase64Json(value) {
        const raw = atob(String(value || ''));
        const bytes = Uint8Array.from(raw, (char) => char.charCodeAt(0));
        return JSON.parse(new TextDecoder().decode(bytes));
    }

    function encodeBase64Json(value) {
        const bytes = new TextEncoder().encode(JSON.stringify(value));
        let binary = '';
        for (const byte of bytes) {
            binary += String.fromCharCode(byte);
        }
        return btoa(binary);
    }

    function getParams(payload) {
        if (Array.isArray(payload?.params)) {
            return payload.params;
        }
        return [];
    }

    function getTransactionFromPayload(payload) {
        const params = getParams(payload);
        if (params.length) {
            const first = params[0];
            if (first && typeof first === 'object' && Object.prototype.hasOwnProperty.call(first, 'transaction')) {
                return first.transaction;
            }
            return first;
        }
        if (payload && typeof payload === 'object' && Object.prototype.hasOwnProperty.call(payload, 'transaction')) {
            return payload.transaction;
        }
        return null;
    }

    function normalizeTransactionObject(payload) {
        const incoming = getTransactionFromPayload(payload);
        if (!incoming) {
            throw new Error('Missing transaction payload');
        }
        if (typeof incoming === 'string') {
            return decodeBase64Json(incoming);
        }
        if (typeof incoming === 'object') {
            return incoming;
        }
        throw new Error('Unsupported transaction payload');
    }

    function normalizeMessageBytes(payload) {
        const params = getParams(payload);
        const first = params.length ? params[0] : payload?.message;
        const message = first && typeof first === 'object' && Object.prototype.hasOwnProperty.call(first, 'message')
            ? first.message
            : first;

        if (message instanceof Uint8Array) {
            return message;
        }
        if (Array.isArray(message)) {
            return Uint8Array.from(message);
        }
        if (typeof message === 'string') {
            if (/^0x[0-9a-f]+$/i.test(message)) {
                return LichenCrypto.hexToBytes(message.slice(2));
            }
            return new TextEncoder().encode(message);
        }

        throw new Error('Unsupported message payload');
    }

    function shortenValue(value) {
        const text = String(value || '').trim();
        if (!text) return '—';
        if (text.length <= 20) return text;
        return `${text.slice(0, 10)}...${text.slice(-8)}`;
    }

    function detailRowHtml(label, value, options = {}) {
        const displayValue = String(value ?? '').trim() || '—';
        const valueClass = options.mono ? ' class="mono"' : '';
        return `<div style="display:flex;justify-content:space-between;align-items:flex-start;flex-wrap:wrap;gap:0.35rem 1rem;margin:0.3rem 0;"><strong style="flex:0 0 auto;">${escapeBridgeHtml(label)}</strong><span${valueClass} style="flex:1 1 12rem;min-width:0;text-align:right;overflow-wrap:anywhere;word-break:break-word;">${escapeBridgeHtml(displayValue)}</span></div>`;
    }

    function describeProgram(programId) {
        if (!Array.isArray(programId) && !(programId instanceof Uint8Array)) {
            return 'Unknown Program';
        }
        const bytes = programId instanceof Uint8Array ? programId : Uint8Array.from(programId);
        if (bytes.length === 32 && bytes.every((byte) => byte === 0)) {
            return 'System Program';
        }
        if (bytes.length === 32 && bytes.every((byte) => byte === 0xff)) {
            return 'Contract Program';
        }
        try {
            return shortenValue(bs58.encode(bytes));
        } catch {
            return `Program (${bytes.length} bytes)`;
        }
    }

    function transactionSummaryHtml(txObject) {
        const message = txObject?.message || {};
        const instructions = Array.isArray(message.instructions) ? message.instructions : [];
        const firstInstruction = instructions[0] || null;
        const rows = [
            ['Instructions', String(instructions.length)],
            ['Primary program', describeProgram(firstInstruction?.program_id)],
            ['Blockhash', shortenValue(message.blockhash || message.recent_blockhash || '')],
        ];

        if (Number.isFinite(message.compute_budget)) {
            rows.push(['Compute budget', String(message.compute_budget)]);
        }
        if (Number.isFinite(message.compute_unit_price)) {
            rows.push(['Compute unit price', String(message.compute_unit_price)]);
        }

        return rows.map(([label, value]) => detailRowHtml(label, value, { mono: true })).join('');
    }

    function approvalMessageHtml(request, providerState) {
        const origin = String(request.origin || 'unknown');
        const wallet = getActiveWalletSafe();
        const walletName = String(wallet?.name || 'Active wallet');
        const walletAddress = String(providerState.activeAddress || wallet?.address || '—');
        const network = String(providerState.network || getCurrentNetwork());
        const method = normalizeMethod(request?.payload?.method);

        let intro = 'Approve this request from the connected application.';
        if (method === 'licn_requestAccounts') {
            intro = 'Allow this site to view your current wallet address.';
        } else if (method === 'licn_sendTransaction') {
            intro = 'Review the transaction details below. Your wallet password is required before signing and broadcasting.';
        } else if (method === 'licn_signTransaction') {
            intro = 'Review the transaction details below. Your wallet password is required before signing.';
        } else if (method === 'licn_signMessage') {
            intro = 'Review the signing request below. Your wallet password is required before signing.';
        }

        let details = '';
        if (method === 'licn_requestAccounts') {
            details = `
                ${detailRowHtml('Wallet', walletName)}
                ${detailRowHtml('Address', walletAddress, { mono: true })}
                ${detailRowHtml('Network', network)}
            `;
        } else if (method === 'licn_signMessage') {
            const messageBytes = normalizeMessageBytes(request.payload);
            details = `
                ${detailRowHtml('Wallet', walletName)}
                ${detailRowHtml('Network', network)}
                ${detailRowHtml('Message bytes', String(messageBytes.length))}
            `;
        } else {
            details = transactionSummaryHtml(normalizeTransactionObject(request.payload));
        }

        return `
            <div style="display:grid;gap:0.7rem;">
                <p style="margin:0;color:var(--text-muted);line-height:1.55;">${intro}</p>
                <div style="padding:0.85rem 1rem;border:1px solid rgba(255,255,255,0.12);border-radius:14px;background:rgba(255,255,255,0.03);display:grid;gap:0.2rem;">
                    ${detailRowHtml('Site', origin, { mono: true })}
                    ${details}
                </div>
            </div>
        `;
    }

    async function requestApproval(request) {
        const method = normalizeMethod(request?.payload?.method);
        const providerState = buildProviderState(request.origin);
        const needsPassword = SIGNING_METHODS.has(method);
        const title = method === 'licn_requestAccounts'
            ? 'Connect Site'
            : method === 'licn_signMessage'
                ? 'Approve Message Signature'
                : method === 'licn_signTransaction'
                    ? 'Approve Transaction Signature'
                    : 'Approve Transaction';

        const values = await showPasswordModal({
            title,
            message: approvalMessageHtml(request, providerState),
            icon: method === 'licn_requestAccounts' ? 'fas fa-link' : 'fas fa-shield-alt',
            confirmText: method === 'licn_requestAccounts' ? 'Connect' : 'Approve',
            fields: needsPassword
                ? [{
                    id: 'password',
                    label: 'Wallet Password',
                    type: 'password',
                    placeholder: 'Enter password to sign',
                }]
                : [],
        });

        return values;
    }

    async function finalizeSignMessage(request, password) {
        const wallet = getActiveWalletSafe();
        if (!wallet?.encryptedKey) {
            throw new Error('No active wallet available for signing');
        }

        let privateKeyHex;
        try {
            privateKeyHex = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password);
            const messageBytes = normalizeMessageBytes(request.payload);
            const signature = await LichenCrypto.signTransaction(privateKeyHex, messageBytes);
            return {
                ok: true,
                result: {
                    signature: signature.sig,
                    pqSignature: signature,
                },
            };
        } finally {
            if (typeof privateKeyHex === 'string') {
                privateKeyHex = '0'.repeat(privateKeyHex.length);
            }
        }
    }

    async function finalizeSignTransaction(request, password) {
        const wallet = getActiveWalletSafe();
        if (!wallet?.encryptedKey) {
            throw new Error('No active wallet available for signing');
        }

        let privateKeyHex;
        try {
            privateKeyHex = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password);
            const txObject = normalizeTransactionObject(request.payload);
            const messageBytes = serializeMessageBincode(txObject.message || {});
            const signature = await LichenCrypto.signTransaction(privateKeyHex, messageBytes);
            const signedTransaction = {
                ...txObject,
                signatures: Array.isArray(txObject.signatures)
                    ? [...txObject.signatures, signature]
                    : [signature],
            };

            return {
                ok: true,
                result: {
                    signature: signature.sig,
                    pqSignature: signature,
                    signedTransaction,
                    signedTransactionBase64: encodeBase64Json(signedTransaction),
                },
            };
        } finally {
            if (typeof privateKeyHex === 'string') {
                privateKeyHex = '0'.repeat(privateKeyHex.length);
            }
        }
    }

    async function finalizeSendTransaction(request, password) {
        const signResult = await finalizeSignTransaction(request, password);
        const txHash = await rpc.sendTransaction(signResult.result.signedTransactionBase64);
        return {
            ok: true,
            result: {
                txHash,
                signature: signResult.result.signature,
                pqSignature: signResult.result.pqSignature,
                signedTransaction: signResult.result.signedTransaction,
                signedTransactionBase64: signResult.result.signedTransactionBase64,
            },
        };
    }

    function hintMessageForRequest(request) {
        const origin = escapeBridgeHtml(request?.origin || 'a connected site');
        if (!hasWallet()) {
            return `Finish creating or importing a wallet to continue the request from <span class="mono" style="overflow-wrap:anywhere;word-break:break-word;">${origin}</span>.`;
        }
        if (walletState?.isLocked) {
            return `Unlock your wallet to continue the request from <span class="mono" style="overflow-wrap:anywhere;word-break:break-word;">${origin}</span>.`;
        }
        return '';
    }

    function removeHint() {
        document.getElementById(HINT_ID)?.remove();
    }

    function renderHint() {
        const request = Array.from(pendingRequests.values())
            .filter((entry) => !entry.responded && shouldAwaitWalletSetup(entry))
            .sort((a, b) => Number(a.createdAt || 0) - Number(b.createdAt || 0))[0];

        if (!request || (!walletState?.isLocked && hasWallet())) {
            removeHint();
            return;
        }

        const container = document.querySelector('.unlock-card')
            || document.querySelector('.welcome-container')
            || document.getElementById('walletDashboard');
        if (!container) return;

        const message = hintMessageForRequest(request);
        if (!message) {
            removeHint();
            return;
        }

        let hint = document.getElementById(HINT_ID);
        if (!hint) {
            hint = document.createElement('div');
            hint.id = HINT_ID;
            hint.style.cssText = [
                'margin: 0 0 16px',
                'padding: 12px 14px',
                'border-radius: 14px',
                'border: 1px solid rgba(0, 201, 219, 0.25)',
                'background: rgba(0, 201, 219, 0.08)',
                'color: var(--text-primary)',
                'font-size: 0.9rem',
                'line-height: 1.5',
            ].join(';');
            container.prepend(hint);
        }
        hint.innerHTML = `<i class="fas fa-link" style="margin-right:0.5rem;color:var(--teal-primary);"></i>${message}`;
    }

    function noteRequestState(request, nextState) {
        if (!request || request.noticeState === nextState) {
            return;
        }
        request.noticeState = nextState;
        if (nextState === 'missing-wallet') {
            showToast('Finish creating or importing a wallet to continue the dApp request.');
        } else if (nextState === 'locked') {
            showToast('Unlock your wallet to continue the dApp request.');
        }
    }

    async function processRequest(request) {
        if (!request || request.processing || request.responded) {
            return;
        }

        if (Date.now() - Number(request.createdAt || 0) > REQUEST_TTL_MS) {
            sendResponse(request, { ok: false, error: 'Wallet request timed out' });
            return;
        }

        const method = normalizeMethod(request?.payload?.method);
        const networkReady = await ensureRequestedNetwork();
        if (REQUESTED_NETWORK && !networkReady && INTERACTIVE_METHODS.has(method)) {
            return;
        }
        const providerState = buildProviderState(request.origin);

        if (method === 'licn_getProviderState') {
            sendResponse(request, { ok: true, result: providerState });
            return;
        }

        if (method === 'licn_isConnected') {
            sendResponse(request, { ok: true, result: providerState.connected });
            return;
        }

        if (method === 'licn_chainId') {
            sendResponse(request, { ok: true, result: providerState.chainId });
            return;
        }

        if (method === 'licn_network') {
            sendResponse(request, {
                ok: true,
                result: { network: providerState.network, chainId: providerState.chainId },
            });
            return;
        }

        if (method === 'licn_version') {
            sendResponse(request, { ok: true, result: providerState.version });
            return;
        }

        if (method === 'licn_accounts') {
            sendResponse(request, { ok: true, result: providerState.accounts });
            return;
        }

        if (method === 'licn_permissions') {
            sendResponse(request, { ok: true, result: buildPermissionsResult(request.origin, providerState) });
            return;
        }

        if (method === 'licn_disconnect') {
            revokeOrigin(request.origin);
            sendResponse(request, { ok: true, result: true });
            lastSnapshot = buildProviderState(request.origin);
            emitProviderEvent('disconnect', { origin: request.origin });
            emitProviderEvent('accountsChanged', []);
            return;
        }

        if (!shouldAwaitWalletSetup(request)) {
            sendResponse(request, { ok: false, error: `Unsupported provider method: ${method || 'unknown'}` });
            return;
        }

        if (!hasWallet()) {
            noteRequestState(request, 'missing-wallet');
            return;
        }

        if (walletState?.isLocked) {
            noteRequestState(request, 'locked');
            return;
        }

        if (activeApprovalId && activeApprovalId !== request.id) {
            return;
        }

        activeApprovalId = request.id;
        request.processing = true;
        request.noticeState = '';

        try {
            if (method === 'licn_requestAccounts' && providerState.connected && providerState.accounts.length) {
                sendResponse(request, { ok: true, result: providerState.accounts });
                schedulePopupClose();
                return;
            }

            const approvalValues = await requestApproval(request);
            if (!approvalValues) {
                sendResponse(request, { ok: false, error: 'User rejected request' });
                schedulePopupClose();
                return;
            }

            if (SIGNING_METHODS.has(method) && !approvalValues.password) {
                sendResponse(request, { ok: false, error: 'Password required for signing' });
                schedulePopupClose();
                return;
            }

            approveOrigin(request.origin);

            if (method === 'licn_requestAccounts') {
                const connectedState = buildProviderState(request.origin);
                if (!connectedState.accounts.length) {
                    sendResponse(request, { ok: false, error: 'No active wallet available' });
                    schedulePopupClose();
                    return;
                }
                sendResponse(request, { ok: true, result: connectedState.accounts });
                schedulePopupClose();
                return;
            }

            if (method === 'licn_signMessage') {
                sendResponse(request, await finalizeSignMessage(request, approvalValues.password));
                schedulePopupClose();
                return;
            }

            if (method === 'licn_signTransaction') {
                sendResponse(request, await finalizeSignTransaction(request, approvalValues.password));
                schedulePopupClose();
                return;
            }

            if (method === 'licn_sendTransaction') {
                sendResponse(request, await finalizeSendTransaction(request, approvalValues.password));
                schedulePopupClose();
                return;
            }

            sendResponse(request, { ok: false, error: `Unsupported provider method: ${method || 'unknown'}` });
        } catch (error) {
            sendResponse(request, { ok: false, error: error?.message || String(error) });
        } finally {
            request.processing = false;
            if (activeApprovalId === request.id) {
                activeApprovalId = null;
            }
        }
    }

    function emitSnapshotChanges() {
        if (!sessionOrigin || !sessionSource || sessionSource.closed) {
            return;
        }

        const nextSnapshot = buildProviderState(sessionOrigin);
        if (!lastSnapshot) {
            lastSnapshot = nextSnapshot;
            return;
        }

        if (lastSnapshot.chainId !== nextSnapshot.chainId) {
            emitProviderEvent('chainChanged', nextSnapshot.chainId);
        }

        const previousAccounts = JSON.stringify(lastSnapshot.accounts || []);
        const nextAccounts = JSON.stringify(nextSnapshot.accounts || []);
        if (previousAccounts !== nextAccounts) {
            emitProviderEvent('accountsChanged', nextSnapshot.accounts || []);
        }

        if (!lastSnapshot.connected && nextSnapshot.connected && nextSnapshot.accounts.length) {
            emitProviderEvent('connect', nextSnapshot);
        }

        if (lastSnapshot.connected && !nextSnapshot.connected) {
            emitProviderEvent('disconnect', { origin: sessionOrigin });
        }

        lastSnapshot = nextSnapshot;
    }

    function driveBridgeLoop() {
        pruneApprovedOrigins();
        pruneFinalizedResponses();
        void ensureRequestedNetwork();
        renderHint();
        emitSnapshotChanges();

        if (activeApprovalId) {
            return;
        }

        const nextRequest = Array.from(pendingRequests.values())
            .filter((entry) => !entry.responded)
            .sort((a, b) => Number(a.createdAt || 0) - Number(b.createdAt || 0))[0];

        if (nextRequest) {
            void processRequest(nextRequest);
        }
    }

    window.addEventListener('message', (event) => {
        if (!event.data || event.data.target !== REQUEST_TARGET) {
            return;
        }

        if (!isTrustedOrigin(event.origin)) {
            postToSource(event.source, event.origin, {
                target: RESPONSE_TARGET,
                id: event.data.id,
                response: { ok: false, error: 'Untrusted dApp origin' },
            });
            return;
        }

        if (window.opener && event.source !== window.opener) {
            return;
        }

        sessionOrigin = event.origin;
        sessionSource = event.source;

        const request = upsertPendingRequest(event);
        if (!request) {
            return;
        }

        if (!lastSnapshot) {
            lastSnapshot = buildProviderState(sessionOrigin);
        }

        driveBridgeLoop();
    });

    window.addEventListener('beforeunload', () => {
        removeHint();
    });

    setInterval(driveBridgeLoop, LOOP_INTERVAL_MS);
})();