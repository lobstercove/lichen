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
    eth_sendTransaction: 'licn_sendTransaction',
    wallet_switchEthereumChain: 'licn_switchNetwork',
    wallet_addEthereumChain: 'licn_addNetwork'
  };
  return aliases[key] || key;
}

function isSigningMethod(method) {
  const normalized = normalizeMethod(method);
  return normalized === 'licn_signMessage'
    || normalized === 'licn_signTransaction'
    || normalized === 'licn_sendTransaction';
}

function isNetworkChangeMethod(method) {
  const normalized = normalizeMethod(method);
  return normalized === 'licn_switchNetwork' || normalized === 'licn_addNetwork';
}

function setDecisionEnabled(enabled) {
  const approveBtn = document.getElementById('approveBtn');
  const rejectBtn = document.getElementById('rejectBtn');
  if (approveBtn) approveBtn.disabled = !enabled;
  if (rejectBtn) rejectBtn.disabled = !enabled;
}

function setApproveEnabled(enabled) {
  const approveBtn = document.getElementById('approveBtn');
  if (approveBtn) approveBtn.disabled = !enabled;
}

function restrictionSummary(preflight) {
  if (!preflight || typeof preflight !== 'object') return '';
  if (Array.isArray(preflight.blocks) && preflight.blocks.length) {
    return preflight.blocks.join(' | ');
  }
  if (Array.isArray(preflight.warnings) && preflight.warnings.length) {
    return preflight.warnings.join(' | ');
  }
  if (preflight.skipped) return '';
  return 'Restriction preflight passed';
}

function requestHasBlockingRestriction(request) {
  return request?.restrictionPreflight?.allowed === false;
}

function requestGrantsAccountAccess(request) {
  const provider = request?.providerState || {};
  return Boolean(request?.grantsAccountAccess)
    || (isSigningMethod(request?.method) && !provider.connected && Boolean(request?.origin));
}

function renderNetworkChangeRows(networkChange) {
  if (!networkChange || typeof networkChange !== 'object') return '';
  const action = networkChange.kind === 'add' ? 'Add & switch network' : 'Switch network';
  const rpcRows = networkChange.kind === 'add'
    ? `
    <div>RPC Origin</div><div class="mono">${escapeHtml(networkChange.rpcOrigin || '—')}</div>
    <div>RPC URL</div><div class="mono">${escapeHtml(networkChange.rpcEndpoint || '—')}</div>
  `
    : '';
  return `
    <div>Network Action</div><div>${escapeHtml(action)}</div>
    <div>From Network</div><div>${escapeHtml(networkChange.previousNetwork || '—')}</div>
    <div>From Chain ID</div><div class="mono">${escapeHtml(networkChange.previousChainId || '—')}</div>
    <div>To Network</div><div>${escapeHtml(networkChange.nextNetwork || '—')}</div>
    <div>To Chain ID</div><div class="mono">${escapeHtml(networkChange.nextChainId || networkChange.requestedChainId || '—')}</div>
    ${rpcRows}
  `;
}

function renderIntentRow(label, value, mono = false) {
  const className = mono ? ' class="mono"' : '';
  return `<div>${escapeHtml(label)}</div><div${className}>${escapeHtml(value ?? '—')}</div>`;
}

function renderTransactionIntentRows(transactionIntent) {
  if (!transactionIntent || typeof transactionIntent !== 'object') return '';
  const rows = [
    ['Intent', transactionIntent.intent],
    ['Instructions', transactionIntent.instructionCount],
    ['Account', transactionIntent.account, true],
    ['Destination', transactionIntent.destination, true],
    ['Amount', transactionIntent.amount],
    ['Token', transactionIntent.token],
    ['Token Decimals', transactionIntent.tokenDecimals],
    ['Network', transactionIntent.network],
    ['RPC', transactionIntent.rpc, true],
    ['Fee', transactionIntent.fee],
    ['Program', transactionIntent.program],
    ['Blockhash', transactionIntent.blockhash, true],
    ['Source Format', transactionIntent.sourceFormat]
  ];

  if (transactionIntent.contract) rows.splice(11, 0, ['Contract', transactionIntent.contract, true]);
  if (transactionIntent.computeBudget) rows.push(['Compute Budget', transactionIntent.computeBudget]);
  if (transactionIntent.computeUnitPrice) rows.push(['Compute Unit Price', transactionIntent.computeUnitPrice]);
  if (Array.isArray(transactionIntent.warnings) && transactionIntent.warnings.length) {
    rows.push(['Warnings', transactionIntent.warnings.join(' ')]);
  }

  return rows.map(([label, value, mono]) => renderIntentRow(label, value, Boolean(mono))).join('');
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
        ${escapeHtml(item.method || 'unknown')} • ${escapeHtml(item.origin || 'unknown')}${item.restrictionBlocked ? ' • blocked' : ''}
      </button>
    </div>
  `).join('');
}

async function bindRequest(request) {
  renderRequest(request);
  document.getElementById('approveBtn').onclick = () => decide(request.requestId, true, request);
  document.getElementById('rejectBtn').onclick = () => decide(request.requestId, false, request);
  return requestHasBlockingRestriction(request);
}

async function pickAndLoadRequest(requestId) {
  const status = document.getElementById('approveStatus');
  try {
    const request = await loadRequest(requestId);
    const blocked = await bindRequest(request);
    setDecisionEnabled(true);
    setApproveEnabled(!blocked);
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
  const grantsAccountAccess = requestGrantsAccountAccess(request);
  const grantAccountAddress = request.grantAccountAddress || provider.activeAddress || 'active account';
  const networkChangeRows = renderNetworkChangeRows(request.networkChange);
  const transactionIntentRows = renderTransactionIntentRows(request.transactionIntent);
  const accountAccessConsent = grantsAccountAccess
    ? `
    <div>Account Access</div>
    <div>Approving also connects this site to <span class="mono">${escapeHtml(grantAccountAddress)}</span> for 30 days or until disconnected.</div>
  `
    : '';

  const content = document.getElementById('approveContent');
  content.innerHTML = `
    <div>Origin</div><div class="mono">${escapeHtml(request.origin || 'unknown')}</div>
    <div>Method</div><div class="mono">${escapeHtml(normalizedMethod || 'unknown')}</div>
    <div>Network</div><div>${escapeHtml(provider.network || '—')}</div>
    <div>Chain ID</div><div class="mono">${escapeHtml(provider.chainId || '—')}</div>
    <div>Connected</div><div>${provider.connected ? 'Yes' : 'No'}</div>
    ${accountAccessConsent}
    ${networkChangeRows}
    ${transactionIntentRows}
    <div>Active Account</div><div class="mono">${escapeHtml(provider.activeAddress || '—')}</div>
    <div>Exposed Accounts</div><div class="mono">${escapeHtml(accountDisplay)}</div>
    <div>Wallet Locked</div><div>${provider.isLocked ? 'Yes' : 'No'}</div>
    <div>Created</div><div>${new Date(request.createdAt).toLocaleString()}</div>
    <div>Params</div><div class="mono">${escapeHtml(paramsDisplay)}</div>
  `;

  const needsPassword = isSigningMethod(request.method);
  document.getElementById('passwordRow').style.display = needsPassword ? 'block' : 'none';
  const approveBtn = document.getElementById('approveBtn');
  if (approveBtn) {
    let approveText = 'Approve';
    if (normalizedMethod === 'licn_requestAccounts') {
      approveText = 'Connect';
    } else if (grantsAccountAccess) {
      approveText = 'Approve & Connect';
    } else if (isNetworkChangeMethod(normalizedMethod)) {
      approveText = request.networkChange?.kind === 'add' ? 'Add & Switch Network' : 'Switch Network';
    }
    approveBtn.textContent = approveText;
  }

  const restrictionEl = document.getElementById('approveRestrictionStatus');
  const preflight = request.restrictionPreflight || null;
  const summary = restrictionSummary(preflight);
  if (!restrictionEl) return;
  if (!summary) {
    restrictionEl.className = 'approve-restriction-status';
    restrictionEl.textContent = '';
    return;
  }
  if (preflight?.allowed === false) {
    restrictionEl.className = 'approve-restriction-status blocked';
    restrictionEl.textContent = `Blocked by consensus restriction preflight: ${summary}`;
  } else if (Array.isArray(preflight?.warnings) && preflight.warnings.length) {
    restrictionEl.className = 'approve-restriction-status warning';
    restrictionEl.textContent = `Restriction preflight warning: ${summary}`;
  } else {
    restrictionEl.className = 'approve-restriction-status passed';
    restrictionEl.textContent = summary;
  }
}

async function decide(requestId, approved, request) {
  const status = document.getElementById('approveStatus');
  status.textContent = approved ? 'Approving...' : 'Rejecting...';

  if (approved && requestHasBlockingRestriction(request)) {
    status.textContent = restrictionSummary(request.restrictionPreflight) || 'Blocked by consensus restriction preflight';
    setApproveEnabled(false);
    return;
  }

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
    const blocked = await bindRequest(request);
    setApproveEnabled(!blocked);
  } catch (error) {
    status.textContent = error?.message || String(error);
    setDecisionEnabled(false);
  }
}

boot();
