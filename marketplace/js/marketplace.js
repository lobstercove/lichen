// Molt Market - NFT Marketplace JavaScript
// All data sourced from RPC — no mock/fallback data

console.log('🦞 Molt Market loading...');

const RPC_URL = (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) || 'http://localhost:8899';
const dataSource = window.marketplaceDataSource;
let currentWallet = null;

// ===== Initialize =====
document.addEventListener('DOMContentLoaded', () => {
    console.log('✅ Molt Market initialized');
    
    // Load homepage content
    loadFeaturedCollections();
    loadTrendingNFTs('24h');
    loadTopCreators();
    loadRecentSales();
    
    // Setup event listeners
    setupConnectWallet();
    setupSearch();
    setupFilterTabs();
    updateStats();
    
    // Initialize network selector
    if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();
    
    // Update stats periodically
    setInterval(updateStats, 10000);
    
    console.log('✅ Molt Market ready');
});

// ===== Load Featured Collections =====
async function loadFeaturedCollections() {
    let collections = [];
    try {
        if (dataSource) {
            collections = await dataSource.getFeaturedCollections(6);
        }
    } catch (err) {
        console.warn('Live collections unavailable:', err.message);
    }

    const container = document.getElementById('featuredCollections');
    
    if (!collections || collections.length === 0) {
        container.innerHTML = '<div class="empty-state" style="grid-column: 1/-1; text-align: center; padding: 3rem; opacity: 0.6;"><i class="fas fa-images" style="font-size: 2rem; margin-bottom: 1rem; display: block;"></i>No collections yet. Be the first to create one!</div>';
        return;
    }
    
    container.innerHTML = collections.map(collection => `
        <div class="collection-card" onclick="viewCollection('${escapeHtml(collection.id)}')">
            <div class="collection-banner" style="background: ${escapeHtml(collection.banner)}"></div>
            <div class="collection-avatar">${escapeHtml(collection.avatar)}</div>
            <div class="collection-info">
                <div class="collection-name">${escapeHtml(collection.name)}</div>
                <div class="collection-stats">
                    <div class="collection-stat">
                        <div class="collection-stat-value">${formatNumber(collection.items)}</div>
                        <div class="collection-stat-label">Items</div>
                    </div>
                    <div class="collection-stat">
                        <div class="collection-stat-value">${escapeHtml(collection.floor)}</div>
                        <div class="collection-stat-label">Floor</div>
                    </div>
                    <div class="collection-stat">
                        <div class="collection-stat-value">${formatNumber(collection.volume)}</div>
                        <div class="collection-stat-label">Volume</div>
                    </div>
                </div>
            </div>
        </div>
    `).join('');
}

// ===== Load Trending NFTs =====
async function loadTrendingNFTs(period = '24h') {
    let nfts = [];
    try {
        if (dataSource) {
            nfts = await dataSource.getTrendingNFTs(8, period);
        }
    } catch (err) {
        console.warn('Live trending NFTs unavailable:', err.message);
    }

    const container = document.getElementById('trendingNFTs');

    if (!nfts || nfts.length === 0) {
        container.innerHTML = '<div class="empty-state" style="grid-column: 1/-1; text-align: center; padding: 3rem; opacity: 0.6;"><i class="fas fa-fire" style="font-size: 2rem; margin-bottom: 1rem; display: block;"></i>No trending NFTs yet</div>';
        return;
    }
    
    container.innerHTML = nfts.map(nft => `
        <div class="nft-card" onclick="viewNFT('${escapeHtml(nft.id)}')">
            <div class="nft-image" style="background: ${escapeHtml(nft.image)}"></div>
            <div class="nft-info">
                <div class="nft-collection">${escapeHtml(nft.collection)}</div>
                <div class="nft-name">${escapeHtml(nft.name)}</div>
                <div class="nft-footer">
                    <div class="nft-price">
                        Price
                        <span class="nft-price-value">${escapeHtml(nft.price)} MOLT</span>
                    </div>
                    <button class="nft-action" onclick="event.stopPropagation(); buyNFT('${escapeHtml(nft.id)}')">
                        Buy Now
                    </button>
                </div>
            </div>
        </div>
    `).join('');
}

// ===== Load Top Creators =====
async function loadTopCreators() {
    let creators = [];
    try {
        if (dataSource) {
            creators = await dataSource.getTopCreators(5);
        }
    } catch (err) {
        console.warn('Live creators unavailable:', err.message);
    }

    const container = document.getElementById('topCreators');

    if (!creators || creators.length === 0) {
        container.innerHTML = '<div class="empty-state" style="text-align: center; padding: 2rem; opacity: 0.6;">No creators yet</div>';
        return;
    }
    
    container.innerHTML = creators.slice(0, 5).map(creator => `
        <div class="creator-card" onclick="viewCreator('${escapeHtml(creator.id)}')">
            <div class="creator-avatar">${escapeHtml(creator.avatar)}</div>
            <div class="creator-name">${escapeHtml(creator.name)}</div>
            <div class="creator-sales">${formatNumber(creator.sales)} sales</div>
        </div>
    `).join('');
}

// ===== Load Recent Sales =====
async function loadRecentSales() {
    let sales = [];
    try {
        if (dataSource) {
            sales = await dataSource.getRecentSales(10);
        }
    } catch (err) {
        console.warn('Live sales unavailable:', err.message);
    }

    const tbody = document.getElementById('recentSales');

    if (!sales || sales.length === 0) {
        tbody.innerHTML = '<tr><td colspan="6" style="text-align: center; padding: 2rem; opacity: 0.6;">No recent sales</td></tr>';
        return;
    }
    
    tbody.innerHTML = sales.map(sale => `
        <tr onclick="viewNFT('${escapeHtml(sale.id)}')">
            <td>
                <div class="sale-nft">
                    <div class="sale-nft-image" style="background: ${escapeHtml(sale.image)}"></div>
                    <div>
                        <div class="sale-nft-name">${escapeHtml(sale.nft)}</div>
                        <div class="sale-nft-collection">${escapeHtml(sale.collection)}</div>
                    </div>
                </div>
            </td>
            <td>${escapeHtml(sale.collection)}</td>
            <td class="sale-price">${escapeHtml(sale.price)} MOLT</td>
            <td>
                <a href="#" class="sale-address" onclick="event.stopPropagation(); viewAddress('${escapeHtml(sale.from)}')">
                    ${formatHash(sale.from, 8)}
                </a>
            </td>
            <td>
                <a href="#" class="sale-address" onclick="event.stopPropagation(); viewAddress('${escapeHtml(sale.to)}')">
                    ${formatHash(sale.to, 8)}
                </a>
            </td>
            <td class="sale-time">${timeAgo(sale.timestamp)}</td>
        </tr>
    `).join('');
}

// ===== Update Stats =====
async function updateStats() {
    let stats = null;
    try {
        if (dataSource) {
            stats = await dataSource.getStats();
        }
    } catch (err) {
        console.warn('Live stats unavailable:', err.message);
    }

    if (!stats) {
        stats = { totalNFTs: 0, totalCollections: 0, totalVolume: 0, totalCreators: 0 };
    }

    animateNumber('totalNFTs', stats.totalNFTs || 0, 0);
    animateNumber('totalCollections', stats.totalCollections || 0, 0);
    animateNumber('totalVolume', stats.totalVolume || 0, 0);
    animateNumber('totalCreators', stats.totalCreators || 0, 0);
}

// ===== Connect Wallet =====
function setupConnectWallet() {
    // Use shared MoltWallet if available
    if (window.MoltWallet) {
        window.moltWallet = new MoltWallet({ rpcUrl: RPC_URL });
        window.moltWallet.bindConnectButton('#connectWallet');
        window.moltWallet.onConnect(function(info) {
            currentWallet = info;
            console.log('Wallet connected:', info.address);
        });
        window.moltWallet.onDisconnect(function() {
            currentWallet = null;
            console.log('Wallet disconnected');
        });
        return;
    }
    
    // Fallback: direct button handling
    const button = document.getElementById('connectWallet');
    button.addEventListener('click', async () => {
        if (currentWallet) {
            currentWallet = null;
            button.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet';
        } else {
            button.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Connecting...';
            try {
                var result = await moltRpcCall('createWallet', [], RPC_URL);
                currentWallet = { address: result.address || result, balance: 0 };
                button.innerHTML = '<i class="fas fa-wallet"></i> ' + formatHash(currentWallet.address, 8);
            } catch (e) {
                button.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet';
            }
        }
    });
}

// ===== Search =====
function setupSearch() {
    const searchInput = document.getElementById('searchInput');
    
    searchInput.addEventListener('keypress', (e) => {
        if (e.key === 'Enter') {
            const query = searchInput.value.trim();
            if (query) {
                console.log('Searching for:', query);
                window.location.href = `browse.html?q=${encodeURIComponent(query)}`;
            }
        }
    });
}

// ===== Filter Tabs =====
function setupFilterTabs() {
    const tabs = document.querySelectorAll('.filter-tab');
    
    tabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const period = tab.dataset.period;
            
            // Update active state
            tabs.forEach(t => t.classList.remove('active'));
            tab.classList.add('active');
            
            // Reload trending NFTs with new period
            console.log('Loading trending NFTs for period:', period);
            loadTrendingNFTs(period);
        });
    });
}

// ===== Navigation Functions =====
function viewCollection(id) {
    console.log('Viewing collection:', id);
    window.location.href = `browse.html?collection=${id}`;
}

function viewNFT(id) {
    console.log('Viewing NFT:', id);
    window.location.href = `item.html?id=${id}`;
}

function viewCreator(id) {
    console.log('Viewing creator:', id);
    window.location.href = `profile.html?id=${id}`;
}

function viewAddress(address) {
    console.log('Viewing address:', address);
    // Link to explorer
    window.location.href = `../explorer/address.html?address=${address}`;
}

function buyNFT(id) {
    if (!currentWallet) {
        alert('Please connect your wallet first');
        return;
    }
    
    // Navigate to item page for full purchase flow
    window.location.href = `item.html?id=${id}`;
}

// ===== Utility Functions =====
// formatNumber, formatHash, timeAgo provided by shared/utils.js

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

// ===== Mobile Menu Toggle =====
const navToggle = document.getElementById('navToggle');
if (navToggle) {
    navToggle.addEventListener('click', () => {
        const navMenu = document.querySelector('.nav-menu');
        navMenu.classList.toggle('active');
    });
}

console.log('✅ Molt Market script loaded');
