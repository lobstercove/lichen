use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WithdrawalVelocityTier {
    #[default]
    Standard,
    Elevated,
    Extraordinary,
}

impl WithdrawalVelocityTier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            WithdrawalVelocityTier::Standard => "standard",
            WithdrawalVelocityTier::Elevated => "elevated",
            WithdrawalVelocityTier::Extraordinary => "extraordinary",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WithdrawalVelocityPolicy {
    pub(crate) tx_caps: BTreeMap<String, u64>,
    pub(crate) daily_caps: BTreeMap<String, u64>,
    pub(crate) elevated_thresholds: BTreeMap<String, u64>,
    pub(crate) extraordinary_thresholds: BTreeMap<String, u64>,
    pub(crate) elevated_delay_secs: i64,
    pub(crate) extraordinary_delay_secs: i64,
    pub(crate) operator_confirmation_tokens: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WithdrawalVelocitySnapshot {
    pub(crate) tier: WithdrawalVelocityTier,
    pub(crate) daily_cap: u64,
    pub(crate) required_signer_threshold: usize,
    pub(crate) required_operator_confirmations: usize,
    pub(crate) delay_secs: i64,
}

const SPORES_PER_ASSET_UNIT: u64 = 1_000_000_000;

fn build_asset_policy_map(entries: [(&str, u64); 4]) -> BTreeMap<String, u64> {
    entries
        .into_iter()
        .map(|(asset, amount)| (asset.to_string(), amount))
        .collect()
}

pub(crate) fn default_withdrawal_tx_caps() -> BTreeMap<String, u64> {
    build_asset_policy_map([
        ("musd", 250_000 * SPORES_PER_ASSET_UNIT),
        ("wsol", 50_000 * SPORES_PER_ASSET_UNIT),
        ("weth", 5_000 * SPORES_PER_ASSET_UNIT),
        ("wbnb", 10_000 * SPORES_PER_ASSET_UNIT),
    ])
}

pub(crate) fn default_withdrawal_daily_caps() -> BTreeMap<String, u64> {
    build_asset_policy_map([
        ("musd", 1_000_000 * SPORES_PER_ASSET_UNIT),
        ("wsol", 250_000 * SPORES_PER_ASSET_UNIT),
        ("weth", 25_000 * SPORES_PER_ASSET_UNIT),
        ("wbnb", 50_000 * SPORES_PER_ASSET_UNIT),
    ])
}

pub(crate) fn default_withdrawal_elevated_thresholds() -> BTreeMap<String, u64> {
    build_asset_policy_map([
        ("musd", 100_000 * SPORES_PER_ASSET_UNIT),
        ("wsol", 10_000 * SPORES_PER_ASSET_UNIT),
        ("weth", 1_000 * SPORES_PER_ASSET_UNIT),
        ("wbnb", 2_500 * SPORES_PER_ASSET_UNIT),
    ])
}

pub(crate) fn default_withdrawal_extraordinary_thresholds() -> BTreeMap<String, u64> {
    build_asset_policy_map([
        ("musd", 200_000 * SPORES_PER_ASSET_UNIT),
        ("wsol", 25_000 * SPORES_PER_ASSET_UNIT),
        ("weth", 2_500 * SPORES_PER_ASSET_UNIT),
        ("wbnb", 5_000 * SPORES_PER_ASSET_UNIT),
    ])
}

fn parse_policy_u64(value: &Value, env_name: &str, asset: &str) -> u64 {
    if let Some(number) = value.as_u64() {
        return number;
    }

    if let Some(number) = value.as_str().and_then(|text| text.parse::<u64>().ok()) {
        return number;
    }

    panic!(
        "FATAL: {} must map asset '{}' to an integer spore amount",
        env_name, asset
    );
}

fn load_asset_policy_from_env(
    env_name: &str,
    defaults: &BTreeMap<String, u64>,
) -> BTreeMap<String, u64> {
    let Some(raw) = std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return defaults.clone();
    };

    let parsed: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("FATAL: {} must be valid JSON: {}", env_name, error));
    let object = parsed.as_object().unwrap_or_else(|| {
        panic!(
            "FATAL: {} must be a JSON object of asset -> spores",
            env_name
        )
    });

    let mut policy = defaults.clone();
    for (asset, value) in object {
        let asset_key = asset.to_ascii_lowercase();
        if !policy.contains_key(&asset_key) {
            panic!(
                "FATAL: {} contains unsupported withdrawal asset '{}'; expected musd, wsol, weth, or wbnb",
                env_name, asset
            );
        }
        policy.insert(
            asset_key.clone(),
            parse_policy_u64(value, env_name, &asset_key),
        );
    }

    policy
}

fn load_operator_confirmation_tokens() -> Vec<String> {
    std::env::var("CUSTODY_OPERATOR_CONFIRMATION_TOKENS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|entry| entry.trim().to_string())
                .filter(|entry| !entry.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn load_withdrawal_velocity_policy() -> WithdrawalVelocityPolicy {
    WithdrawalVelocityPolicy {
        tx_caps: load_asset_policy_from_env(
            "CUSTODY_WITHDRAWAL_TX_CAPS",
            &default_withdrawal_tx_caps(),
        ),
        daily_caps: load_asset_policy_from_env(
            "CUSTODY_WITHDRAWAL_DAILY_CAPS",
            &default_withdrawal_daily_caps(),
        ),
        elevated_thresholds: load_asset_policy_from_env(
            "CUSTODY_WITHDRAWAL_ELEVATED_THRESHOLDS",
            &default_withdrawal_elevated_thresholds(),
        ),
        extraordinary_thresholds: load_asset_policy_from_env(
            "CUSTODY_WITHDRAWAL_EXTRAORDINARY_THRESHOLDS",
            &default_withdrawal_extraordinary_thresholds(),
        ),
        elevated_delay_secs: std::env::var("CUSTODY_WITHDRAWAL_ELEVATED_DELAY_SECS")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(1_800),
        extraordinary_delay_secs: std::env::var("CUSTODY_WITHDRAWAL_EXTRAORDINARY_DELAY_SECS")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(14_400),
        operator_confirmation_tokens: load_operator_confirmation_tokens(),
    }
}

pub(super) fn withdrawal_policy_amount(policy: &BTreeMap<String, u64>, asset: &str) -> u64 {
    policy
        .get(&asset.to_ascii_lowercase())
        .copied()
        .unwrap_or_default()
}

pub(crate) fn velocity_delay_secs(
    policy: &WithdrawalVelocityPolicy,
    tier: WithdrawalVelocityTier,
) -> i64 {
    match tier {
        WithdrawalVelocityTier::Standard => 0,
        WithdrawalVelocityTier::Elevated => policy.elevated_delay_secs,
        WithdrawalVelocityTier::Extraordinary => policy.extraordinary_delay_secs,
    }
}

fn required_signer_threshold_for_tier(
    config: &CustodyConfig,
    tier: WithdrawalVelocityTier,
) -> usize {
    match tier {
        WithdrawalVelocityTier::Standard => config.signer_threshold,
        WithdrawalVelocityTier::Elevated | WithdrawalVelocityTier::Extraordinary => {
            if config.signer_endpoints.is_empty() {
                0
            } else {
                config.signer_endpoints.len().max(config.signer_threshold)
            }
        }
    }
}

pub(crate) fn build_withdrawal_velocity_snapshot(
    config: &CustodyConfig,
    asset: &str,
    amount: u64,
) -> Result<WithdrawalVelocitySnapshot, String> {
    let asset_key = asset.to_ascii_lowercase();
    let tx_cap = withdrawal_policy_amount(&config.withdrawal_velocity_policy.tx_caps, &asset_key);
    if tx_cap > 0 && amount > tx_cap {
        return Err(format!(
            "withdrawal amount {} exceeds the {} per-transaction cap {}",
            amount, asset, tx_cap
        ));
    }

    let extraordinary_threshold = withdrawal_policy_amount(
        &config.withdrawal_velocity_policy.extraordinary_thresholds,
        &asset_key,
    );
    let elevated_threshold = withdrawal_policy_amount(
        &config.withdrawal_velocity_policy.elevated_thresholds,
        &asset_key,
    );

    let tier = if extraordinary_threshold > 0 && amount >= extraordinary_threshold {
        WithdrawalVelocityTier::Extraordinary
    } else if elevated_threshold > 0 && amount >= elevated_threshold {
        WithdrawalVelocityTier::Elevated
    } else {
        WithdrawalVelocityTier::Standard
    };

    let required_operator_confirmations = if tier == WithdrawalVelocityTier::Extraordinary {
        if config
            .withdrawal_velocity_policy
            .operator_confirmation_tokens
            .is_empty()
        {
            return Err(
                "extraordinary withdrawals are disabled until CUSTODY_OPERATOR_CONFIRMATION_TOKENS is configured"
                    .to_string(),
            );
        }
        1
    } else {
        0
    };

    Ok(WithdrawalVelocitySnapshot {
        tier,
        daily_cap: withdrawal_policy_amount(
            &config.withdrawal_velocity_policy.daily_caps,
            &asset_key,
        ),
        required_signer_threshold: required_signer_threshold_for_tier(config, tier),
        required_operator_confirmations,
        delay_secs: velocity_delay_secs(&config.withdrawal_velocity_policy, tier),
    })
}
