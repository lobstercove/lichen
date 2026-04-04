// Lichen Market — Network Configuration
// Now delegates to shared-config.js (LICHEN_CONFIG) for centralized env/network management.
// Must be loaded AFTER shared-config.js and BEFORE marketplace.js, marketplace-data.js, etc.

(function () {
    'use strict';

    var STORAGE_KEY = 'lichenmarket_network';

    var currentNetwork = LICHEN_CONFIG.currentNetwork(STORAGE_KEY);
    var config = LICHEN_CONFIG.networks[currentNetwork];

    // Expose global config (browse.js, item.js, create.js, profile.js all read this)
    window.lichenMarketConfig = {
        rpcUrl: config.rpc,
        wsUrl: config.ws,
        network: currentNetwork,
        networks: LICHEN_CONFIG.networks,
    };

    // Network selector initialization (call from DOMContentLoaded)
    window.initMarketNetworkSelector = function () {
        LICHEN_CONFIG.initNetworkSelector('marketNetworkSelect', STORAGE_KEY, function (network) {
            localStorage.setItem(STORAGE_KEY, network);
            window.location.reload();
        });
    };

    window.getMarketRpcUrl = function () {
        return window.lichenMarketConfig.rpcUrl;
    };

    window.getTrustedMarketNetwork = function () {
        return (window.lichenMarketConfig && window.lichenMarketConfig.network)
            || LICHEN_CONFIG.currentNetwork(STORAGE_KEY);
    };

    window.marketTrustedRpcCall = function (method, params) {
        if (typeof signedMetadataRpcCall === 'function') {
            return signedMetadataRpcCall(method, params, window.getTrustedMarketNetwork(), function (resolvedMethod, resolvedParams) {
                if (typeof trustedLichenRpcCall === 'function') {
                    return trustedLichenRpcCall(resolvedMethod, resolvedParams, window.getTrustedMarketNetwork());
                }
                return rpcCall(resolvedMethod, resolvedParams, LICHEN_CONFIG.rpc(window.getTrustedMarketNetwork()));
            });
        }
        if (typeof trustedLichenRpcCall === 'function') {
            return trustedLichenRpcCall(method, params, window.getTrustedMarketNetwork());
        }
        return rpcCall(method, params, LICHEN_CONFIG.rpc(window.getTrustedMarketNetwork()));
    };

    window.setMarketNetwork = function (name) {
        var resolved = LICHEN_CONFIG.resolveNetwork(name);
        localStorage.setItem(STORAGE_KEY, resolved);
        window.location.reload();
    };

})();
