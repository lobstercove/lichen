// Lichen Website JavaScript
// Live stats, animations, and interactions

// Network configuration — centralized in shared-config.js (LICHEN_CONFIG)
const NETWORKS = {};
const WS_ENDPOINTS = {};
for (const [k, v] of Object.entries(LICHEN_CONFIG.networks)) {
    NETWORKS[k] = v.rpc;
    WS_ENDPOINTS[k] = v.ws;
}

function getSelectedNetwork() {
    return LICHEN_CONFIG.currentNetwork('lichen_website_network');
}

function getRpcEndpoint() {
    return LICHEN_CONFIG.rpc(getSelectedNetwork());
}

function switchNetwork(network) {
    localStorage.setItem('lichen_website_network', network);
    // Update RPC client endpoint
    rpc.url = getRpcEndpoint();
    // Reconnect WebSocket to new network
    disconnectWebsiteWS();
    connectWebsiteWS();
    // Refresh stats with new endpoint
    updateStats();
}

// Copy code to clipboard
function copyCode(button) {
    const codeBlock = button.closest('.code-example').querySelector('code');
    const text = codeBlock.textContent;
    const originalHTML = button.innerHTML;

    navigator.clipboard;

    const showCopySuccess = () => {
        button.innerHTML = '<i class="fas fa-check"></i>';
        button.style.color = '#06D6A0';

        setTimeout(() => {
            button.innerHTML = originalHTML;
            button.style.color = '';
        }, 2000);
    };

    const fallbackCopy = () => {
        const textarea = document.createElement('textarea');
        textarea.value = text;
        textarea.setAttribute('readonly', '');
        textarea.style.position = 'fixed';
        textarea.style.opacity = '0';
        textarea.style.pointerEvents = 'none';
        document.body.appendChild(textarea);
        textarea.focus();
        textarea.select();

        try {
            const ok = document.execCommand('copy');
            if (ok) {
                showCopySuccess();
                return true;
            }
            return false;
        } catch (error) {
            console.error('Copy fallback failed:', error);
            return false;
        } finally {
            document.body.removeChild(textarea);
        }
    };

    if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).then(() => {
            showCopySuccess();
        }).catch(err => {
            const copied = fallbackCopy();
            if (copied) return;

            console.error('Copy failed:', err);
            button.innerHTML = '<i class="fas fa-times"></i>';
            button.style.color = '#FF6B6B';
            setTimeout(() => {
                button.innerHTML = originalHTML;
                button.style.color = '';
            }, 2000);
        });
        return;
    }

    const copied = fallbackCopy();
    if (!copied) {
        button.innerHTML = '<i class="fas fa-times"></i>';
        button.style.color = '#FF6B6B';
        setTimeout(() => {
            button.innerHTML = originalHTML;
            button.style.color = '';
        }, 2000);
    }
}

// RPC Client
class LichenRPC {
    constructor(url) {
        this.url = url;
    }

    async call(method, params = []) {
        try {
            const response = await fetch(this.url, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    jsonrpc: '2.0',
                    id: 1,
                    method,
                    params
                })
            });
            const data = await response.json();
            if (data.error) {
                console.error('RPC error response:', data.error);
                return null;
            }
            return data.result;
        } catch (error) {
            console.error('RPC Error:', error);
            return null;
        }
    }

    async getValidators() { return this.call('getValidators'); }
    async getSlot() { return this.call('getSlot'); }
    async getMetrics() { return this.call('getMetrics'); }
    async getBalance(pubkey) { return this.call('getBalance', [pubkey]); }
    async getAccount(pubkey) { return this.call('getAccount', [pubkey]); }
    async sendTransaction(txData) { return this.call('sendTransaction', [txData]); }
    async health() { return this.call('health'); }
}

function resolveValidatorCount(validatorsResult, currentSlot) {
    let list = null;
    if (Array.isArray(validatorsResult)) list = validatorsResult;
    else if (validatorsResult && Array.isArray(validatorsResult.validators)) list = validatorsResult.validators;
    if (!list) return 0;
    // Count only validators active within 100 slots of current (same as explorer)
    if (typeof currentSlot === 'number' && currentSlot > 0) {
        return list.filter(v => {
            const lastActive = v.last_active_slot || v.lastActiveSlot || 0;
            return currentSlot - lastActive <= 100;
        }).length;
    }
    return list.length;
}

function resolveTps(metricsResult) {
    if (!metricsResult || typeof metricsResult !== 'object') return null;
    const value = metricsResult.tps;
    if (typeof value === 'number' && isFinite(value)) return value;
    if (typeof value === 'string') {
        const parsed = Number(value);
        if (!Number.isNaN(parsed) && isFinite(parsed)) return parsed;
    }
    return null;
}

const rpc = new LichenRPC(getRpcEndpoint());

function setNetworkIndicator(status, message) {
    const indicator = document.getElementById('networkIndicator');
    if (!indicator) return;
    indicator.classList.remove('status-online', 'status-offline', 'status-connecting');
    indicator.classList.add(`status-${status}`);
    indicator.textContent = message;
}

let statsInitialLoad = true;

// Update live stats — only Latest Block + Validators
async function updateStats() {
    try {
        // Only show 'connecting' on initial load, not on periodic re-polls
        if (statsInitialLoad) {
            setNetworkIndicator('connecting', 'Syncing network stats…');
        }
        const [slot, validators, metrics] = await Promise.allSettled([
            rpc.getSlot(),
            rpc.getValidators(),
            rpc.getMetrics()
        ]);

        const slotOk = slot.status === 'fulfilled' && slot.value !== null;
        const validatorsOk = validators.status === 'fulfilled' && validators.value !== null;
        const metricsOk = metrics.status === 'fulfilled' && metrics.value !== null;

        if (slotOk) {
            const blockEl = document.getElementById('stat-block');
            if (blockEl) blockEl.textContent = formatNumber(slot.value);
        }

        if (validatorsOk) {
            const currentSlot = slotOk ? slot.value : null;
            const count = resolveValidatorCount(validators.value, currentSlot);
            const el = document.getElementById('stat-validators');
            if (el) el.textContent = count;
        }

        if (metricsOk) {
            const tps = resolveTps(metrics.value);
            const el = document.getElementById('stat-tps');
            if (el) el.textContent = tps === null ? '—' : formatNumber(Math.round(tps));
        }

        if (slotOk || validatorsOk || metricsOk) {
            statsInitialLoad = false;
            setNetworkIndicator('online', `Connected · ${getSelectedNetwork()}`);
        } else {
            setNetworkIndicator('offline', `RPC unavailable · ${getSelectedNetwork()}`);
        }
    } catch (error) {
        console.error('Stats update error:', error);
        setNetworkIndicator('offline', `RPC unavailable · ${getSelectedNetwork()}`);
    }
}

// Format large numbers
function formatNumber(num) {
    if (typeof num !== 'number' || !isFinite(num)) return '—';
    if (num >= 1000000) {
        return (num / 1000000).toFixed(1) + 'M';
    } else if (num >= 1000) {
        return (num / 1000).toFixed(1) + 'K';
    }
    return num.toLocaleString();
}

// ===== WebSocket live stats =====
let websiteWs = null;
let websiteWsReconnectTimer = null;

function getWsEndpoint() {
    return LICHEN_CONFIG.ws(getSelectedNetwork());
}

function connectWebsiteWS() {
    if (websiteWs) {
        try { websiteWs.close(); } catch (e) { }
    }
    clearTimeout(websiteWsReconnectTimer);

    try {
        websiteWs = new WebSocket(getWsEndpoint());

        websiteWs.onopen = () => {
            setNetworkIndicator('online', `Connected · ${getSelectedNetwork()}`);
            // Subscribe to slot updates
            websiteWs.send(JSON.stringify({
                jsonrpc: '2.0', id: 1,
                method: 'slotSubscribe', params: []
            }));
        };

        websiteWs.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                const slot = extractSlotFromWsMessage(data);
                if (slot !== null) {
                    const blockEl = document.getElementById('stat-block');
                    if (blockEl) blockEl.textContent = formatNumber(slot);
                }
            } catch (e) { }
        };

        websiteWs.onclose = () => {
            setNetworkIndicator('offline', `Realtime disconnected · retrying…`);
            websiteWsReconnectTimer = setTimeout(connectWebsiteWS, 5000);
        };

        websiteWs.onerror = () => {
            websiteWs.close();
        };
    } catch (e) {
        console.error('[WS] Connection failed:', e);
        websiteWsReconnectTimer = setTimeout(connectWebsiteWS, 5000);
    }
}

function extractSlotFromWsMessage(data) {
    if (!data || typeof data !== 'object') return null;

    const direct = data?.params?.result?.slot;
    if (typeof direct === 'number') return direct;

    const nested = data?.params?.result?.result?.slot;
    if (typeof nested === 'number') return nested;

    const alt = data?.result?.slot;
    if (typeof alt === 'number') return alt;

    return null;
}

function disconnectWebsiteWS() {
    clearTimeout(websiteWsReconnectTimer);
    if (websiteWs) {
        try { websiteWs.close(); } catch (e) { }
        websiteWs = null;
    }
}

// Smooth scroll for anchor links
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

        if (navMenu && navMenu.classList.contains('active')) {
            navMenu.classList.remove('active');
            if (navToggle) navToggle.classList.remove('active');
            if (navActions) navActions.classList.remove('active');
            updateMobileNavOffsets();
        }
    });
});

// Mobile nav toggle
const navToggle = document.getElementById('navToggle');
const navMenu = document.querySelector('.nav-menu');
const navActions = document.querySelector('.nav-actions');
const navContainer = document.querySelector('.nav-container');

function updateMobileNavOffsets() {
    if (!navContainer || !navMenu) return;
    const menuHeight = navMenu.classList.contains('active') ? navMenu.offsetHeight : 0;
    navContainer.style.setProperty('--mobile-nav-menu-height', `${menuHeight}px`);
}

if (navToggle && navMenu) {
    navToggle.addEventListener('click', () => {
        navMenu.classList.toggle('active');
        navToggle.classList.toggle('active');
        if (navActions) navActions.classList.toggle('active');
        updateMobileNavOffsets();
    });

    window.addEventListener('resize', updateMobileNavOffsets);
}

// Intersection Observer for animations
const observerOptions = {
    threshold: 0.1,
    rootMargin: '0px 0px -100px 0px'
};

const observer = new IntersectionObserver((entries) => {
    entries.forEach(entry => {
        if (entry.isIntersecting) {
            entry.target.classList.add('visible');
        }
    });
}, observerOptions);

// Observe all sections and cards
document.querySelectorAll('.section, .feature-card, .vision-card, .comparison-card, .spec-card, .token-card, .chain-card, .contract-card, .roadmap-phase, .discount-tier, .community-card, .reputation-discounts').forEach(el => {
    observer.observe(el);
});

// API Tab Switching
function setupApiTabs() {
    const apiTabs = document.querySelectorAll('.api-tab');
    const apiCategories = document.querySelectorAll('.api-category');

    if (apiTabs.length === 0) return;

    apiTabs.forEach(tab => {
        const isActive = tab.classList.contains('active');
        tab.setAttribute('aria-selected', isActive ? 'true' : 'false');
        tab.tabIndex = isActive ? 0 : -1;
    });

    apiCategories.forEach(panel => {
        panel.hidden = !panel.classList.contains('active');
    });

    apiTabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const category = tab.dataset.category;

            // Remove active from all tabs and categories
            apiTabs.forEach(t => {
                t.classList.remove('active');
                t.setAttribute('aria-selected', 'false');
                t.tabIndex = -1;
            });
            apiCategories.forEach(c => {
                c.classList.remove('active');
                c.hidden = true;
            });

            // Add active to clicked tab
            tab.classList.add('active');
            tab.setAttribute('aria-selected', 'true');
            tab.tabIndex = 0;

            // Show corresponding category
            const targetCategory = document.querySelector(`.api-category[data-category="${category}"]`);
            if (targetCategory) {
                targetCategory.classList.add('active');
                targetCategory.hidden = false;
            }
        });
    });
}

// Wizard Tab Switching (Deploy Section)
function setupWizardTabs() {
    const wizardTabs = document.querySelectorAll('.wizard-tab');
    const wizardSteps = document.querySelectorAll('.wizard-step');

    if (wizardTabs.length === 0) return;

    wizardTabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const step = tab.dataset.step;

            // Remove active from all tabs and steps
            wizardTabs.forEach(t => t.classList.remove('active'));
            wizardSteps.forEach(s => s.classList.remove('active'));

            // Add active to clicked tab
            tab.classList.add('active');

            // Show corresponding step
            const targetStep = document.querySelector(`.wizard-step[data-step="${step}"]`);
            if (targetStep) {
                targetStep.classList.add('active');
            }
        });
    });
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    // Wire network selector via LICHEN_CONFIG (auto-populates, hides local-* in production)
    LICHEN_CONFIG.initNetworkSelector('websiteNetworkSelect', 'lichen_website_network', (network) => {
        switchNetwork(network);
    });

    // Initial stats update
    updateStats();

    // Update stats every 5 seconds (polling fallback)
    setInterval(updateStats, 5000);

    // Connect WebSocket for live block updates
    connectWebsiteWS();

    // Add fade-in animation to hero
    setTimeout(() => {
        const heroContent = document.querySelector('.hero-content');
        if (heroContent) {
            heroContent.classList.add('visible');
        }
    }, 100);

    // Setup API tabs
    setupApiTabs();

    // Setup Wizard tabs
    setupWizardTabs();
});

// Parallax effect for hero background
let ticking = false;

function updateParallax() {
    const scrolled = window.pageYOffset;
    const parallax = document.querySelector('.hero-background');

    if (parallax) {
        const yPos = -(scrolled * 0.5);
        parallax.style.transform = `translate3d(0, ${yPos}px, 0)`;
    }

    ticking = false;
}

window.addEventListener('scroll', () => {
    if (!ticking) {
        window.requestAnimationFrame(updateParallax);
        ticking = true;
    }
});

// Add animation classes for visibility
const style = document.createElement('style');
style.textContent = `
    .feature-card, .vision-card, .comparison-card, .spec-card, .token-card, .chain-card, .contract-card, .roadmap-phase, .discount-tier, .community-card, .reputation-discounts {
        opacity: 0;
        transform: translateY(30px);
        transition: opacity 0.6s ease, transform 0.6s ease;
    }
    
    .feature-card.visible, .vision-card.visible, .comparison-card.visible, .spec-card.visible, .token-card.visible, .chain-card.visible, .contract-card.visible, .roadmap-phase.visible, .discount-tier.visible, .community-card.visible, .reputation-discounts.visible {
        opacity: 1;
        transform: translateY(0);
    }
    
    .hero-content {
        opacity: 0;
        transform: translateY(20px);
        transition: opacity 0.8s ease, transform 0.8s ease;
    }
    
    .hero-content.visible {
        opacity: 1;
        transform: translateY(0);
    }
    
    /* Mobile nav active state */
    .nav-menu.active {
        display: flex;
        flex-direction: column;
        position: absolute;
        top: 100%;
        left: 0;
        right: 0;
        background: var(--bg-dark);
        border-top: 1px solid var(--border);
        padding: 1rem;
        gap: 1rem;
    }
    
    .nav-toggle.active span:nth-child(1) {
        transform: rotate(45deg) translate(5px, 5px);
    }
    
    .nav-toggle.active span:nth-child(2) {
        opacity: 0;
    }
    
    .nav-toggle.active span:nth-child(3) {
        transform: rotate(-45deg) translate(7px, -7px);
    }
    
    @media (max-width: 768px) {
        .nav-menu {
            display: none;
        }
        
        .nav-menu.active {
            display: flex;
        }
    }
`;
document.head.appendChild(style);

// Clean up WebSocket on page unload and pause in background tabs
window.addEventListener('beforeunload', () => {
    disconnectWebsiteWS();
});

document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
        disconnectWebsiteWS();
    } else {
        connectWebsiteWS();
    }
});

// Console branding (production-safe — no sensitive data leaked)
// Removed: was logging RPC URL which is operational detail
