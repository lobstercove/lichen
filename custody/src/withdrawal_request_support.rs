use super::*;

pub(super) fn default_preferred_stablecoin() -> String {
    "usdt".to_string()
}

mod completion;
mod preflight;

pub(super) use self::completion::{
    complete_withdrawal_request, enforce_withdrawal_rate_limits,
    resolve_withdrawal_preferred_stablecoin, validate_withdrawal_request_destination,
};
#[cfg(test)]
pub(super) use self::preflight::withdrawal_access_message;
#[allow(unused_imports)]
pub(super) use self::preflight::CreateWithdrawalPreflight;
pub(super) use self::preflight::{
    build_create_withdrawal_response, handle_withdrawal_auth_replay,
    prepare_create_withdrawal_request,
};
