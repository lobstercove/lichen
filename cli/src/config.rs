// CLI configuration management

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct CliConfig {
    /// RPC endpoint URL
    pub rpc_url: String,
    
    /// WebSocket endpoint URL (optional)
    pub ws_url: Option<String>,
    
    /// Default keypair path (optional)
    pub keypair: Option<PathBuf>,
}

impl Default for CliConfig {
    fn default() -> Self {
        CliConfig {
            rpc_url: "http://localhost:8899".to_string(),
            ws_url: Some("ws://localhost:8900".to_string()),
            keypair: None,
        }
    }
}

impl CliConfig {
    /// Load configuration from file or create default
    pub fn load(config_path: Option<&PathBuf>, url_override: Option<&String>) -> Result<Self> {
        let mut config = if let Some(path) = config_path {
            Self::load_from_file(path)?
        } else {
            let default_path = Self::default_path();
            if default_path.exists() {
                Self::load_from_file(&default_path)?
            } else {
                Self::default()
            }
        };
        
        // Override RPC URL if provided
        if let Some(url) = url_override {
            config.rpc_url = url.clone();
        }
        
        Ok(config)
    }
    
    fn load_from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .context("Failed to read config file")?;
        
        serde_json::from_str(&content)
            .context("Failed to parse config file")
    }
    
    /// Get default config path (~/.moltchain/config.json)
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".moltchain")
            .join("config.json")
    }
    
    /// Save configuration
    pub fn save(&self) -> Result<()> {
        let path = Self::default_path();
        
        // Create directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json)?;
        
        Ok(())
    }
    
    /// Display current configuration
    pub fn display(&self) {
        println!("🔧 MoltChain CLI Configuration");
        println!("\n📡 RPC Endpoint:  {}", self.rpc_url);
        if let Some(ws) = &self.ws_url {
            println!("🔌 WebSocket:     {}", ws);
        }
        if let Some(kp) = &self.keypair {
            println!("🔑 Default Key:   {}", kp.display());
        }
        println!("\n📁 Config file:   {}", Self::default_path().display());
    }
}
