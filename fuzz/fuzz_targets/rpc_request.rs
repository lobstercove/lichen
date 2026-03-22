#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz JSON-RPC request parsing.
/// The RPC server must handle malformed JSON gracefully without panicking.
fuzz_target!(|data: &[u8]| {
    // Parse as JSON-RPC request — the RPC layer uses serde_json
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct JsonRpcRequest {
        jsonrpc: String,
        id: serde_json::Value,
        method: String,
        params: Option<serde_json::Value>,
    }

    // Must never panic on arbitrary bytes
    let _ = serde_json::from_slice::<JsonRpcRequest>(data);

    // Try as a batch request
    let _ = serde_json::from_slice::<Vec<JsonRpcRequest>>(data);

    // Also try as raw JSON value (catch edge cases)
    let _ = serde_json::from_slice::<serde_json::Value>(data);
});
