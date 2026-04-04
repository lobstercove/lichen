function getRequestId() {
  const params = new URLSearchParams(window.location.search);
  return params.get('requestId');
}

function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function normalizeMethod(method) {
  const key = String(method || '').trim();
  const aliases = {
    licn_getAccounts: 'licn_accounts',
    licn_request_accounts: 'licn_requestAccounts',
    licn_sign_message: 'licn_signMessage',
    licn_sign_transaction: 'licn_signTransaction',
    licn_send_transaction: 'licn_sendTransaction',
    personal_sign: 'licn_signMessage',
    eth_sign: 'licn_signMessage',
    eth_signTransaction: 'licn_signTransaction',
    eth_sendTransaction: 'licn_sendTransaction'
  };
  return aliases[key] || key;
}

function isSigningMethod(method) {
  const normalized = normalizeMethod(method);
  return normalized === 'licn_signMessage'
    || normalized === 'licn_signTransaction'
    || normalized === 'licn_sendTransaction';
}

function setDecisionEnabled(enabled) {
  const approveBtn = document.getElementById('approveBtn');
  const rejectBtn = document.getElementById('rejectBtn');
  if (approveBtn) approveBtn.disabled = !enabled;
  if (rejectBtn) rejectBtn.disabled = !enabled;
}

async function loadRequest(requestId) {
  const response = await chrome.runtime.sendMessage({
    type: 'LICHEN_PROVIDER_PENDING_GET',
    requestId
  });

  if (!response?.ok) {
    throw new Error(response?.error || 'Request not found');
  }

  return response.result;
}

async function loadPendingRequests() {
  const response = await chrome.runtime.sendMessage({ type: 'LICHEN_PROVIDER_LIST_PENDING' });
  if (!response?.ok) {
    throw new Error(response?.error || 'Failed to load pending requests');
  }
  return Array.isArray(response.result) ? response.result : [];
}

function renderPendingRequests(items) {
  const root = document.getElementById('pendingRequests');
  if (!root) return;

  if (!items.length) {
    root.innerHTML = '<div>Pending</div><div>No pending requests</div>';
    return;
  }

  root.innerHTML = items.map((item) => `
    <div>Pending</div>
    <div>
      <button class="btn btn-secondary btn-small" data-action="pickPending" data-request-id="${item.requestId}">
        ${escapeHtml(item.method || 'unknown')} • ${escapeHtml(item.origin || 'unknown')}
      </button>
    </div>
  `).join('');
}

async function bindRequest(request) {
  renderRequest(request);
  document.getElementById('approveBtn').onclick = () => decide(request.requestId, true, request);
  document.getElementById('rejectBtn').onclick = () => decide(request.requestId, false, request);
}

async function pickAndLoadRequest(requestId) {
  const status = document.getElementById('approveStatus');
  try {
    const request = await loadRequest(requestId);
    await bindRequest(request);
    setDecisionEnabled(true);
    status.textContent = 'Loaded selected pending request';
  } catch (error) {
    status.textContent = error?.message || String(error);
    setDecisionEnabled(false);
  }
}

async function refreshPendingList() {
  const status = document.getElementById('approveStatus');
  try {
    const pending = await loadPendingRequests();
    renderPendingRequests(pending);
    status.textContent = pending.length ? `${pending.length} pending request(s)` : 'No pending requests';
    return pending;
  } catch (error) {
    status.textContent = error?.message || String(error);
    renderPendingRequests([]);
    return [];
  }
}

function renderRequest(request) {
  const provider = request.providerState || {};
  const accountDisplay = Array.isArray(provider.accounts) && provider.accounts.length
    ? provider.accounts.join(', ')
    : '—';

  const normalizedMethod = normalizeMethod(request.method);
  const paramsDisplay = JSON.stringify(request.params || {}, null, 2);

  const content = document.getElementById('approveContent');
  content.innerHTML = `
    <div>Origin</div><div class="mono">${escapeHtml(request.origin || 'unknown')}</div>
    <div>Method</div><div class="mono">${escapeHtml(normalizedMethod || 'unknown')}</div>
    <div>Network</div><div>${escapeHtml(provider.network || '—')}</div>
    <div>Chain ID</div><div class="mono">${escapeHtml(provider.chainId || '—')}</div>
    <div>Connected</div><div>${provider.connected ? 'Yes' : 'No'}</div>
    <div>Active Account</div><div class="mono">${escapeHtml(provider.activeAddress || '—')}</div>
    <div>Exposed Accounts</div><div class="mono">${escapeHtml(accountDisplay)}</div>
    <div>Wallet Locked</div><div>${provider.isLocked ? 'Yes' : 'No'}</div>
    <div>Created</div><div>${new Date(request.createdAt).toLocaleString()}</div>
    <div>Params</div><div class="mono">${escapeHtml(paramsDisplay)}</div>
  `;

  const needsPassword = isSigningMethod(request.method);
  document.getElementById('passwordRow').style.display = needsPassword ? 'block' : 'none';
}

async function decide(requestId, approved, request) {
  const status = document.getElementById('approveStatus');
  status.textContent = approved ? 'Approving...' : 'Rejecting...';

  const approvalInput = {};
  if (approved && isSigningMethod(request.method)) {
    const password = document.getElementById('approvePassword').value;
    if (!password) {
      status.textContent = 'Password is required for signing';
      return;
    }
    approvalInput.password = password;
  }

  const response = await chrome.runtime.sendMessage({
    type: 'LICHEN_PROVIDER_PENDING_DECIDE',
    requestId,
    approved,
    approvalInput
  });

  if (!response?.ok) {
    status.textContent = response?.error || 'Decision failed';
    setDecisionEnabled(true);
    return;
  }

  document.getElementById('approvePassword').value = '';
  setDecisionEnabled(false);
  status.textContent = approved ? 'Approved' : 'Rejected';
  setTimeout(() => window.close(), 350);
}

async function boot() {
  const requestId = getRequestId();
  const status = document.getElementById('approveStatus');
  const pendingRoot = document.getElementById('pendingRequests');

  document.getElementById('refreshPendingBtn')?.addEventListener('click', refreshPendingList);
  pendingRoot?.addEventListener('click', (event) => {
    const target = event.target;
    if (!(target instanceof HTMLElement)) return;
    if (target.dataset?.action !== 'pickPending') return;
    const pendingRequestId = target.dataset.requestId;
    if (!pendingRequestId) return;
    pickAndLoadRequest(pendingRequestId);
  });

  if (!requestId) {
    const pending = await refreshPendingList();
    if (!pending.length) {
      setDecisionEnabled(false);
      status.textContent = 'No pending requests';
      return;
    }

    await pickAndLoadRequest(pending[0].requestId);
    return;
  }

  try {
    const request = await loadRequest(requestId);
    await refreshPendingList();
    await bindRequest(request);
  } catch (error) {
    status.textContent = error?.message || String(error);
    setDecisionEnabled(false);
  }
}

boot();
