use super::bootstrap::{cors_origin_values, cors_origin_values_for_dev};
use super::models::AirdropQuery;
use super::rate_limit::RateLimiter;
use super::storage::{load_airdrops, save_airdrops};
use super::{
    models::{AirdropRecord, DEFAULT_COOLDOWN_SECONDS, DEFAULT_DAILY_LIMIT_PER_IP},
    now_ms, select_airdrop_records,
};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_airdrops_file(name: &str) -> String {
    let mut path = std::env::temp_dir();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    path.push(format!(
        "lichen-faucet-{}-{}-{}.json",
        name,
        std::process::id(),
        unique
    ));
    path.to_string_lossy().into_owned()
}

#[test]
fn faucet_cors_allows_wallet_origin_for_history_fetches() {
    let origins: Vec<String> = cors_origin_values()
        .iter()
        .map(|origin| origin.to_str().expect("valid header value").to_string())
        .collect();

    assert!(
        origins
            .iter()
            .any(|origin| origin == "https://wallet.lichen.network"),
        "wallet origin must be allowed to read faucet history"
    );
    assert!(
        origins
            .iter()
            .any(|origin| origin == "https://lichen-network-wallet.pages.dev"),
        "wallet Pages origin must be allowed for deployment previews"
    );
}

#[test]
fn faucet_dev_cors_allows_local_wallet_origin() {
    let origins: Vec<String> = cors_origin_values_for_dev(true)
        .iter()
        .map(|origin| origin.to_str().expect("valid header value").to_string())
        .collect();

    assert!(
        origins
            .iter()
            .any(|origin| origin == "http://localhost:3009"),
        "local wallet dev origin must be allowed when DEV_CORS is enabled"
    );
    assert!(
        origins
            .iter()
            .any(|origin| origin == "http://127.0.0.1:3009"),
        "127.0.0.1 wallet dev origin must be allowed when DEV_CORS is enabled"
    );
}

#[test]
fn airdrop_history_filters_by_requested_wallet_address() {
    let records = vec![
        AirdropRecord {
            signature: Some("older-target".to_string()),
            recipient: "target-wallet".to_string(),
            amount_licn: 5,
            timestamp_ms: 100,
            ip: None,
        },
        AirdropRecord {
            signature: Some("other-wallet".to_string()),
            recipient: "other-wallet".to_string(),
            amount_licn: 10,
            timestamp_ms: 300,
            ip: None,
        },
        AirdropRecord {
            signature: Some("newer-target".to_string()),
            recipient: "target-wallet".to_string(),
            amount_licn: 10,
            timestamp_ms: 200,
            ip: None,
        },
    ];

    let selected = select_airdrop_records(
        &records,
        &AirdropQuery {
            address: Some("target-wallet".to_string()),
            limit: Some(10),
        },
    );

    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].signature.as_deref(), Some("newer-target"));
    assert_eq!(selected[1].signature.as_deref(), Some("older-target"));
    assert!(selected
        .iter()
        .all(|record| record.recipient == "target-wallet"));
}

#[test]
fn restore_from_airdrops_preserves_ip_daily_limit_on_restart() {
    let path = temp_airdrops_file("ip-limit");
    let now = now_ms();
    let records = vec![AirdropRecord {
        signature: Some("sig-ip".to_string()),
        recipient: "addr-1".to_string(),
        amount_licn: DEFAULT_DAILY_LIMIT_PER_IP,
        timestamp_ms: now.saturating_sub((DEFAULT_COOLDOWN_SECONDS + 5) * 1000),
        ip: Some("203.0.113.10".to_string()),
    }];

    save_airdrops(&path, &records).expect("persist faucet history");
    let restored_records = load_airdrops(&path);
    let mut limiter = RateLimiter::default();
    limiter.restore_from_airdrops(&restored_records);

    let err = limiter
        .reserve(
            "203.0.113.10",
            "addr-2",
            now,
            1,
            DEFAULT_DAILY_LIMIT_PER_IP,
            DEFAULT_COOLDOWN_SECONDS,
        )
        .expect_err("same IP should remain rate-limited after restart");
    assert_eq!(err, "Daily faucet limit reached for this IP");

    let _ = fs::remove_file(&path);
}

#[test]
fn restore_from_airdrops_preserves_address_limit_without_ip_history() {
    let path = temp_airdrops_file("address-limit");
    let now = now_ms();
    let records = vec![AirdropRecord {
        signature: Some("sig-address".to_string()),
        recipient: "addr-1".to_string(),
        amount_licn: DEFAULT_DAILY_LIMIT_PER_IP,
        timestamp_ms: now.saturating_sub((DEFAULT_COOLDOWN_SECONDS + 5) * 1000),
        ip: None,
    }];

    save_airdrops(&path, &records).expect("persist faucet history");
    let restored_records = load_airdrops(&path);
    let mut limiter = RateLimiter::default();
    limiter.restore_from_airdrops(&restored_records);

    let err = limiter
        .reserve(
            "198.51.100.8",
            "addr-1",
            now,
            1,
            DEFAULT_DAILY_LIMIT_PER_IP,
            DEFAULT_COOLDOWN_SECONDS,
        )
        .expect_err("same address should remain rate-limited after restart");
    assert_eq!(err, "Daily faucet limit reached for this address");

    let _ = fs::remove_file(&path);
}

#[test]
fn reserve_blocks_follow_up_request_until_committed_or_rolled_back() {
    let mut limiter = RateLimiter::default();
    let now = now_ms();

    let reservation = limiter
        .reserve(
            "203.0.113.15",
            "addr-1",
            now,
            10,
            DEFAULT_DAILY_LIMIT_PER_IP,
            DEFAULT_COOLDOWN_SECONDS,
        )
        .expect("first request should reserve quota");

    let err = limiter
        .reserve(
            "203.0.113.15",
            "addr-2",
            now,
            1,
            DEFAULT_DAILY_LIMIT_PER_IP,
            DEFAULT_COOLDOWN_SECONDS,
        )
        .expect_err("reservation should enforce cooldown before RPC completes");
    assert_eq!(err, "Rate limit: try again in 60 seconds");

    limiter.rollback(&reservation);

    limiter
        .reserve(
            "203.0.113.15",
            "addr-2",
            now,
            1,
            DEFAULT_DAILY_LIMIT_PER_IP,
            DEFAULT_COOLDOWN_SECONDS,
        )
        .expect("rolled-back reservation should release quota");
}

#[test]
fn reserve_rejects_request_that_would_exceed_daily_limit() {
    let mut limiter = RateLimiter::default();
    let now = now_ms();

    limiter
        .reserve("198.51.100.10", "addr-1", now, 149, 150, 0)
        .expect("initial usage should fit within limit");

    let err = limiter
        .reserve("198.51.100.10", "addr-2", now, 2, 150, 0)
        .expect_err("request should be rejected when reserved total exceeds daily limit");
    assert_eq!(err, "Daily faucet limit reached for this IP");
}
