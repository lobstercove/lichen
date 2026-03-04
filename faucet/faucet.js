// MoltFaucet JavaScript
// Connects to the MoltChain faucet backend (Rust/axum; default port 9100, Docker uses 9101 via PORT env)

const FAUCET_API =
    (typeof MOLT_CONFIG !== 'undefined' && MOLT_CONFIG?.faucet) ||
    (typeof window !== 'undefined' && window.MOLT_CONFIG?.faucet) ||
    'http://localhost:9100';
const EXPLORER_BASE =
    (typeof MOLT_CONFIG !== 'undefined' && MOLT_CONFIG?.explorer) ||
    (typeof window !== 'undefined' && window.MOLT_CONFIG?.explorer) ||
    '../explorer';
const MOLT_PER_REQUEST = 100;

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
        const amount = Number(record.amount_molt || 0);
        const shortAddress = escapeHtml(`${recipient.slice(0, 8)}...${recipient.slice(-4)}`);
        const safeAmount = escapeHtml(String(amount));

        const row = document.createElement('tr');
        row.innerHTML = `
            <td><code>${shortAddress}</code></td>
            <td>${safeAmount} MOLT</td>
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

        if (perRequestEl) perRequestEl.textContent = `${Number(data.max_per_request || MOLT_PER_REQUEST)} MOLT`;
        if (cooldownEl) cooldownEl.textContent = formatCooldown(data.cooldown_seconds || 0);
        if (dailyLimitEl) dailyLimitEl.textContent = `${Number(data.daily_limit_per_ip || 0)} MOLT / IP`;

        const balanceEl = document.getElementById('statFaucetBalance');
        if (balanceEl) {
            try {
                const statusResp = await fetch(`${FAUCET_API}/faucet/status`);
                if (statusResp.ok) {
                    const statusData = await statusResp.json();
                    balanceEl.textContent = `${Number(statusData.balance_molt || 0)} MOLT`;
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
    if (document.querySelector('#recentRequests')) {
        loadRecentRequests();
    }
    updateStats();
});

// Form submission
document.getElementById('faucetForm').addEventListener('submit', async (e) => {
    e.preventDefault();

    const address = document.getElementById('address').value.trim();
    const captcha = parseInt(document.getElementById('captcha').value);
    const submitBtn = document.getElementById('submitBtn');
    const successMessage = document.getElementById('successMessage');
    const errorMessage = document.getElementById('errorMessage');

    // Hide previous messages
    successMessage.classList.add('hidden');
    errorMessage.classList.add('hidden');

    // F16.4 fix: validate address format (base58 addresses, 32-44 chars)
    if (!address || address.length < 32 || address.length > 44) {
        showError('Invalid address. Enter a valid MoltChain base58 address (32-44 characters).');
        return;
    }
    // Reject non-base58 characters
    if (!/^[1-9A-HJ-NP-Za-km-z]+$/.test(address)) {
        showError('Invalid address. Only base58 characters are allowed.');
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
                body: JSON.stringify({ address, amount: MOLT_PER_REQUEST }),
                signal: controller.signal
            });
        } finally {
            clearTimeout(timeoutId);
        }

        const data = await response.json();

        if (data.success) {
            // F16.2 fix: escape all dynamic values in success HTML
            const safeSig = escapeHtml(data.signature || '');
            const effectiveAmount = data.amount ?? MOLT_PER_REQUEST;
            const safeAmount = escapeHtml(String(effectiveAmount));
            const explorerLink = data.signature
                ? ` <a href="${EXPLORER_BASE}/transaction.html?sig=${encodeURIComponent(data.signature)}&to=${encodeURIComponent(address)}&amount=${encodeURIComponent(effectiveAmount)}" class="tx-link">View in Explorer</a>`
                : '';

            // Show success
            successMessage.querySelector('div').innerHTML =
                `<strong>Success!</strong> ${safeAmount} MOLT sent to your address.` + explorerLink;
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
        <td>${safeAmount} MOLT</td>
        <td>Just now</td>
        <td><span class="badge badge-success">Completed</span></td>
    `;

    tbody.insertBefore(row, tbody.firstChild);

    // Keep only last 10 requests
    while (tbody.children.length > 10) {
        tbody.removeChild(tbody.lastChild);
    }
}
