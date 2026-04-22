pub(super) fn print_symbol_lookup(symbol: &str, entry: &serde_json::Value) {
    println!("🏷️  Symbol: {}", symbol.to_uppercase());
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    if let Some(name) = entry.get("name").and_then(|value| value.as_str()) {
        println!("Name:     {}", name);
    }
    if let Some(program) = entry.get("program").and_then(|value| value.as_str()) {
        println!("Address:  {}", program);
    }
    if let Some(owner) = entry.get("owner").and_then(|value| value.as_str()) {
        println!("Owner:    {}", owner);
    }
    if let Some(template) = entry.get("template").and_then(|value| value.as_str()) {
        println!("Template: {}", template);
    }
    if let Some(decimals) = entry.get("decimals").and_then(|value| value.as_u64()) {
        println!("Decimals: {}", decimals);
    }
}