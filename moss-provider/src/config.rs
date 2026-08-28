use lichen_core::Pubkey;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub rpc_url: String,
    pub contract: Pubkey,
    pub keypair_path: PathBuf,
    pub public_base_url: String,
    pub allowed_origins: Vec<String>,
    pub max_object_bytes: u64,
    pub max_total_bytes: u64,
    pub owner_hourly_bytes: u64,
    pub staged_ttl: Duration,
    pub reconcile_interval: Duration,
    pub require_upload_signature: bool,
}

fn required(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn parse_u64(name: &str, default: u64) -> Result<u64, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| format!("{name} must be an unsigned integer")),
        Err(_) => Ok(default),
    }
}

fn parse_bool(name: &str, default: bool) -> Result<bool, String> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Ok(true),
            "0" | "false" | "no" => Ok(false),
            _ => Err(format!("{name} must be true or false")),
        },
        Err(_) => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let listen = std::env::var("MOSS_PROVIDER_LISTEN")
            .unwrap_or_else(|_| "127.0.0.1:9120".to_string())
            .parse::<SocketAddr>()
            .map_err(|_| "MOSS_PROVIDER_LISTEN must be host:port".to_string())?;
        let data_dir = PathBuf::from(required("MOSS_PROVIDER_DATA_DIR")?);
        let rpc_url = required("LICHEN_RPC_URL")?;
        let contract_text = required("MOSS_STORAGE_CONTRACT")?;
        let contract = Pubkey::from_base58(&contract_text)
            .map_err(|error| format!("MOSS_STORAGE_CONTRACT is invalid: {error}"))?;
        let keypair_path = PathBuf::from(required("MOSS_PROVIDER_KEYPAIR")?);
        let public_base_url = required("MOSS_PROVIDER_PUBLIC_BASE_URL")?
            .trim_end_matches('/')
            .to_string();
        if !public_base_url.starts_with("https://")
            && !public_base_url.starts_with("http://localhost")
            && !public_base_url.starts_with("http://127.0.0.1")
        {
            return Err(
                "MOSS_PROVIDER_PUBLIC_BASE_URL must use HTTPS outside localhost".to_string(),
            );
        }
        let allowed_origins = required("MOSS_PROVIDER_ALLOWED_ORIGINS")?
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if allowed_origins.is_empty() || allowed_origins.iter().any(|origin| origin == "*") {
            return Err("MOSS_PROVIDER_ALLOWED_ORIGINS must contain explicit origins".to_string());
        }
        let max_object_bytes = parse_u64("MOSS_PROVIDER_MAX_OBJECT_BYTES", 256 * 1024 * 1024)?;
        let max_total_bytes = parse_u64("MOSS_PROVIDER_MAX_TOTAL_BYTES", 50 * 1024 * 1024 * 1024)?;
        let owner_hourly_bytes = parse_u64("MOSS_PROVIDER_OWNER_HOURLY_BYTES", 512 * 1024 * 1024)?;
        if max_object_bytes == 0
            || max_total_bytes < max_object_bytes
            || owner_hourly_bytes < max_object_bytes
        {
            return Err("Moss byte limits are inconsistent or zero".to_string());
        }
        let staged_ttl_secs = parse_u64("MOSS_PROVIDER_STAGED_TTL_SECS", 3_600)?;
        let reconcile_secs = parse_u64("MOSS_PROVIDER_RECONCILE_SECS", 5)?;
        if staged_ttl_secs < 60 || reconcile_secs == 0 || reconcile_secs > 60 {
            return Err("Moss TTL/reconcile intervals are outside safe bounds".to_string());
        }
        let require_upload_signature = parse_bool("MOSS_PROVIDER_REQUIRE_SIGNATURE", true)?;
        if !listen.ip().is_loopback() && !require_upload_signature {
            return Err("public Moss listeners require signed uploads".to_string());
        }

        Ok(Self {
            listen,
            data_dir,
            rpc_url,
            contract,
            keypair_path,
            public_base_url,
            allowed_origins,
            max_object_bytes,
            max_total_bytes,
            owner_hourly_bytes,
            staged_ttl: Duration::from_secs(staged_ttl_secs),
            reconcile_interval: Duration::from_secs(reconcile_secs),
            require_upload_signature,
        })
    }

    pub fn is_loopback(&self) -> bool {
        self.listen.ip().is_loopback()
    }
}
