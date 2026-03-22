(() => {
  if (window.moltwallet) return;

  let requestId = 0;
  const pending = new Map();
  const listeners = new Map();

  function addListener(event, handler) {
    if (!event || typeof handler !== 'function') return;
    const list = listeners.get(event) || new Set();
    list.add(handler);
    listeners.set(event, list);
  }

  function removeListener(event, handler) {
    const list = listeners.get(event);
    if (!list) return;
    list.delete(handler);
    if (list.size === 0) {
      listeners.delete(event);
    }
  }

  function emit(event, payload) {
    const list = listeners.get(event);
    if (!list || list.size === 0) return;

    for (const handler of list) {
      try {
        handler(payload);
      } catch {
        // keep other listeners running
      }
    }
  }

  function normalizeRequestInput(payloadOrMethod, maybeParams) {
    if (typeof payloadOrMethod === 'string') {
      return {
        method: payloadOrMethod,
        params: maybeParams === undefined ? [] : maybeParams
      };
    }

    if (payloadOrMethod && typeof payloadOrMethod === 'object') {
      return payloadOrMethod;
    }

    throw new Error('Invalid request payload');
  }

  function sendRequest(payloadOrMethod, maybeParams) {
    const payload = normalizeRequestInput(payloadOrMethod, maybeParams);
    const id = ++requestId;

    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });

      window.postMessage(
        {
          target: 'MOLT_EXTENSION',
          id,
          payload
        },
        '*'
      );

      setTimeout(() => {
        if (!pending.has(id)) return;
        pending.delete(id);
        reject(new Error('MoltWallet request timed out'));
      }, 120000);
    });
  }

  function ethereumRequest(payloadOrMethod, maybeParams) {
    const payload = normalizeRequestInput(payloadOrMethod, maybeParams);
    const method = String(payload?.method || '').trim();
    const allowedNamespace = /^(eth_|net_|web3_|wallet_)/.test(method);

    if (!allowedNamespace) {
      return Promise.reject(new Error(`Unsupported window.ethereum method: ${method || '<empty>'}`));
    }

    return sendRequest({
      ...payload,
      method,
      params: payload.params === undefined ? [] : payload.params
    });
  }

  window.addEventListener('message', (event) => {
    if (event.source !== window) return;
    if (!event.data) return;

    if (event.data.target === 'MOLT_INPAGE_EVENT') {
      emit(event.data.event, event.data.payload);
      return;
    }

    if (event.data.target !== 'MOLT_INPAGE') return;

    const { id, response } = event.data;
    const active = pending.get(id);
    if (!active) return;

    pending.delete(id);

    if (response?.ok) {
      active.resolve(response.result);
      return;
    }

    active.reject(new Error(response?.error || 'Unknown MoltWallet error'));
  });

  window.moltwallet = {
    isMoltWallet: true,
    on: addListener,
    removeListener,
    request: sendRequest,
    getProviderState: () => sendRequest({ method: 'molt_getProviderState' }),
    isConnected: () => sendRequest({ method: 'molt_isConnected' }),
    chainId: () => sendRequest({ method: 'molt_chainId' }),
    network: () => sendRequest({ method: 'molt_network' }),
    version: () => sendRequest({ method: 'molt_version' }),
    accounts: () => sendRequest({ method: 'molt_accounts' }),
    requestAccounts: () => sendRequest({ method: 'molt_requestAccounts' }),
    connect: () => sendRequest({ method: 'molt_connect' }),
    disconnect: () => sendRequest({ method: 'molt_disconnect' }),
    getPermissions: () => sendRequest({ method: 'molt_getPermissions' }),
    revokePermissions: () => sendRequest({ method: 'wallet_revokePermissions' }),
    getBalance: (address) => sendRequest({ method: 'molt_getBalance', params: [{ address }] }),
    getAccount: (address) => sendRequest({ method: 'molt_getAccount', params: [{ address }] }),
    getLatestBlock: () => sendRequest({ method: 'molt_getLatestBlock' }),
    getTransactions: (address, limit = 20) => sendRequest({ method: 'molt_getTransactions', params: [{ address, limit }] }),
    signMessage: (message) => sendRequest({ method: 'molt_signMessage', params: [{ message }] }),
    signTransaction: (transaction) => sendRequest({ method: 'molt_signTransaction', params: [{ transaction }] }),
    sendTransaction: (transaction) => sendRequest({ method: 'molt_sendTransaction', params: [{ transaction }] })
  };

  if (!window.ethereum) {
    window.ethereum = {
      isMetaMask: false,
      on: addListener,
      removeListener,
      request: ethereumRequest,
      isConnected: () => ethereumRequest({ method: 'eth_accounts' }).then((accounts) => Array.isArray(accounts) && accounts.length > 0),
      selectedAddress: null,
      enable: () => ethereumRequest({ method: 'eth_requestAccounts' }),
      send: (payloadOrMethod, maybeParamsOrCallback) => {
        if (typeof maybeParamsOrCallback === 'function') {
          ethereumRequest(payloadOrMethod)
            .then((result) => maybeParamsOrCallback(null, { jsonrpc: '2.0', id: payloadOrMethod?.id ?? null, result }))
            .catch((error) => maybeParamsOrCallback(error, null));
          return;
        }
        return ethereumRequest(payloadOrMethod, maybeParamsOrCallback);
      },
      sendAsync: (payload, callback) => {
        ethereumRequest(payload)
          .then((result) => callback?.(null, { jsonrpc: '2.0', id: payload?.id ?? null, result }))
          .catch((error) => callback?.(error, null));
      }
    };

    window.moltwallet.on('accountsChanged', (accounts) => {
      window.ethereum.selectedAddress = Array.isArray(accounts) && accounts.length ? accounts[0] : null;
    });

    window.dispatchEvent(new Event('ethereum#initialized'));
  }

  window.dispatchEvent(new Event('moltwallet#initialized'));
})();
