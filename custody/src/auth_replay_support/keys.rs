use super::*;

pub(super) fn bridge_auth_replay_lookup_key(action: &str, digest: &str) -> String {
    format!("1:{}:{}", action, digest)
}

pub(super) fn bridge_auth_replay_expiry_key(expires_at: u64, action: &str, digest: &str) -> String {
    format!("0:{:020}:{}:{}", expires_at, action, digest)
}

pub(super) fn delete_auth_replay_record_by_expiry(
    db: &DB,
    action: &str,
    digest: &str,
    expires_at: u64,
) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_BRIDGE_AUTH_REPLAY)
        .ok_or_else(|| "missing bridge_auth_replay cf".to_string())?;
    db.delete_cf(cf, bridge_auth_replay_lookup_key(action, digest).as_bytes())
        .map_err(|error| format!("db delete: {}", error))?;
    db.delete_cf(
        cf,
        bridge_auth_replay_expiry_key(expires_at, action, digest).as_bytes(),
    )
    .map_err(|error| format!("db delete: {}", error))
}

pub(super) fn delete_bridge_auth_replay_record(
    db: &DB,
    action: &str,
    digest: &str,
    replay: &BridgeAuthReplayRecord,
) -> Result<(), String> {
    delete_auth_replay_record_by_expiry(db, action, digest, replay.expires_at)
}
