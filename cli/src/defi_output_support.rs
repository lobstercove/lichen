use crate::output_support::print_defi_stats;

pub(super) fn print_defi_protocol_view(title: &str, stats: &serde_json::Value) {
    println!("📊 {}", title);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    print_defi_stats(stats);
}

pub(super) fn print_defi_overview_view(
    labels: &[&str],
    results: &[(String, Option<serde_json::Value>)],
) {
    println!("📊 DeFi Protocol Overview");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    for (label, (_, maybe_stats)) in labels.iter().zip(results.iter()) {
        println!("\n{}:", label);
        match maybe_stats {
            Some(stats) => print_defi_stats(stats),
            None => println!("  (unavailable)"),
        }
    }
}