use serde::Deserialize;

#[derive(Deserialize)]
pub struct BlockInfo {
    pub slot: u64,
    pub hash: String,
    pub parent_hash: String,
    pub state_root: String,
    pub validator: String,
    pub timestamp: u64,
    pub transaction_count: usize,
}

#[derive(Deserialize)]
pub struct BurnedInfo {
    pub spores: u64,
    pub licn: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_current_total_burned_shape() {
        let burned: BurnedInfo = serde_json::from_value(json!({
            "spores": 1_500_000_000_000u64,
            "licn": 1500.0
        }))
        .expect("current getTotalBurned shape parses");

        assert_eq!(burned.spores, 1_500_000_000_000);
        assert_eq!(burned.licn, 1500.0);
    }
}
