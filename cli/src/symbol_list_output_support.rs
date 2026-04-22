pub(super) fn print_symbol_registry(entries: &serde_json::Value) {
    println!("🏷️  Symbol Registry");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    if let Some(arr) = entries.as_array() {
        println!("{:<12} {:<30} {:<10} Address", "Symbol", "Name", "Template");
        println!("{}", "─".repeat(90));
        for entry in arr {
            let sym = entry
                .get("symbol")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            let name = entry
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            let tmpl = entry
                .get("template")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            let addr = entry
                .get("program")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            let addr_short = if addr.len() > 16 { &addr[..16] } else { addr };
            println!("{:<12} {:<30} {:<10} {}...", sym, name, tmpl, addr_short);
        }
        println!("\nTotal: {} symbols registered", arr.len());
    }
}