use anyhow::Result;
use std::path::PathBuf;

use crate::cli_args::ConfigCommands;
use crate::config::CliConfig;
use crate::output_support::print_json;

pub(super) fn handle_config_command(config_cmd: ConfigCommands, json_output: bool) -> Result<()> {
    match config_cmd {
        ConfigCommands::Show => {
            let cfg = CliConfig::load(None, None)?;
            if json_output {
                print_json(&serde_json::json!({
                    "rpc_url": cfg.rpc_url,
                    "ws_url": cfg.ws_url,
                    "keypair": cfg.keypair,
                    "config_path": CliConfig::default_path().display().to_string(),
                }));
            } else {
                cfg.display();
            }
        }
        ConfigCommands::Set { key, value } => {
            let mut cfg = CliConfig::load(None, None)?;
            match key.as_str() {
                "rpc_url" | "rpc" => {
                    cfg.rpc_url = value.clone();
                    println!("✅ rpc_url set to: {}", value);
                }
                "ws_url" | "ws" => {
                    cfg.ws_url = Some(value.clone());
                    println!("✅ ws_url set to: {}", value);
                }
                "keypair" | "key" => {
                    cfg.keypair = Some(PathBuf::from(&value));
                    println!("✅ default keypair set to: {}", value);
                }
                _ => {
                    anyhow::bail!(
                        "Unknown config key '{}'. Valid: rpc_url, ws_url, keypair",
                        key
                    );
                }
            }
            cfg.save()?;
        }
        ConfigCommands::Reset => {
            let cfg = CliConfig::default();
            cfg.save()?;
            println!("✅ Configuration reset to defaults");
            cfg.display();
        }
    }

    Ok(())
}
