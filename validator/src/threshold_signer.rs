use axum::{routing::get, routing::post, Json, Router};
use moltchain_core::Keypair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct SignRequest {
    pub job_id: String,
    pub chain: String,
    pub asset: String,
    pub from_address: String,
    pub to_address: String,
    #[serde(default)]
    pub amount: Option<String>,
    #[serde(default)]
    pub tx_hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SignResponse {
    pub status: String,
    pub signer_pubkey: String,
    pub signature: String,
    pub message_hash: String,
    pub message: String,
}

#[derive(Clone)]
struct SignerState {
    keypair: Arc<Keypair>,
    pubkey_base58: String,
    /// T2.2 fix: Auth token required for signing requests.
    /// Only validators with the correct token can request signatures.
    auth_token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SignerKeyFile {
    #[serde(rename = "privateKey")]
    private_key: Vec<u8>,
    #[serde(rename = "publicKey")]
    public_key: Vec<u8>,
    #[serde(rename = "publicKeyBase58")]
    public_key_base58: String,
}

pub async fn start_signer_server(bind: SocketAddr, data_dir: &Path) {
    let keypair_path = resolve_signer_keypair_path(data_dir);
    let keypair = load_or_generate_signer_keypair(&keypair_path);
    let pubkey_base58 = keypair.pubkey().to_base58();

    // T2.2 fix: Require authentication for signing requests.
    // Read token from env or generate a random one.
    let auth_token = std::env::var("MOLTCHAIN_SIGNER_AUTH_TOKEN").unwrap_or_else(|_| {
        use sha2::{Digest, Sha256};
        let seed = format!("signer-auth-{}-{}", pubkey_base58, std::process::id());
        let hash = Sha256::digest(seed.as_bytes());
        hex::encode(&hash[..16])
    });
    info!("threshold signer auth token configured (set MOLTCHAIN_SIGNER_AUTH_TOKEN to override)");

    let state = SignerState {
        keypair: Arc::new(keypair),
        pubkey_base58,
        auth_token,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/sign", post(sign_request))
        .with_state(state);

    info!("threshold signer listening on {}", bind);

    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                "threshold signer failed to bind {}: {} — signer disabled",
                bind,
                e
            );
            return;
        }
    };

    if let Err(err) = axum::serve(listener, app).await {
        tracing::error!("threshold signer error: {}", err);
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn sign_request(
    axum::extract::State(state): axum::extract::State<SignerState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SignRequest>,
) -> axum::response::Response {
    // T2.2 fix: Authenticate the caller before allowing signing
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|token| token == state.auth_token)
        .unwrap_or(false);

    if !authorized {
        warn!(
            "threshold signer: rejected unauthenticated sign request for job={}",
            req.job_id
        );
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(SignResponse {
                status: "unauthorized".to_string(),
                signer_pubkey: String::new(),
                signature: String::new(),
                message_hash: String::new(),
                message: "Missing or invalid Authorization: Bearer <token>".to_string(),
            }),
        ));
    }

    let payload = build_signing_payload(&req);
    let hash = Sha256::digest(payload.as_bytes());
    let signature = state.keypair.sign(hash.as_slice());

    axum::response::IntoResponse::into_response(axum::Json(SignResponse {
        status: "signed".to_string(),
        signer_pubkey: state.pubkey_base58.clone(),
        signature: hex::encode(signature),
        message_hash: hex::encode(hash),
        message: "threshold signer signature produced".to_string(),
    }))
}

fn build_signing_payload(req: &SignRequest) -> String {
    let amount = req.amount.as_deref().unwrap_or("unknown");
    let tx_hash = req.tx_hash.as_deref().unwrap_or("unknown");
    format!(
        "job_id={};chain={};asset={};from={};to={};amount={};tx_hash={}",
        req.job_id, req.chain, req.asset, req.from_address, req.to_address, amount, tx_hash
    )
}

fn resolve_signer_keypair_path(data_dir: &Path) -> PathBuf {
    if let Ok(path) = std::env::var("MOLTCHAIN_SIGNER_KEYPAIR") {
        return PathBuf::from(path);
    }
    data_dir.join("signer-keypair.json")
}

fn load_or_generate_signer_keypair(path: &Path) -> Keypair {
    if path.exists() {
        match load_signer_keypair(path) {
            Ok(keypair) => return keypair,
            Err(err) => warn!("failed to load signer keypair {}: {}", path.display(), err),
        }
    }

    let keypair = Keypair::new();
    if let Err(err) = save_signer_keypair(&keypair, path) {
        warn!("failed to save signer keypair {}: {}", path.display(), err);
    } else {
        info!("saved signer keypair to {}", path.display());
    }
    keypair
}

fn load_signer_keypair(path: &Path) -> Result<Keypair, String> {
    let json = fs::read_to_string(path).map_err(|e| format!("read: {}", e))?;
    let keypair_file: SignerKeyFile =
        serde_json::from_str(&json).map_err(|e| format!("parse: {}", e))?;
    if keypair_file.private_key.len() != 32 {
        return Err("invalid private key length".to_string());
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&keypair_file.private_key);
    Ok(Keypair::from_seed(&seed))
}

fn save_signer_keypair(keypair: &Keypair, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
    }

    let pubkey = keypair.pubkey();
    let seed = keypair.to_seed();
    let keypair_file = SignerKeyFile {
        private_key: seed.to_vec(),
        public_key: pubkey.0.to_vec(),
        public_key_base58: pubkey.to_base58(),
    };

    let json = serde_json::to_string_pretty(&keypair_file).map_err(|e| format!("encode: {}", e))?;
    fs::write(path, json).map_err(|e| format!("write: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, permissions).map_err(|e| format!("chmod: {}", e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_signing_payload_full() {
        let req = SignRequest {
            job_id: "job1".to_string(),
            chain: "solana".to_string(),
            asset: "SOL".to_string(),
            from_address: "AAA".to_string(),
            to_address: "BBB".to_string(),
            amount: Some("100".to_string()),
            tx_hash: Some("0xabc".to_string()),
        };
        let payload = build_signing_payload(&req);
        assert!(payload.contains("job_id=job1"));
        assert!(payload.contains("chain=solana"));
        assert!(payload.contains("asset=SOL"));
        assert!(payload.contains("from=AAA"));
        assert!(payload.contains("to=BBB"));
        assert!(payload.contains("amount=100"));
        assert!(payload.contains("tx_hash=0xabc"));
    }

    #[test]
    fn test_build_signing_payload_defaults() {
        let req = SignRequest {
            job_id: "job2".to_string(),
            chain: "moltchain".to_string(),
            asset: "MOLT".to_string(),
            from_address: "X".to_string(),
            to_address: "Y".to_string(),
            amount: None,
            tx_hash: None,
        };
        let payload = build_signing_payload(&req);
        assert!(payload.contains("amount=unknown"));
        assert!(payload.contains("tx_hash=unknown"));
    }

    #[test]
    fn test_signer_keypair_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("moltchain_signer_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test-signer-keypair.json");
        let _ = fs::remove_file(&path);

        let keypair = Keypair::new();
        let pubkey = keypair.pubkey().to_base58();

        save_signer_keypair(&keypair, &path).expect("save failed");
        let loaded = load_signer_keypair(&path).expect("load failed");

        assert_eq!(loaded.pubkey().to_base58(), pubkey);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_resolve_signer_keypair_path_default() {
        // Without env var, should use data_dir
        std::env::remove_var("MOLTCHAIN_SIGNER_KEYPAIR");
        let path = resolve_signer_keypair_path(Path::new("/tmp/data"));
        assert_eq!(path, PathBuf::from("/tmp/data/signer-keypair.json"));
    }

    #[test]
    fn test_load_or_generate_creates_new() {
        let dir = std::env::temp_dir().join(format!("molt_signer_gen_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("new-signer.json");
        let _ = fs::remove_file(&path);

        let kp = load_or_generate_signer_keypair(&path);
        assert!(path.exists());
        // Should be a valid keypair
        assert!(!kp.pubkey().to_base58().is_empty());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}
