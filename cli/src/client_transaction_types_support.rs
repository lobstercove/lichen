use serde::Deserialize;

#[derive(Deserialize)]
pub struct TransactionInfo {
    pub signature: String,
    pub slot: u64,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub tx_type: String,
    pub amount: f64,
    pub amount_spores: u64,
    pub fee_spores: u64,
}

#[derive(Deserialize)]
pub struct TransactionHistoryResponse {
    pub transactions: Vec<TransactionInfo>,
    pub has_more: bool,
    pub next_before_slot: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_current_transaction_history_envelope() {
        let history: TransactionHistoryResponse = serde_json::from_value(json!({
            "transactions": [{
                "hash": "abc",
                "signature": "abc",
                "slot": 42,
                "timestamp": 123,
                "from": "from",
                "to": "to",
                "type": "Transfer",
                "amount": 1.25,
                "amount_spores": 1_250_000_000u64,
                "fee": 1_000_000u64,
                "fee_spores": 1_000_000u64,
                "fee_licn": 0.001,
                "success": true
            }],
            "has_more": true,
            "next_before_slot": 41
        }))
        .expect("current transaction history envelope parses");

        assert!(history.has_more);
        assert_eq!(history.next_before_slot, Some(41));
        assert_eq!(history.transactions[0].amount_spores, 1_250_000_000);
    }
}
