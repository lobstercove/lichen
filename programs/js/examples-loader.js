/**
 * Lichen Playground - Real Example Contracts
 * Production-ready contracts ready to deploy and test
 */

// Load example contracts from files
const REAL_EXAMPLES = {};

// Load examples via fetch
async function loadRealExamples() {
    const examples = ['token', 'nft', 'dex', 'dao'];
    
    for (const example of examples) {
        try {
            const response = await fetch(`examples/${example}.rs`);
            if (response.ok) {
                const code = await response.text();
                REAL_EXAMPLES[example] = code;
            }
        } catch (e) {
            console.warn(`Failed to load example: ${example}`, e);
        }
    }
    
    return REAL_EXAMPLES;
}

// Export for use in playground
window.loadRealExamples = loadRealExamples;
window.REAL_EXAMPLES = REAL_EXAMPLES;
