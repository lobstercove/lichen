// Blocks Page Logic

let currentPage = 1;
const blocksPerPage = 50;
let allBlocks = [];
let filteredBlocks = [];
let blocksPolling = null;
let lastRenderedSlot = null;
let hasRenderedBlocks = false;
let isLoadingBlocks = false;

async function loadAllBlocks() {
    const table = document.getElementById('blocksTableFull');
    if (!table) return;

    if (isLoadingBlocks) return;
    isLoadingBlocks = true;
    
    try {
        const latestBlock = await rpc.getLatestBlock();
        if (!latestBlock) {
            isLoadingBlocks = false;
            table.innerHTML = '<tr><td colspan="6" style="text-align:center; color: var(--text-muted);">No blocks found</td></tr>';
            return;
        }

        if (lastRenderedSlot !== null && latestBlock.slot === lastRenderedSlot) {
            isLoadingBlocks = false;
            return;
        }
        
        const blocks = [];
        const currentSlot = latestBlock.slot;
        const maxPages = 5;
        const totalToLoad = Math.min(blocksPerPage * maxPages, currentSlot + 1);
        
        // Load blocks in parallel batches of 10
        const BATCH_SIZE = 10;
        for (let start = 0; start < totalToLoad; start += BATCH_SIZE) {
            const batchEnd = Math.min(start + BATCH_SIZE, totalToLoad);
            const promises = [];
            for (let i = start; i < batchEnd; i++) {
                promises.push(rpc.getBlock(currentSlot - i).catch(() => null));
            }
            const results = await Promise.all(promises);
            results.forEach(b => { if (b) blocks.push(b); });
            
            // Update progressively
            if (!hasRenderedBlocks && start % 20 === 0) {
                table.innerHTML = `<tr class="loading-row"><td colspan="6"><div class="loading-spinner"></div> Loading blocks... ${start}/${totalToLoad}</td></tr>`;
            }

            // Brief pause between batches to avoid rate limiting
            if (start + BATCH_SIZE < totalToLoad) {
                await new Promise(r => setTimeout(r, 30));
            }
        }
        
        allBlocks = blocks;
        filteredBlocks = blocks;
        renderBlocks();
        hasRenderedBlocks = true;
        lastRenderedSlot = currentSlot;
        isLoadingBlocks = false;
        
    } catch (error) {
        console.error('Failed to load blocks:', error);
        table.innerHTML = '<tr><td colspan="6" style="text-align:center; color: #FF6B6B;">Failed to load blocks</td></tr>';
        isLoadingBlocks = false;
    }
}

function renderBlocks() {
    const table = document.getElementById('blocksTableFull');
    if (!table) return;
    
    const start = (currentPage - 1) * blocksPerPage;
    const end = start + blocksPerPage;
    const pageBlocks = filteredBlocks.slice(start, end);
    
    if (pageBlocks.length === 0) {
        table.innerHTML = '<tr><td colspan="6" style="text-align:center; color: var(--text-muted);">No blocks found</td></tr>';
        return;
    }
    
    table.innerHTML = pageBlocks.map(block => `
        <tr>
            <td><a href="block.html?slot=${block.slot}">#${formatSlot(block.slot)}</a></td>
            <td>
                <span class="hash-short" title="${escapeHtml(block.hash)}">${formatHash(block.hash)}</span>
                <i class="fas fa-copy copy-hash" data-copy="${escapeHtml(block.hash)}" onclick="safeCopy(this)" title="Copy hash"></i>
            </td>
            <td>
                <span class="hash-short" title="${escapeHtml(block.parent_hash)}">${formatHash(block.parent_hash)}</span>
            </td>
            <td><span class="pill pill-info">${block.transaction_count || 0} txs</span></td>
            <td>${formatValidator(block.validator)}</td>
            <td>${formatTime(block.timestamp)}</td>
        </tr>
    `).join('');
    
    updatePagination();
}

function updatePagination() {
    const totalPages = Math.ceil(filteredBlocks.length / blocksPerPage);
    document.getElementById('paginationInfo').textContent = `Page ${currentPage} of ${totalPages}`;
    
    document.getElementById('prevPage').disabled = currentPage === 1;
    document.getElementById('nextPage').disabled = currentPage >= totalPages;
}

function nextPage() {
    const totalPages = Math.ceil(filteredBlocks.length / blocksPerPage);
    if (currentPage < totalPages) {
        currentPage++;
        renderBlocks();
        window.scrollTo({ top: 0, behavior: 'smooth' });
    }
}

function previousPage() {
    if (currentPage > 1) {
        currentPage--;
        renderBlocks();
        window.scrollTo({ top: 0, behavior: 'smooth' });
    }
}

function applyFilters() {
    const fromSlot = parseInt(document.getElementById('slotFromFilter').value) || 0;
    const toSlot = parseInt(document.getElementById('slotToFilter').value) || Infinity;
    
    filteredBlocks = allBlocks.filter(block => 
        block.slot >= fromSlot && block.slot <= toSlot
    );
    
    currentPage = 1;
    renderBlocks();
}

function clearFilters() {
    document.getElementById('slotFromFilter').value = '';
    document.getElementById('slotToFilter').value = '';
    filteredBlocks = allBlocks;
    currentPage = 1;
    renderBlocks();
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    loadAllBlocks();

    const startPolling = () => {
        if (blocksPolling) return;
        blocksPolling = setInterval(loadAllBlocks, 5000);
    };

    const stopPolling = () => {
        if (blocksPolling) {
            clearInterval(blocksPolling);
            blocksPolling = null;
        }
    };

    if (typeof ws !== 'undefined') {
        ws.onOpen(() => {
            stopPolling();
            ws.subscribe('subscribeBlocks', () => loadAllBlocks());
        });

        ws.onClose(() => {
            startPolling();
        });

        ws.connect();
        setTimeout(() => {
            if (!ws.isConnected()) {
                startPolling();
            }
        }, 2000);
    } else {
        startPolling();
    }
});
