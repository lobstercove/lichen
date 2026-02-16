import { getConfiguredWsEndpoint } from './rpc-service.js';

class WalletWsManager {
  constructor() {
    this.socket = null;
    this.currentAddress = null;
    this.currentNetwork = null;
    this.subscriptionId = null;
    this.reconnectTimer = null;
    this.state = 'idle';
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
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect(this.currentAddress, this.currentNetwork);
    }, 5000);
  }

  async connect(address, network) {
    if (!address) return;

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
      this.socket.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'subscribeAccount',
        params: address
      }));
    };

    this.socket.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.id === 1 && msg.result !== undefined) {
          this.subscriptionId = msg.result;
        }
      } catch {
        // ignore
      }
    };

    this.socket.onerror = () => {
      this.state = 'error';
    };

    this.socket.onclose = () => {
      this.subscriptionId = null;
      this.state = 'closed';
      this.scheduleReconnect();
    };
  }
}

export const walletWsManager = new WalletWsManager();
