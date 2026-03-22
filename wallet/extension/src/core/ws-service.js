import { getConfiguredWsEndpoint } from './rpc-service.js';

class WalletWsManager {
  constructor() {
    this.socket = null;
    this.currentAddress = null;
    this.currentNetwork = null;
    this.subscriptionId = null;
    this.reconnectTimer = null;
    this.keepaliveTimer = null;
    this.reconnectDelay = 1000;
    this.state = 'idle';
    this.listeners = new Set();
  }

  addListener(listener) {
    if (typeof listener === 'function') this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  emit(event) {
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch {
        // ignore listener errors
      }
    }
  }

  async endpoint(network) {
    return getConfiguredWsEndpoint(network);
  }

  status() {
    return {
      state: this.state,
      address: this.currentAddress,
      network: this.currentNetwork,
      subscribed: Boolean(this.subscriptionId)
    };
  }

  stopReconnect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  disconnect() {
    this.stopReconnect();
    if (this.keepaliveTimer) { clearInterval(this.keepaliveTimer); this.keepaliveTimer = null; }
    if (this.socket) {
      this.socket.onclose = null;
      this.socket.close();
      this.socket = null;
    }
    this.subscriptionId = null;
    this.state = 'idle';
  }

  scheduleReconnect() {
    if (this.reconnectTimer || !this.currentAddress || !this.currentNetwork) return;
    const delay = this.reconnectDelay;
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30000);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect(this.currentAddress, this.currentNetwork);
    }, delay);
  }

  async connect(address, network) {
    if (!address) return;

    // Don't reconnect if already connected/connecting for this address+network
    if (this.socket && this.currentAddress === address && this.currentNetwork === network) {
      if (this.socket.readyState === WebSocket.OPEN || this.socket.readyState === WebSocket.CONNECTING) {
        return;
      }
    }

    this.currentAddress = address;
    this.currentNetwork = network;

    if (this.socket) {
      this.disconnect();
    }

    const wsUrl = await this.endpoint(network);
    this.state = 'connecting';

    try {
      this.socket = new WebSocket(wsUrl);
    } catch {
      this.state = 'error';
      this.scheduleReconnect();
      return;
    }

    this.socket.onopen = () => {
      this.state = 'open';
      this.reconnectDelay = 1000;  // Reset backoff on success
      this.socket.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'subscribeAccount',
        params: address
      }));
      // Client-side keepalive ping every 25s
      if (this.keepaliveTimer) clearInterval(this.keepaliveTimer);
      this.keepaliveTimer = setInterval(() => {
        if (this.socket && this.socket.readyState === WebSocket.OPEN) {
          this.socket.send(JSON.stringify({ method: 'ping' }));
        }
      }, 25000);
    };

    this.socket.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.id === 1 && msg.result !== undefined) {
          this.subscriptionId = msg.result;
          this.emit({ type: 'subscribed', subscriptionId: this.subscriptionId });
          return;
        }
        if (msg.method === 'subscription' && msg.params?.result) {
          this.emit({
            type: 'account-change',
            subscriptionId: msg.params?.subscription,
            payload: msg.params.result
          });
        }
      } catch {
        // ignore
      }
    };

    this.socket.onerror = () => {
      this.state = 'error';
    };

    this.socket.onclose = () => {
      if (this.keepaliveTimer) { clearInterval(this.keepaliveTimer); this.keepaliveTimer = null; }
      this.subscriptionId = null;
      this.state = 'closed';
      this.emit({ type: 'closed' });
      this.scheduleReconnect();
    };
  }
}

export const walletWsManager = new WalletWsManager();
