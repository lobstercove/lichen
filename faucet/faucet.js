// Lichen Faucet JavaScript
// Connects to the Lichen faucet backend (Rust/axum; default port 9100, Docker uses 9101 via PORT env)

const FAUCET_API =
    (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG?.faucet) ||
    (typeof window !== 'undefined' && window.LICHEN_CONFIG?.faucet) ||
    'http://localhost:9100';
const EXPLORER_BASE =
    (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG?.explorer) ||
    (typeof window !== 'undefined' && window.LICHEN_CONFIG?.explorer) ||
    '../explorer';
let LICN_PER_REQUEST = 10; // default; overwritten by /faucet/config
const FAUCET_BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function sanitizeFaucetBase58(value) {
    return String(value || '').split('').filter((char) => FAUCET_BASE58_ALPHABET.indexOf(char) !== -1).join('');
}

function sanitizeFaucetInteger(value) {
    return String(value || '').replace(/\D/g, '');
}

function applyFaucetInputGuards() {
    const addressInput = document.getElementById('address');
    if (addressInput && addressInput.dataset.faucetAddressGuarded !== '1') {
        addressInput.dataset.faucetAddressGuarded = '1';
        addressInput.setAttribute('autocomplete', 'off');
        addressInput.setAttribute('spellcheck', 'false');
        addressInput.addEventListener('input', () => {
            const sanitized = sanitizeFaucetBase58(addressInput.value);
            if (sanitized !== addressInput.value) addressInput.value = sanitized;
        });
        addressInput.addEventListener('paste', () => requestAnimationFrame(() => {
            addressInput.value = sanitizeFaucetBase58(addressInput.value);
        }));
    }

    const captchaInput = document.getElementById('captcha');
    if (captchaInput && captchaInput.dataset.faucetNumberGuarded !== '1') {
        captchaInput.dataset.faucetNumberGuarded = '1';
        captchaInput.setAttribute('inputmode', 'numeric');
        captchaInput.addEventListener('keydown', (event) => {
            if (event.ctrlKey || event.metaKey || event.altKey) return;
            if (event.key === 'e' || event.key === 'E' || event.key === '+' || event.key === '-' || event.key === '.') {
                event.preventDefault();
            }
        });
        captchaInput.addEventListener('input', () => {
            const sanitized = sanitizeFaucetInteger(captchaInput.value);
            if (sanitized !== captchaInput.value) captchaInput.value = sanitized;
        });
        captchaInput.addEventListener('paste', () => requestAnimationFrame(() => {
            captchaInput.value = sanitizeFaucetInteger(captchaInput.value);
        }));
    }
}

function formatCooldown(seconds) {
    const value = Number(seconds || 0);
    if (value < 60) return `${value}s`;
    if (value % 60 === 0) return `${value / 60} min`;
    return `${Math.floor(value / 60)}m ${value % 60}s`;
}

function formatElapsedTime(timestampMs) {
    const ts = Number(timestampMs || 0);
    if (!ts) return 'Unknown';
    const elapsedSeconds = Math.max(0, Math.floor((Date.now() - ts) / 1000));
    if (elapsedSeconds < 60) return 'Just now';
    if (elapsedSeconds < 3600) return `${Math.floor(elapsedSeconds / 60)} min ago`;
    return `${Math.floor(elapsedSeconds / 3600)}h ago`;
}

function renderRecentRequests(records) {
    const tbody = document.getElementById('recentRequests');
    if (!tbody) return;

    if (!Array.isArray(records) || records.length === 0) {
        tbody.innerHTML = `
            <tr>
                <td colspan="4" style="text-align: center; color: var(--text-muted);">
                    <i class="fas fa-inbox"></i> No recent requests yet
                </td>
            </tr>
        `;
        return;
    }

    tbody.innerHTML = '';
    records.slice(0, 10).forEach((record) => {
        const recipient = String(record.recipient || '');
        const amount = Number(record.amount_licn || 0);
        const shortAddress = escapeHtml(`${recipient.slice(0, 8)}...${recipient.slice(-4)}`);
        const safeAmount = escapeHtml(String(amount));

        const row = document.createElement('tr');
        row.innerHTML = `
            <td><code>${shortAddress}</code></td>
            <td>${safeAmount} LICN</td>
            <td>${formatElapsedTime(record.timestamp_ms)}</td>
            <td><span class="badge badge-success">Completed</span></td>
        `;
        tbody.appendChild(row);
    });
}

async function loadRecentRequests() {
    try {
        const response = await fetch(`${FAUCET_API}/faucet/airdrops?limit=10`);
        if (!response.ok) return;
        const records = await response.json();
        renderRecentRequests(records);
    } catch (e) {
        // Ignore history preload failures.
    }
}

// Generate random captcha
function generateCaptcha() {
    const num1 = Math.floor(Math.random() * 10) + 1;
    const num2 = Math.floor(Math.random() * 10) + 1;
    document.getElementById('num1').textContent = num1;
    document.getElementById('num2').textContent = num2;
    return num1 + num2;
}

// Initialize captcha
let captchaAnswer = generateCaptcha();

// Mobile nav toggle
const navToggle = document.getElementById('navToggle');
const navMenu = document.querySelector('.nav-menu');
if (navToggle && navMenu) {
    navToggle.addEventListener('click', () => {
        navMenu.classList.toggle('active');
        navToggle.classList.toggle('active');
    });
}

// Update stats display
async function updateStats() {
    try {
        const resp = await fetch(`${FAUCET_API}/faucet/config`);
        if (!resp.ok) return null;
        const data = await resp.json();

        const perRequestEl = document.getElementById('statPerRequest');
        const cooldownEl = document.getElementById('statCooldown');
        const dailyLimitEl = document.getElementById('statDailyLimit');

        if (data.max_per_request) LICN_PER_REQUEST = Number(data.max_per_request);
        if (perRequestEl) perRequestEl.textContent = `${LICN_PER_REQUEST} LICN`;
        if (cooldownEl) cooldownEl.textContent = formatCooldown(data.cooldown_seconds || 0);
        if (dailyLimitEl) dailyLimitEl.textContent = `${Number(data.daily_limit_per_ip || 0)} LICN / IP`;

        const balanceEl = document.getElementById('statFaucetBalance');
        if (balanceEl) {
            try {
                const statusResp = await fetch(`${FAUCET_API}/faucet/status`);
                if (statusResp.ok) {
                    const statusData = await statusResp.json();
                    balanceEl.textContent = `${Number(statusData.balance_licn || 0)} LICN`;
                }
            } catch (_) {
                // Keep fallback value on status fetch errors.
            }
        }

        return data;
    } catch (e) {
        // Backend offline
        return null;
    }
}
document.addEventListener('DOMContentLoaded', () => {
    applyFaucetInputGuards();
    if (document.querySelector('#recentRequests')) {
        loadRecentRequests();
    }
    updateStats();
});

// Form submission
document.getElementById('faucetForm').addEventListener('submit', async (e) => {
    e.preventDefault();

    const address = document.getElementById('address').value.trim();
    const captchaValue = String(document.getElementById('captcha').value || '').trim();
    const captcha = /^\d+$/.test(captchaValue) ? Number(captchaValue) : NaN;
    const submitBtn = document.getElementById('submitBtn');
    const successMessage = document.getElementById('successMessage');
    const errorMessage = document.getElementById('errorMessage');

    // Hide previous messages
    successMessage.classList.add('hidden');
    errorMessage.classList.add('hidden');

    if (!window.LichenPQ || typeof window.LichenPQ.isValidAddress !== 'function') {
        showError('Address validator unavailable. Reload the page and try again.');
        return;
    }

    if (!window.LichenPQ.isValidAddress(address)) {
        showError('Invalid address. Enter a valid native Lichen address.');
        return;
    }

    // Validate captcha
    if (captcha !== captchaAnswer) {
        showError('Incorrect answer. Please try again.');
        document.getElementById('captcha').value = '';
        captchaAnswer = generateCaptcha();
        return;
    }

    // Disable button
    submitBtn.disabled = true;
    submitBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Processing...';

    try {
        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 15000);

        let response;
        try {
            response = await fetch(`${FAUCET_API}/faucet/request`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ address, amount: LICN_PER_REQUEST }),
                signal: controller.signal
            });
        } finally {
            clearTimeout(timeoutId);
        }

        const data = await response.json();

        if (data.success) {
            // F16.2 fix: escape all dynamic values in success HTML
            const safeSig = escapeHtml(data.signature || '');
            const effectiveAmount = data.amount ?? LICN_PER_REQUEST;
            const safeAmount = escapeHtml(String(effectiveAmount));
            const explorerLink = data.signature
                ? ` <a href="${EXPLORER_BASE}/transaction.html?sig=${encodeURIComponent(data.signature)}&to=${encodeURIComponent(address)}&amount=${encodeURIComponent(effectiveAmount)}" class="tx-link">View in Explorer</a>`
                : '';

            // Show success
            successMessage.querySelector('div').innerHTML =
                `<strong>Success!</strong> ${safeAmount} LICN sent to your address.` + explorerLink;
            successMessage.classList.remove('hidden');

            // Reset form
            document.getElementById('faucetForm').reset();
            captchaAnswer = generateCaptcha();

            // Add to recent requests
            addRecentRequest(address, data.amount, data.signature);
        } else {
            showError(data.error || 'Request failed. Please try again.');
        }
    } catch (error) {
        if (error && error.name === 'AbortError') {
            showError('Request timed out after 15 seconds. Please try again.');
            return;
        }
        showError(`Could not reach faucet service at ${FAUCET_API}. Make sure the faucet backend is running.`);
    } finally {
        submitBtn.disabled = false;
        submitBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Request Tokens';
    }
});

// Show error message and renew captcha
function showError(message) {
    const errorMessage = document.getElementById('errorMessage');
    document.getElementById('errorText').textContent = message;
    errorMessage.classList.remove('hidden');
    // Renew verification on any error/denial
    captchaAnswer = generateCaptcha();
    document.getElementById('captcha').value = '';
}

// Add request to recent list
function addRecentRequest(address, amount, signature) {
    const tbody = document.getElementById('recentRequests');
    // F16.1 fix: escape user-supplied address before innerHTML injection
    const shortAddress = escapeHtml(`${address.slice(0, 8)}...${address.slice(-4)}`);
    const safeAmount = escapeHtml(String(amount));

    const row = document.createElement('tr');
    row.innerHTML = `
        <td><code>${shortAddress}</code></td>
        <td>${safeAmount} LICN</td>
        <td>Just now</td>
        <td><span class="badge badge-success">Completed</span></td>
    `;

    tbody.insertBefore(row, tbody.firstChild);

    // Keep only last 10 requests
    while (tbody.children.length > 10) {
        tbody.removeChild(tbody.lastChild);
    }
}
