/// LichenID trust tier calculation (matches contract implementation)
/// Tier 0: Newcomer (rep < 100)
/// Tier 1: Known (rep 100-499)
/// Tier 2: Trusted (rep 500-999)
/// Tier 3: Established (rep 1000-4999)
/// Tier 4: Veteran (rep 5000-9999)
/// Tier 5: Legendary (rep 10000+)
pub fn get_trust_tier(reputation: u64) -> u8 {
    if reputation >= 10_000 {
        5
    } else if reputation >= 5_000 {
        4
    } else if reputation >= 1_000 {
        3
    } else if reputation >= 500 {
        2
    } else if reputation >= 100 {
        1
    } else {
        0
    }
}
