use anyhow::Result;

use crate::client::RpcClient;
use crate::output_support::{print_json, to_licn};

pub(super) async fn handle_epoch(client: &RpcClient, json_output: bool) -> Result<()> {
    match client.get_chain_status().await {
        Ok(status) => {
            let slots_per_epoch = client
                .get_reward_adjustment_info()
                .await
                .map(|info| info.slots_per_epoch)
                .ok()
                .filter(|slots| *slots > 0)
                .unwrap_or(lichen_core::consensus::SLOTS_PER_EPOCH);
            let epoch = status._epoch;
            let epoch_start_slot = epoch.saturating_mul(slots_per_epoch);
            let slot_in_epoch = status.current_slot.saturating_sub(epoch_start_slot);
            let epoch_progress_pct = if slots_per_epoch > 0 {
                (slot_in_epoch as f64 / slots_per_epoch as f64) * 100.0
            } else {
                0.0
            };

            if json_output {
                print_json(&serde_json::json!({
                    "current_epoch": epoch,
                    "current_slot": status.current_slot,
                    "slot_in_epoch": slot_in_epoch,
                    "slots_per_epoch": slots_per_epoch,
                    "epoch_progress_pct": epoch_progress_pct,
                    "validators": status.validator_count,
                    "total_staked_licn": to_licn(status.total_staked),
                }));
            } else {
                println!("📅 Epoch Information");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!("Current Epoch: {}", epoch);
                println!(
                    "Current Slot:  {} ({}/{} in epoch)",
                    status.current_slot, slot_in_epoch, slots_per_epoch
                );
                println!("Progress:      {:.1}%", epoch_progress_pct);
                println!("Validators:    {}", status.validator_count);
                println!("Total Staked:  {:.4} LICN", to_licn(status.total_staked));
            }
        }
        Err(error) => println!("Could not fetch epoch info: {}", error),
    }

    Ok(())
}
