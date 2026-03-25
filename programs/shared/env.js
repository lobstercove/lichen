// Lichen Environment Configuration
// -----------------------------------
// This file sets the environment flag for all frontends.
//
// Auto-detects production (*.lichen.network) vs development (localhost).
//
// This file must be loaded BEFORE shared-config.js in every HTML page.
// Override: set window.LICHEN_ENV = 'production' or 'development' before
// this script loads to force a specific environment.

if (typeof window.LICHEN_ENV === 'undefined') {
    const _h = window.location.hostname;
    window.LICHEN_ENV = (_h === 'localhost' || _h === '127.0.0.1')
        ? 'development'
        : 'production';
}
