use super::super::*;

/// AUDIT-FIX 1.20: Withdrawal rate limiting state
#[derive(Clone, Debug)]
pub(crate) struct WithdrawalRateState {
    /// (timestamp, count) for rolling window
    pub(crate) window_start: std::time::Instant,
    pub(crate) count_this_minute: u64,
    pub(crate) count_warning_level: Option<WithdrawalWarningLevel>,
    pub(crate) value_this_hour: u64,
    pub(crate) hour_start: std::time::Instant,
    pub(crate) value_warning_level: Option<WithdrawalWarningLevel>,
    /// Per-address: last withdrawal time
    pub(crate) per_address: std::collections::HashMap<String, std::time::Instant>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WithdrawalVelocityMetrics {
    pub(crate) count_this_minute: u64,
    pub(crate) max_withdrawals_per_min: u64,
    pub(crate) value_this_hour: u64,
    pub(crate) max_value_per_hour: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WithdrawalWarningLevel {
    HalfUsed,
    ThreeQuartersUsed,
    NearLimit,
}

impl WithdrawalWarningLevel {
    pub(crate) fn threshold_percent(self) -> u64 {
        match self {
            WithdrawalWarningLevel::HalfUsed => 50,
            WithdrawalWarningLevel::ThreeQuartersUsed => 75,
            WithdrawalWarningLevel::NearLimit => 90,
        }
    }

    pub(crate) fn severity(self) -> &'static str {
        match self {
            WithdrawalWarningLevel::HalfUsed => "warning",
            WithdrawalWarningLevel::ThreeQuartersUsed => "high",
            WithdrawalWarningLevel::NearLimit => "critical",
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            WithdrawalWarningLevel::HalfUsed => "fifty_percent",
            WithdrawalWarningLevel::ThreeQuartersUsed => "seventy_five_percent",
            WithdrawalWarningLevel::NearLimit => "ninety_percent",
        }
    }
}

impl WithdrawalRateState {
    pub(crate) fn new() -> Self {
        Self {
            window_start: std::time::Instant::now(),
            count_this_minute: 0,
            count_warning_level: None,
            value_this_hour: 0,
            hour_start: std::time::Instant::now(),
            value_warning_level: None,
            per_address: std::collections::HashMap::new(),
        }
    }

    pub(in crate::rate_limits) fn snapshot(
        &self,
        reference_now: std::time::Instant,
        reference_secs: u64,
    ) -> WithdrawalRateStateSnapshot {
        WithdrawalRateStateSnapshot {
            minute_window_start_secs: super::persistence::instant_to_unix_secs(
                self.window_start,
                reference_now,
                reference_secs,
            ),
            count_this_minute: self.count_this_minute,
            count_warning_level: self.count_warning_level,
            hour_window_start_secs: super::persistence::instant_to_unix_secs(
                self.hour_start,
                reference_now,
                reference_secs,
            ),
            value_this_hour: self.value_this_hour,
            value_warning_level: self.value_warning_level,
            per_address: self
                .per_address
                .iter()
                .map(|(address, instant)| {
                    (
                        address.clone(),
                        super::persistence::instant_to_unix_secs(
                            *instant,
                            reference_now,
                            reference_secs,
                        ),
                    )
                })
                .collect(),
        }
    }

    pub(in crate::rate_limits) fn from_snapshot(
        snapshot: WithdrawalRateStateSnapshot,
        reference_now: std::time::Instant,
        reference_secs: u64,
    ) -> Self {
        Self {
            window_start: super::persistence::unix_secs_to_instant(
                snapshot.minute_window_start_secs,
                reference_now,
                reference_secs,
            ),
            count_this_minute: snapshot.count_this_minute,
            count_warning_level: snapshot.count_warning_level,
            value_this_hour: snapshot.value_this_hour,
            hour_start: super::persistence::unix_secs_to_instant(
                snapshot.hour_window_start_secs,
                reference_now,
                reference_secs,
            ),
            value_warning_level: snapshot.value_warning_level,
            per_address: snapshot
                .per_address
                .into_iter()
                .map(|(address, secs)| {
                    (
                        address,
                        super::persistence::unix_secs_to_instant(
                            secs,
                            reference_now,
                            reference_secs,
                        ),
                    )
                })
                .collect(),
        }
    }
}

/// AUDIT-FIX W-H4: Deposit rate limiting state
#[derive(Clone, Debug)]
pub(crate) struct DepositRateState {
    pub(crate) window_start: std::time::Instant,
    pub(crate) count_this_minute: u64,
    /// Per-user: last deposit request time
    pub(crate) per_user: std::collections::HashMap<String, std::time::Instant>,
}

impl DepositRateState {
    pub(crate) fn new() -> Self {
        Self {
            window_start: std::time::Instant::now(),
            count_this_minute: 0,
            per_user: std::collections::HashMap::new(),
        }
    }

    pub(in crate::rate_limits) fn snapshot(
        &self,
        reference_now: std::time::Instant,
        reference_secs: u64,
    ) -> DepositRateStateSnapshot {
        DepositRateStateSnapshot {
            minute_window_start_secs: super::persistence::instant_to_unix_secs(
                self.window_start,
                reference_now,
                reference_secs,
            ),
            count_this_minute: self.count_this_minute,
            per_user: self
                .per_user
                .iter()
                .map(|(user, instant)| {
                    (
                        user.clone(),
                        super::persistence::instant_to_unix_secs(
                            *instant,
                            reference_now,
                            reference_secs,
                        ),
                    )
                })
                .collect(),
        }
    }

    pub(in crate::rate_limits) fn from_snapshot(
        snapshot: DepositRateStateSnapshot,
        reference_now: std::time::Instant,
        reference_secs: u64,
    ) -> Self {
        Self {
            window_start: super::persistence::unix_secs_to_instant(
                snapshot.minute_window_start_secs,
                reference_now,
                reference_secs,
            ),
            count_this_minute: snapshot.count_this_minute,
            per_user: snapshot
                .per_user
                .into_iter()
                .map(|(user, secs)| {
                    (
                        user,
                        super::persistence::unix_secs_to_instant(
                            secs,
                            reference_now,
                            reference_secs,
                        ),
                    )
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::rate_limits) struct WithdrawalRateStateSnapshot {
    pub(in crate::rate_limits) minute_window_start_secs: u64,
    pub(in crate::rate_limits) count_this_minute: u64,
    pub(in crate::rate_limits) count_warning_level: Option<WithdrawalWarningLevel>,
    pub(in crate::rate_limits) hour_window_start_secs: u64,
    pub(in crate::rate_limits) value_this_hour: u64,
    pub(in crate::rate_limits) value_warning_level: Option<WithdrawalWarningLevel>,
    pub(in crate::rate_limits) per_address: std::collections::HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::rate_limits) struct DepositRateStateSnapshot {
    pub(in crate::rate_limits) minute_window_start_secs: u64,
    pub(in crate::rate_limits) count_this_minute: u64,
    pub(in crate::rate_limits) per_user: std::collections::HashMap<String, u64>,
}
