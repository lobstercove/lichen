// MoltChain Explorer – Agent Directory Page
// Table-based layout consistent with blocks.html and transactions.html

const AGENTS_PER_PAGE = 25;
let allAgents = [];
let filteredAgents = [];
let currentPage = 1;

function formatRateMolt(rateRaw) {
    const raw = Number(rateRaw || 0);
    return (raw / 1_000_000_000).toFixed(4);
}

function trustTierLabel(agent) {
    if (agent?.trust_tier_name) return agent.trust_tier_name;
    const rep = Number(agent?.reputation || 0);
    if (rep >= 950) return 'Legendary';
    if (rep >= 800) return 'Elite';
    if (rep >= 600) return 'Established';
    if (rep >= 400) return 'Trusted';
    if (rep >= 200) return 'Verified';
    if (rep >= 100) return 'Newcomer';
    return 'Probation';
}

function tierPillClass(tier) {
    const t = String(tier || '').toLowerCase();
    if (t.includes('legendary') || t.includes('elite')) return 'pill-info';
    if (t.includes('established') || t.includes('trusted')) return 'pill-success';
    if (t.includes('verified')) return 'pill-warning';
    return 'pill';
}

function agentTypeName(agent) {
    const names = {
        0: 'System', 1: 'Trading', 2: 'Development', 3: 'Analysis',
        4: 'Creative', 5: 'Infrastructure', 6: 'Governance', 7: 'Oracle',
        8: 'Storage', 9: 'General'
    };
    if (agent.agent_type_name) return agent.agent_type_name;
    return names[agent.agent_type] || 'Unknown';
}

function normalizeMoltName(name) {
    if (!name) return null;
    return name.endsWith('.molt') ? name : name + '.molt';
}

// ── Data Loading ────────────────────────────────────────────────────────

async function loadAgents() {
    const table = document.getElementById('agentsTable');
    if (!table) return;

    table.innerHTML = '<tr class="loading-row"><td colspan="9"><div class="loading-spinner"></div> Loading agents...</td></tr>';

    try {
        const typeVal = document.getElementById('agentTypeFilter')?.value || 'all';
        const options = { limit: 500, offset: 0 };
        if (typeVal !== 'all') options.type = Number(typeVal);

        const result = await rpc.call('getMoltyIdAgentDirectory', [options]);
        allAgents = result?.agents || (Array.isArray(result) ? result : []);

        // Resolve .molt names for agents that don't have one
        const needNames = allAgents.filter(a => !a.molt_name && a.address).map(a => a.address);
        if (needNames.length > 0 && typeof batchResolveMoltNames === 'function') {
            const nameMap = await batchResolveMoltNames(needNames);
            allAgents = allAgents.map(agent => ({
                ...agent,
                molt_name: agent.molt_name || nameMap[agent.address] || null
            }));
        }

        applySortAndRender();
    } catch (error) {
        console.error('Failed to load agents:', error);
        table.innerHTML = '<tr><td colspan="9" style="text-align:center; color: #FF6B6B;">Failed to load agent directory</td></tr>';
    }
}

// ── Sorting & Filtering ─────────────────────────────────────────────────

function applySortAndRender() {
    const sort = document.getElementById('agentSort')?.value || 'rep-desc';

    filteredAgents = [...allAgents];

    // Stable sort helper: secondary sort by name when primary values are equal
    const byName = (a, b) => (a.molt_name || a.name || '').localeCompare(b.molt_name || b.name || '');

    if (sort === 'rate-asc') {
        filteredAgents.sort((a, b) => Number(a.rate || 0) - Number(b.rate || 0) || byName(a, b));
    } else if (sort === 'newest') {
        filteredAgents.sort((a, b) => Number(b.created_at || 0) - Number(a.created_at || 0) || byName(a, b));
    } else {
        // Default: reputation descending, then alphabetical by name
        filteredAgents.sort((a, b) => Number(b.reputation || 0) - Number(a.reputation || 0) || byName(a, b));
    }

    currentPage = 1;
    renderAgents();
}

// ── Rendering ───────────────────────────────────────────────────────────

function renderAgents() {
    const table = document.getElementById('agentsTable');
    if (!table) return;

    if (filteredAgents.length === 0) {
        table.innerHTML = '<tr><td colspan="9" style="text-align:center; color: var(--text-muted);">No agents found</td></tr>';
        updatePagination();
        return;
    }

    const start = (currentPage - 1) * AGENTS_PER_PAGE;
    const end = start + AGENTS_PER_PAGE;
    const pageAgents = filteredAgents.slice(start, end);

    table.innerHTML = pageAgents.map(agent => {
        const moltName = normalizeMoltName(agent.molt_name);
        const displayName = moltName || escapeHtml(agent.name || '—');
        const addr = agent.address || '';
        const typeName = escapeHtml(agentTypeName(agent));
        const rep = Number(agent.reputation || 0);
        const tier = trustTierLabel(agent);
        const tierClass = tierPillClass(tier);
        const rate = formatRateMolt(agent.rate);
        const available = Number(agent.availability) === 1 || !!agent.available;
        const skillCount = Number(agent.skill_count || 0);
        const vouchCount = Number(agent.vouch_count || 0);

        const nameLink = moltName
            ? `<a href="address.html?address=${addr}&tab=identity" class="agent-name-link">${escapeHtml(moltName)}</a>`
            : `<span style="color: var(--text-muted);">${displayName}</span>`;

        return `
        <tr>
            <td>${nameLink}</td>
            <td>
                <a href="address.html?address=${encodeURIComponent(addr)}&tab=identity" class="hash-short" title="${escapeHtml(addr)}">${formatHash(addr)}</a>
                <i class="fas fa-copy copy-hash" data-copy="${escapeHtml(addr)}" onclick="safeCopy(this)" title="Copy address"></i>
            </td>
            <td><span class="pill">${typeName}</span></td>
            <td>${formatNumber(rep)}</td>
            <td><span class="pill ${tierClass}">${tier}</span></td>
            <td>${rate} MOLT</td>
            <td>${available
                ? '<span class="pill pill-success"><i class="fas fa-circle" style="font-size:0.5em;vertical-align:middle;"></i> Online</span>'
                : '<span class="pill" style="opacity:0.6;"><i class="fas fa-circle" style="font-size:0.5em;vertical-align:middle;"></i> Registered</span>'
            }</td>
            <td>${skillCount}</td>
            <td>${vouchCount}</td>
        </tr>`;
    }).join('');

    updatePagination();
}

// ── Pagination ──────────────────────────────────────────────────────────

function updatePagination() {
    const totalPages = Math.max(1, Math.ceil(filteredAgents.length / AGENTS_PER_PAGE));
    const info = document.getElementById('paginationInfo');
    if (info) info.textContent = `Page ${currentPage} of ${totalPages}`;

    const prevBtn = document.getElementById('prevPage');
    const nextBtn = document.getElementById('nextPage');
    if (prevBtn) prevBtn.disabled = currentPage <= 1;
    if (nextBtn) nextBtn.disabled = currentPage >= totalPages;
}

function nextPage() {
    const totalPages = Math.ceil(filteredAgents.length / AGENTS_PER_PAGE);
    if (currentPage < totalPages) {
        currentPage++;
        renderAgents();
        window.scrollTo({ top: 0, behavior: 'smooth' });
    }
}

function previousPage() {
    if (currentPage > 1) {
        currentPage--;
        renderAgents();
        window.scrollTo({ top: 0, behavior: 'smooth' });
    }
}

// ── Filters ─────────────────────────────────────────────────────────────

function applyFilters() {
    loadAgents();
}

function clearFilters() {
    document.getElementById('agentTypeFilter').value = 'all';
    document.getElementById('agentSort').value = 'rep-desc';
    loadAgents();
}

// ── Initialize ──────────────────────────────────────────────────────────

document.addEventListener('DOMContentLoaded', () => {
    if (typeof initExplorerNetworkSelector === 'function') initExplorerNetworkSelector();
    loadAgents();
});
