// MoltFaucet JavaScript
// Connects to the MoltChain faucet backend (Rust/axum on port 9100)

const FAUCET_API = 'http://localhost:9100';
const MOLT_PER_REQUEST = 10;

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
        const resp = await fetch(`${FAUCET_API}/health`);
        if (resp.ok) {
            document.querySelector('.stat-card:first-child .stat-value').textContent = MOLT_PER_REQUEST + ' MOLT';
        }
    } catch (e) {
        // Backend offline
    }
}
updateStats();

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

    // Validate address format (base58 addresses, typically 32-44 chars)
    if (!address || address.length < 20) {
        showError('Invalid address. Enter a valid MoltChain base58 address.');
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
        const response = await fetch(`${FAUCET_API}/faucet/request`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ address, amount: MOLT_PER_REQUEST })
        });

        const data = await response.json();

        if (data.success) {
            // Build explorer link with airdrop details
            const explorerLink = data.signature
                ? ` <a href="../explorer/transaction.html?sig=${data.signature}&to=${encodeURIComponent(address)}&amount=${data.amount}" class="tx-link">View in Explorer</a>`
                : '';

            // Show success
            successMessage.querySelector('div').innerHTML =
                `<strong>Success!</strong> ${data.amount} MOLT sent to your address.` + explorerLink;
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
        showError('Could not reach faucet service. Make sure the faucet backend is running on port 9100.');
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
    const shortAddress = `${address.slice(0, 8)}...${address.slice(-4)}`;

    const row = document.createElement('tr');
    row.innerHTML = `
        <td><code>${shortAddress}</code></td>
        <td>${amount} MOLT</td>
        <td>Just now</td>
        <td><span class="badge badge-success">Completed</span></td>
    `;

    tbody.insertBefore(row, tbody.firstChild);

    // Keep only last 10 requests
    while (tbody.children.length > 10) {
        tbody.removeChild(tbody.lastChild);
    }
}
