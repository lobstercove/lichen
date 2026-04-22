pub(super) fn print_symbol_by_address(address: &str, entry: &serde_json::Value) {
    let sym = entry
        .get("symbol")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let name = entry
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let tmpl = entry
        .get("template")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    println!("🏷️  {} — {} ({})", sym, name, tmpl);
    println!("Address: {}", address);
}