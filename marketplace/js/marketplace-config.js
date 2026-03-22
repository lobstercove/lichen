// Molt Market — Network Configuration
// Now delegates to shared-config.js (MOLT_CONFIG) for centralized env/network management.
// Must be loaded AFTER shared-config.js and BEFORE marketplace.js, marketplace-data.js, etc.

(function () {
    'use strict';

    var STORAGE_KEY = 'moltmarket_network';

    var currentNetwork = MOLT_CONFIG.currentNetwork(STORAGE_KEY);
    var config = MOLT_CONFIG.networks[currentNetwork];

    // Expose global config (browse.js, item.js, create.js, profile.js all read this)
    window.moltMarketConfig = {
        rpcUrl: config.rpc,
        wsUrl: config.ws,
        network: currentNetwork,
        networks: MOLT_CONFIG.networks,
    };

    // Network selector initialization (call from DOMContentLoaded)
    window.initMarketNetworkSelector = function () {
        MOLT_CONFIG.initNetworkSelector('marketNetworkSelect', STORAGE_KEY, function (network) {
            localStorage.setItem(STORAGE_KEY, network);
            window.location.reload();
        });
    };

    window.getMarketRpcUrl = function () {
        return window.moltMarketConfig.rpcUrl;
    };

    window.setMarketNetwork = function (name) {
        var resolved = MOLT_CONFIG.resolveNetwork(name);
        localStorage.setItem(STORAGE_KEY, resolved);
        window.location.reload();
    };

})();
