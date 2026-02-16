(() => {
  let providerMonitorStarted = false;
  let providerStateSnapshot = {
    connected: null,
    chainId: null,
    accounts: []
  };

  function postProviderEvent(event, payload) {
    window.postMessage(
      {
        target: 'MOLT_INPAGE_EVENT',
        event,
        payload
      },
      '*'
    );
  }

  function sameAccounts(left, right) {
    if (!Array.isArray(left) || !Array.isArray(right)) return false;
    if (left.length !== right.length) return false;
    for (let i = 0; i < left.length; i++) {
      if (left[i] !== right[i]) return false;
    }
    return true;
  }

  async function fetchProviderState() {
    const response = await chrome.runtime.sendMessage({
      type: 'MOLT_PROVIDER_REQUEST',
      payload: { method: 'molt_getProviderState' }
    });

    if (!response?.ok) {
      return null;
    }

    const state = response.result || {};
    return {
      connected: Boolean(state.connected),
      chainId: state.chainId || null,
      accounts: Array.isArray(state.accounts) ? state.accounts : []
    };
  }

  async function checkProviderStateAndEmit() {
    const current = await fetchProviderState().catch(() => null);
    if (!current) return;

    const previous = providerStateSnapshot;

    if (previous.connected !== null && previous.connected !== current.connected) {
      postProviderEvent(current.connected ? 'connect' : 'disconnect', {
        chainId: current.chainId
      });
    }

    if (previous.chainId !== null && previous.chainId !== current.chainId) {
      postProviderEvent('chainChanged', current.chainId);
    }

    if (!sameAccounts(previous.accounts, current.accounts)) {
      postProviderEvent('accountsChanged', current.accounts);
    }

    providerStateSnapshot = current;
  }

  async function startProviderMonitor() {
    if (providerMonitorStarted) return;
    providerMonitorStarted = true;

    await checkProviderStateAndEmit();
    setInterval(() => {
      checkProviderStateAndEmit();
    }, 2000);
  }

  async function waitForProviderDecision(requestId, timeoutMs = 120000) {
    const started = Date.now();

    while (Date.now() - started < timeoutMs) {
      const result = await chrome.runtime.sendMessage({
        type: 'MOLT_PROVIDER_RESULT',
        requestId
      });

      if (result?.pending) {
        await new Promise((resolve) => setTimeout(resolve, 1000));
        continue;
      }

      return result;
    }

    return {
      ok: false,
      error: 'Approval timed out'
    };
  }

  const injected = document.createElement('script');
  injected.src = chrome.runtime.getURL('src/content/inpage-provider.js');
  injected.type = 'text/javascript';
  injected.async = false;
  (document.head || document.documentElement).appendChild(injected);
  injected.remove();

  startProviderMonitor();

  window.addEventListener('message', async (event) => {
    if (event.source !== window) return;
    if (!event.data || event.data.target !== 'MOLT_EXTENSION') return;

    const { id, payload } = event.data;

    try {
      let response = await chrome.runtime.sendMessage({
        type: 'MOLT_PROVIDER_REQUEST',
        payload
      });

      if (response?.pending && response?.requestId) {
        response = await waitForProviderDecision(response.requestId);
      }

      window.postMessage(
        {
          target: 'MOLT_INPAGE',
          id,
          response
        },
        '*'
      );
    } catch (error) {
      window.postMessage(
        {
          target: 'MOLT_INPAGE',
          id,
          response: { ok: false, error: String(error?.message || error) }
        },
        '*'
      );
    }
  });
})();
