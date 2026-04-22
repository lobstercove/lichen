use anyhow::Result;

use crate::client::RpcClient;
use crate::defi_output_support::print_defi_protocol_view;
use crate::output_support::print_json;

pub(super) async fn handle_defi_protocol(
    client: &RpcClient,
    method: &str,
    title: &str,
    error_label: &str,
    json_output: bool,
) -> Result<()> {
    match client.get_defi_stats(method).await {
        Ok(stats) => {
            if json_output {
                print_json(&stats);
            } else {
                print_defi_protocol_view(title, &stats);
            }
        }
        Err(error) => println!("Could not fetch {} stats: {}", error_label, error),
    }

    Ok(())
}
