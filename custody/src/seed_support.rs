pub(super) fn load_required_seed_secret(
    file_var: &str,
    env_var: &str,
    allow_insecure_default: bool,
) -> String {
    let seed = if let Ok(seed_path) = std::env::var(file_var) {
        let seed_path = seed_path.trim().to_string();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&seed_path) {
                let mode = meta.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    tracing::warn!(
                        "⚠️  {} '{}' has permissions {:o} — should be 0600 or stricter. Tightening now.",
                        file_var,
                        seed_path,
                        mode
                    );
                    if let Err(error) =
                        std::fs::set_permissions(&seed_path, std::fs::Permissions::from_mode(0o600))
                    {
                        tracing::warn!("failed to tighten permissions on {}: {}", seed_path, error);
                    }
                }
            }
        }
        match std::fs::read_to_string(&seed_path) {
            Ok(contents) => {
                let secret = contents.trim().to_string();
                if secret.is_empty() {
                    panic!("FATAL: {} '{}' is empty.", file_var, seed_path);
                }
                tracing::info!("Secret loaded from file ({})", file_var);
                Some(secret)
            }
            Err(error) => panic!("FATAL: Cannot read {} '{}': {}", file_var, seed_path, error),
        }
    } else {
        None
    };

    let seed = seed.or_else(|| match std::env::var(env_var) {
        Ok(secret) if !secret.is_empty() => {
            tracing::warn!(
                "⚠️  Secret loaded from env var {}. Prefer {} for production.",
                env_var,
                file_var
            );
            std::env::remove_var(env_var);
            Some(secret)
        }
        _ => None,
    });

    match seed {
        Some(secret) => {
            if secret.len() < 32 && !secret.starts_with("INSECURE_DEFAULT") {
                panic!(
                    "FATAL: Secret from {} is too short ({} chars, minimum 32). Use a high-entropy seed.",
                    env_var,
                    secret.len()
                );
            }
            secret
        }
        None => {
            if allow_insecure_default
                && std::env::var("CUSTODY_ALLOW_INSECURE_SEED").unwrap_or_default() == "1"
            {
                tracing::warn!("⚠️  No seed configured — using insecure default (dev mode)!");
                "INSECURE_DEFAULT_SEED_DO_NOT_USE_IN_PRODUCTION".to_string()
            } else {
                panic!(
                    "FATAL: No seed configured. Set {} (preferred) or {}, or set CUSTODY_ALLOW_INSECURE_SEED=1 for dev.",
                    file_var, env_var
                );
            }
        }
    }
}

pub(super) fn load_optional_seed_secret(file_var: &str, env_var: &str) -> Option<String> {
    if std::env::var_os(file_var).is_none() && std::env::var_os(env_var).is_none() {
        return None;
    }
    Some(load_required_seed_secret(file_var, env_var, false))
}
