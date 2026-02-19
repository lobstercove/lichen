// MoltChain Website JavaScript
// Live stats, animations, and interactions

// Network configuration
const NETWORKS = {
    'mainnet': 'https://rpc.moltchain.network',
    'testnet': 'https://testnet-rpc.moltchain.network',
    'local-testnet': 'http://localhost:8899',
    'local-mainnet': 'http://localhost:8899'
};

const WS_ENDPOINTS = {
    'mainnet': 'wss://rpc.moltchain.network/ws',
    'testnet': 'wss://testnet-rpc.moltchain.network/ws',
    'local-testnet': 'ws://localhost:8900',
    'local-mainnet': 'ws://localhost:8900'
};

function getSelectedNetwork() {
    return localStorage.getItem('moltchain_website_network') || 'local-testnet';
}

function getRpcEndpoint() {
    return NETWORKS[getSelectedNetwork()] || NETWORKS['local-testnet'];
}

function switchNetwork(network) {
    localStorage.setItem('moltchain_website_network', network);
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
    
    navigator.clipboard.writeText(text).then(() => {
        button.innerHTML = '<i class="fas fa-check"></i>';
        button.style.color = '#06D6A0';
        
        setTimeout(() => {
            button.innerHTML = originalHTML;
            button.style.color = '';
        }, 2000);
    }).catch(err => {
        console.error('Copy failed:', err);
        button.innerHTML = '<i class="fas fa-times"></i>';
        button.style.color = '#FF6B6B';
        setTimeout(() => {
            button.innerHTML = originalHTML;
            button.style.color = '';
        }, 2000);
    });
}

// RPC Client
class MoltChainRPC {
    constructor(url) {
        this.url = url;
    }
    
    async call(method, params = []) {
        try {
            const response = await fetch(this.url, {
                method: 'POST',
                headers: {'Content-Type': 'application/json'},
                body: JSON.stringify({
                    jsonrpc: '2.0',
                    id: 1,
                    method,
                    params
                })
            });
            const data = await response.json();
            return data.result || data.error;
        } catch (error) {
            console.error('RPC Error:', error);
            return null;
        }
    }
    
    async getValidators() { return this.call('getValidators'); }
    async getSlot() { return this.call('getSlot'); }
    async getBalance(pubkey) { return this.call('getBalance', [pubkey]); }
    async getAccount(pubkey) { return this.call('getAccount', [pubkey]); }
    async sendTransaction(txData) { return this.call('sendTransaction', [txData]); }
    async health() { return this.call('health'); }
}

const rpc = new MoltChainRPC(getRpcEndpoint());

// Update live stats — only Latest Block + Validators
async function updateStats() {
    try {
        const [slot, validators] = await Promise.allSettled([
            rpc.getSlot(),
            rpc.getValidators()
        ]);

        if (slot.status === 'fulfilled' && slot.value !== null) {
            const blockEl = document.getElementById('stat-block');
            if (blockEl) blockEl.textContent = formatNumber(slot.value);
        }

        if (validators.status === 'fulfilled' && validators.value) {
            const count = validators.value.count || validators.value.validators?.length || 0;
            const el = document.getElementById('stat-validators');
            if (el) el.textContent = count;
        }
    } catch (error) {
        console.error('Stats update error:', error);
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
    return WS_ENDPOINTS[getSelectedNetwork()] || WS_ENDPOINTS['local-testnet'];
}

function connectWebsiteWS() {
    if (websiteWs) {
        try { websiteWs.close(); } catch(e) {}
    }
    clearTimeout(websiteWsReconnectTimer);

    try {
        websiteWs = new WebSocket(getWsEndpoint());

        websiteWs.onopen = () => {
            console.log('[WS] Connected');
            // Subscribe to slot updates
            websiteWs.send(JSON.stringify({
                jsonrpc: '2.0', id: 1,
                method: 'slotSubscribe', params: []
            }));
        };

        websiteWs.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                if (data.params?.result?.slot !== undefined) {
                    const blockEl = document.getElementById('stat-block');
                    if (blockEl) blockEl.textContent = formatNumber(data.params.result.slot);
                }
            } catch(e) {}
        };

        websiteWs.onclose = () => {
            console.log('[WS] Disconnected, reconnecting in 5s');
            websiteWsReconnectTimer = setTimeout(connectWebsiteWS, 5000);
        };

        websiteWs.onerror = () => {
            websiteWs.close();
        };
    } catch(e) {
        console.error('[WS] Connection failed:', e);
        websiteWsReconnectTimer = setTimeout(connectWebsiteWS, 5000);
    }
}

function disconnectWebsiteWS() {
    clearTimeout(websiteWsReconnectTimer);
    if (websiteWs) {
        try { websiteWs.close(); } catch(e) {}
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
    });
});

// Mobile nav toggle
const navToggle = document.getElementById('navToggle');
const navMenu = document.querySelector('.nav-menu');
const navActions = document.querySelector('.nav-actions');

if (navToggle && navMenu) {
    navToggle.addEventListener('click', () => {
        navMenu.classList.toggle('active');
        navToggle.classList.toggle('active');
        if (navActions) navActions.classList.toggle('active');
    });
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
        tab.addEventListener('click', () => {
            const category = tab.dataset.category;
            
            // Remove active from all tabs and categories
            apiTabs.forEach(t => t.classList.remove('active'));
            apiCategories.forEach(c => c.classList.remove('active'));
            
            // Add active to clicked tab
            tab.classList.add('active');
            
            // Show corresponding category
            const targetCategory = document.querySelector(`.api-category[data-category="${category}"]`);
            if (targetCategory) {
                targetCategory.classList.add('active');
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
    console.log('MoltChain website loaded 🦞');
    
    // Restore saved network selection
    const savedNetwork = getSelectedNetwork();
    const networkSelect = document.getElementById('websiteNetworkSelect');
    if (networkSelect) {
        networkSelect.value = savedNetwork;
    }
    
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

// Console art
console.log('%c🦞 MoltChain', 'font-size: 24px; font-weight: bold; color: #FF6B35;');
console.log('%cThe Agent-First Blockchain', 'font-size: 14px; color: #B8C1EC;');
console.log('%cWebsite loaded successfully', 'font-size: 12px; color: #06D6A0;');
console.log('%cRPC URL:', 'font-size: 12px; color: #6B7A99;', getRpcEndpoint());
