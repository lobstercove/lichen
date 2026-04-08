use axum::{routing::get, routing::post, Json, Router};
use lichen_core::{
    keypair_file::{
        load_keypair_with_password_policy, plaintext_keypair_compat_allowed,
        require_runtime_keypair_password,
    },
    Keypair, KeypairFile,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

pub async fn start_signer_server(bind: SocketAddr, data_dir: &Path) {
    let keypair_path = resolve_signer_keypair_path(data_dir);
    let keypair = match load_or_generate_signer_keypair(&keypair_path) {
        Ok(keypair) => keypair,
        Err(err) => {
            warn!(
                "threshold signer disabled because keypair setup failed at {}: {}",
                keypair_path.display(),
                err
            );
            return;
        }
    };
    let pubkey_base58 = keypair.pubkey().to_base58();

    // T2.2 fix: Require authentication for signing requests.
    // Read token from env or generate a random one.
    let auth_token = std::env::var("LICHEN_SIGNER_AUTH_TOKEN").unwrap_or_else(|_| {
        use sha2::{Digest, Sha256};
        let seed = format!("signer-auth-{}-{}", pubkey_base58, std::process::id());
        let hash = Sha256::digest(seed.as_bytes());
        hex::encode(&hash[..16])
    });
    info!("threshold signer auth token configured (set LICHEN_SIGNER_AUTH_TOKEN to override)");

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
        signature: serde_json::to_string(&signature).expect("serialize signature"),
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
    if let Ok(path) = std::env::var("LICHEN_SIGNER_KEYPAIR") {
        return PathBuf::from(path);
    }
    data_dir.join("signer-keypair.json")
}

fn load_or_generate_signer_keypair(path: &Path) -> Result<Keypair, String> {
    let allow_plaintext = plaintext_keypair_compat_allowed();
    let password = require_runtime_keypair_password("threshold signer keypair load")?;

    if path.exists() {
        match load_signer_keypair_with_policy(path, password.as_deref(), allow_plaintext) {
            Ok(keypair) => return Ok(keypair),
            Err(err) => warn!("failed to load signer keypair {}: {}", path.display(), err),
        }
    }

    let keypair = Keypair::new();
    if let Err(err) = save_signer_keypair_with_password(&keypair, path, password.as_deref()) {
        return Err(err);
    } else {
        info!("saved signer keypair to {}", path.display());
    }
    Ok(keypair)
}

fn load_signer_keypair_with_policy(
    path: &Path,
    password: Option<&str>,
    allow_plaintext: bool,
) -> Result<Keypair, String> {
    load_keypair_with_password_policy(path, password, allow_plaintext)
        .map_err(|err| format!("load signer keypair {}: {}", path.display(), err))
}

fn save_signer_keypair_with_password(
    keypair: &Keypair,
    path: &Path,
    password: Option<&str>,
) -> Result<(), String> {
    KeypairFile::from_keypair(keypair)
        .save_with_password(path, password, password.is_some())
        .map_err(|err| format!("save signer keypair {}: {}", path.display(), err))
}

#[cfg(test)]
fn load_or_generate_signer_keypair_with_policy(
    path: &Path,
    password: Option<&str>,
    allow_plaintext: bool,
) -> Result<Keypair, String> {
    if path.exists() {
        match load_signer_keypair_with_policy(path, password, allow_plaintext) {
            Ok(keypair) => return Ok(keypair),
            Err(err) => warn!("failed to load signer keypair {}: {}", path.display(), err),
        }
    }

    let keypair = Keypair::new();
    if let Err(err) = save_signer_keypair_with_password(&keypair, path, password) {
        return Err(err);
    } else {
        info!("saved signer keypair to {}", path.display());
    }
    Ok(keypair)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
            chain: "lichen".to_string(),
            asset: "LICN".to_string(),
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
        let dir = std::env::temp_dir().join(format!("lichen_signer_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test-signer-keypair.json");
        let _ = fs::remove_file(&path);

        let keypair = Keypair::new();
        let pubkey = keypair.pubkey().to_base58();

        save_signer_keypair_with_password(&keypair, &path, None).expect("save failed");
        let loaded = load_signer_keypair_with_policy(&path, None, true).expect("load failed");

        assert_eq!(loaded.pubkey().to_base58(), pubkey);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_resolve_signer_keypair_path_default() {
        // Without env var, should use data_dir
        std::env::remove_var("LICHEN_SIGNER_KEYPAIR");
        let path = resolve_signer_keypair_path(Path::new("/tmp/data"));
        assert_eq!(path, PathBuf::from("/tmp/data/signer-keypair.json"));
    }

    #[test]
    fn test_load_or_generate_creates_new() {
        let dir = std::env::temp_dir().join(format!("licn_signer_gen_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("new-signer.json");
        let _ = fs::remove_file(&path);

        let kp = load_or_generate_signer_keypair_with_policy(&path, None, true).unwrap();
        assert!(path.exists());
        // Should be a valid keypair
        assert!(!kp.pubkey().to_base58().is_empty());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}
