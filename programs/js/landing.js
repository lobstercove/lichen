// Lichen Programs Landing Page - JavaScript
// Interactive features and animations

console.log('🦞 Lichen Programs Landing Page Loading...');

const NETWORK_STORAGE_KEY = 'programs_network';

function resolveNetwork(name) {
    if (typeof LICHEN_CONFIG !== 'undefined') {
        return LICHEN_CONFIG.resolveNetwork(name);
    }
    if (name === 'local') return 'local-testnet';
    return name || 'testnet';
}

let currentNetwork = resolveNetwork(localStorage.getItem(NETWORK_STORAGE_KEY) || 'testnet');
let RPC_URL = typeof LICHEN_CONFIG !== 'undefined'
    ? LICHEN_CONFIG.rpc(currentNetwork)
    : 'https://testnet-rpc.lichen.network';

function setProgramsNetwork(network, { reload = false } = {}) {
    currentNetwork = resolveNetwork(network);
    localStorage.setItem(NETWORK_STORAGE_KEY, currentNetwork);
    RPC_URL = typeof LICHEN_CONFIG !== 'undefined'
        ? LICHEN_CONFIG.rpc(currentNetwork)
        : RPC_URL;
    if (reload) {
        window.location.reload();
    }
}

function initProgramsNetworkSelector() {
    const selector = document.getElementById('programsNetworkSelect');
    if (!selector) return;
    selector.value = currentNetwork;
    selector.addEventListener('change', () => {
        setProgramsNetwork(selector.value, { reload: true });
    });
}

// ===== Initialize =====
document.addEventListener('DOMContentLoaded', () => {
    console.log('✅ Landing page initialized');
    initProgramsNetworkSelector();

    // Setup event listeners
    setupWizardTabs();
    setupLanguageTabs();
    setupSmoothScroll();
    setupAnimations();
    updateStats();

    // Update stats periodically (RPC-backed)
    setInterval(updateStats, 5000);
});

// ===== Wizard Tabs =====
function setupWizardTabs() {
    const tabs = document.querySelectorAll('.wizard-tab');
    const steps = document.querySelectorAll('.wizard-step');

    tabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const step = tab.dataset.step;

            // Update tabs
            tabs.forEach(t => t.classList.remove('active'));
            tab.classList.add('active');

            // Update steps
            steps.forEach(s => s.classList.remove('active'));
            const targetStep = document.querySelector(`.wizard-step[data-step="${step}"]`);
            if (targetStep) {
                targetStep.classList.add('active');
            }
        });
    });
}

// ===== Language Tabs (COPIED FROM WEBSITE API TABS) =====
function setupLanguageTabs() {
    const tabs = document.querySelectorAll('.language-tab');
    const contents = document.querySelectorAll('.language-content');

    if (tabs.length === 0) return;

    tabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const lang = tab.dataset.lang;

            // Remove active from all tabs and contents
            tabs.forEach(t => t.classList.remove('active'));
            contents.forEach(c => c.classList.remove('active'));

            // Add active to clicked tab
            tab.classList.add('active');

            // Show corresponding content
            const targetContent = document.querySelector(`.language-content[data-lang="${lang}"]`);
            if (targetContent) {
                targetContent.classList.add('active');
            }
        });
    });
}

// ===== Code Copying =====
function copyCode(button) {
    const codeBlock = button.closest('.code-block, .code-example');
    const code = codeBlock.querySelector('code').textContent;

    navigator.clipboard.writeText(code).then(() => {
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
    }).catch(err => {
        console.error('Failed to copy code:', err);
        button.innerHTML = '<i class="fas fa-times"></i> Failed';
    });
}

// ===== View Code =====
function viewCode(exampleName) {
    // In a real implementation, this would open a modal with the full code
    // For now, redirect to playground with the example loaded
    window.location.href = `playground.html?example=${exampleName}`;
}

// ===== Smooth Scroll =====
function setupSmoothScroll() {
    document.querySelectorAll('a[href^="#"]').forEach(anchor => {
        anchor.addEventListener('click', function (e) {
            e.preventDefault();
            const target = document.querySelector(this.getAttribute('href'));
            if (target) {
                target.scrollIntoView({
                    behavior: 'smooth',
                    block: 'start'
                });
            }
        });
    });
}

// ===== Scroll Animations =====
function setupAnimations() {
    const observerOptions = {
        threshold: 0.1,
        rootMargin: '0px 0px -100px 0px'
    };

    const observer = new IntersectionObserver((entries) => {
        entries.forEach(entry => {
            if (entry.isIntersecting) {
                entry.target.style.opacity = '1';
                entry.target.style.transform = 'translateY(0)';
            }
        });
    }, observerOptions);

    // Observe all sections
    document.querySelectorAll('.section').forEach(section => {
        section.style.opacity = '0';
        section.style.transform = 'translateY(30px)';
        section.style.transition = 'opacity 0.6s ease, transform 0.6s ease';
        observer.observe(section);
    });

    // Observe cards
    document.querySelectorAll('.comparison-card, .step-card, .example-card, .feature-card').forEach(card => {
        card.style.opacity = '0';
        card.style.transform = 'translateY(20px)';
        card.style.transition = 'opacity 0.4s ease, transform 0.4s ease';
        observer.observe(card);
    });
}

// ===== Update Stats (zero fallback — shows 0 when RPC unavailable) =====
async function updateStats() {
    var stats = {
        programsDeployed: 0,
        validators: 0,
        deployTime: '0.0',
        deployFee: '0.000000'
    };

    try {
        const [programs, metrics, feeConfig, validators] = await Promise.all([
            rpcCall('getPrograms', [{ limit: 500 }]),
            rpcCall('getMetrics'),
            rpcCall('getFeeConfig'),
            rpcCall('getValidators')
        ]);

        const programsDeployed = programs?.count || programs?.programs?.length || 0;
        const validatorCount = validators?.validators?.length || 0;
        const deployTime = metrics?.average_block_time
            ? Number(metrics.average_block_time).toFixed(1)
            : '0.0';
        const deployFee = feeConfig?.contract_deploy_fee
            ? (feeConfig.contract_deploy_fee / 1_000_000_000).toFixed(6)
            : '0.000000';

        stats = {
            programsDeployed,
            validators: validatorCount,
            deployTime,
            deployFee
        };
    } catch (error) {
        // Keep zeros — no fake inflation
    }

    animateNumber('programsDeployed', stats.programsDeployed, 0);
    animateNumber('activeDevs', stats.validators, 0);

    const deployTimeEl = document.getElementById('deployTime');
    const deployFeeEl = document.getElementById('deployFee');

    if (deployTimeEl) deployTimeEl.textContent = stats.deployTime + 's';
    if (deployFeeEl) deployFeeEl.textContent = '$' + stats.deployFee;
}

async function rpcCall(method, params = []) {
    const response = await fetch(RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method,
            params
        })
    });

    const payload = await response.json();
    if (payload.error) {
        throw new Error(payload.error.message || 'RPC error');
    }
    return payload.result;
}

// ===== Animate Number =====
function animateNumber(elementId, target, decimals = 0) {
    const element = document.getElementById(elementId);
    if (!element) return;

    const current = parseInt(element.textContent.replace(/,/g, '')) || 0;
    const increment = (target - current) / 20;
    let value = current;

    const timer = setInterval(() => {
        value += increment;
        if ((increment > 0 && value >= target) || (increment < 0 && value <= target)) {
            value = target;
            clearInterval(timer);
        }

        element.textContent = decimals > 0
            ? value.toFixed(decimals)
            : Math.floor(value).toLocaleString();
    }, 50);
}

// ===== Mobile Menu (if needed) =====
function toggleMobileMenu() {
    const menu = document.querySelector('.nav-menu');
    menu.classList.toggle('active');
}

// ===== Parallax Effect (Hero Background) =====
window.addEventListener('scroll', () => {
    const scrolled = window.pageYOffset;
    const heroBackground = document.querySelector('.hero-background');

    if (heroBackground && scrolled < window.innerHeight) {
        heroBackground.style.transform = `translateY(${scrolled * 0.5}px)`;
    }
});

// ===== Example Cards Hover Effect =====
document.querySelectorAll('.example-card').forEach(card => {
    card.addEventListener('mouseenter', function () {
        this.style.transform = 'translateY(-8px) scale(1.02)';
    });

    card.addEventListener('mouseleave', function () {
        this.style.transform = '';
    });
});

// ===== Feature Cards Hover Effect =====
document.querySelectorAll('.feature-card').forEach(card => {
    card.addEventListener('mouseenter', function () {
        const icon = this.querySelector('.feature-icon');
        if (icon) {
            icon.style.transform = 'scale(1.1) rotate(5deg)';
        }
    });

    card.addEventListener('mouseleave', function () {
        const icon = this.querySelector('.feature-icon');
        if (icon) {
            icon.style.transform = '';
        }
    });
});

// ===== Track Clicks (Analytics) =====
function trackClick(category, action, label) {
    console.log('Track:', category, action, label);
    // In production: send to analytics service
}

// Add tracking to CTAs
document.querySelectorAll('.btn').forEach(btn => {
    btn.addEventListener('click', function () {
        const text = this.textContent.trim();
        trackClick('CTA', 'Click', text);
    });
});

// ===== Console Welcome Message =====
console.log('%c🦞 Lichen Programs', 'font-size: 20px; font-weight: bold; color: #00C9DB;');
console.log('%cBuild, Deploy & Scale Smart Contracts', 'font-size: 14px; color: #B8C1EC;');
console.log('%cInterested in building? Join us:', 'font-size: 12px; color: #6B7A99;');
console.log('%chttps://discord.gg/gkQmsHXRXp', 'font-size: 12px; color: #00C9DB;');

console.log('✅ Landing page ready!');
