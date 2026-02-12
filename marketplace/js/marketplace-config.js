// Molt Market — Network Configuration
// Shared network config for all marketplace pages
// Must be loaded BEFORE marketplace.js, marketplace-data.js, and page-specific scripts

(function () {
    'use strict';

    var NETWORKS = {
        mainnet: {
            rpc: 'https://rpc.moltchain.network',
            ws: null,
            label: 'Mainnet',
        },
        testnet: {
            rpc: 'https://testnet-rpc.moltchain.network',
            ws: null,
            label: 'Testnet',
        },
        'local-testnet': {
            rpc: 'http://localhost:8899',
            ws: 'ws://localhost:8900',
            label: 'Local Testnet',
        },
        'local-mainnet': {
            rpc: 'http://localhost:9899',
            ws: 'ws://localhost:9900',
            label: 'Local Mainnet',
        }
    };

    var STORAGE_KEY = 'moltmarket_network';

    function resolveNetwork(name) {
        if (name === 'local') return 'local-testnet';
        return NETWORKS[name] ? name : 'local-testnet';
    }

    var currentNetwork = resolveNetwork(localStorage.getItem(STORAGE_KEY) || 'local-testnet');
    var config = NETWORKS[currentNetwork];

    // Expose global config (browsed.js, item.js, create.js, profile.js all read this)
    window.moltMarketConfig = {
        rpcUrl: config.rpc,
        wsUrl: config.ws,
        network: currentNetwork,
        networks: NETWORKS,
    };

    // Network selector initialization (call from DOMContentLoaded)
    window.initMarketNetworkSelector = function () {
        var select = document.getElementById('marketNetworkSelect');
        if (!select) return;

        // Populate options if empty
        if (select.options.length === 0) {
            Object.keys(NETWORKS).forEach(function (key) {
                var opt = document.createElement('option');
                opt.value = key;
                opt.textContent = NETWORKS[key].label;
                select.appendChild(opt);
            });
        }

        select.value = currentNetwork;
        select.addEventListener('change', function () {
            var newNetwork = resolveNetwork(select.value);
            localStorage.setItem(STORAGE_KEY, newNetwork);
            window.location.reload();
        });
    };

    window.getMarketRpcUrl = function () {
        return window.moltMarketConfig.rpcUrl;
    };

    window.setMarketNetwork = function (name) {
        var resolved = resolveNetwork(name);
        localStorage.setItem(STORAGE_KEY, resolved);
        window.location.reload();
    };

})();
