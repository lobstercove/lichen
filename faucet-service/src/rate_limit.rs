use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use super::models::AirdropRecord;

#[derive(Debug, Default)]
pub(super) struct RateLimiter {
    next_entry_id: u64,
    by_ip: HashMap<String, Vec<RateLimitEntry>>,
    // AUDIT-FIX M-24: Track per-recipient-address to prevent griefing a single address
    by_address: HashMap<String, Vec<RateLimitEntry>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RateLimitEntry {
    id: u64,
    timestamp_ms: u64,
    amount_licn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RateLimitReservation {
    id: u64,
    ip: String,
    address: String,
}

impl RateLimiter {
    fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(24 * 60 * 60 * 1000);
        self.by_ip.retain(|_, entries| {
            entries.retain(|entry| entry.timestamp_ms >= cutoff);
            !entries.is_empty()
        });
        self.by_address.retain(|_, entries| {
            entries.retain(|entry| entry.timestamp_ms >= cutoff);
            !entries.is_empty()
        });
    }

    fn next_id(&mut self) -> u64 {
        self.next_entry_id = self.next_entry_id.saturating_add(1).max(1);
        self.next_entry_id
    }

    pub(super) fn reserve(
        &mut self,
        ip: &str,
        address: &str,
        now_ms: u64,
        amount_licn: u64,
        daily_limit_licn: u64,
        cooldown_seconds: u64,
    ) -> Result<RateLimitReservation, String> {
        self.prune(now_ms);
        {
            let entries = self.by_ip.entry(ip.to_string()).or_default();
            if let Some(last_entry) = entries.last().copied() {
                let elapsed = now_ms.saturating_sub(last_entry.timestamp_ms) / 1000;
                if elapsed < cooldown_seconds {
                    let remaining = cooldown_seconds - elapsed;
                    return Err(format!("Rate limit: try again in {} seconds", remaining));
                }
            }

            let used_today: u64 = entries.iter().map(|entry| entry.amount_licn).sum();
            if used_today.saturating_add(amount_licn) > daily_limit_licn {
                return Err("Daily faucet limit reached for this IP".to_string());
            }
        }

        // AUDIT-FIX M-24: Also check per-address daily limit
        {
            let addr_entries = self.by_address.entry(address.to_string()).or_default();
            let addr_used: u64 = addr_entries.iter().map(|entry| entry.amount_licn).sum();
            if addr_used.saturating_add(amount_licn) > daily_limit_licn {
                return Err("Daily faucet limit reached for this address".to_string());
            }
        }

        let entry = RateLimitEntry {
            id: self.next_id(),
            timestamp_ms: now_ms,
            amount_licn,
        };
        self.by_ip.entry(ip.to_string()).or_default().push(entry);
        self.by_address
            .entry(address.to_string())
            .or_default()
            .push(entry);

        Ok(RateLimitReservation {
            id: entry.id,
            ip: ip.to_string(),
            address: address.to_string(),
        })
    }

    pub(super) fn rollback(&mut self, reservation: &RateLimitReservation) {
        if let Some(entries) = self.by_ip.get_mut(&reservation.ip) {
            entries.retain(|entry| entry.id != reservation.id);
        }
        if let Some(entries) = self.by_address.get_mut(&reservation.address) {
            entries.retain(|entry| entry.id != reservation.id);
        }
        self.by_ip.retain(|_, entries| !entries.is_empty());
        self.by_address.retain(|_, entries| !entries.is_empty());
    }

    /// AUDIT-FIX HIGH-05: Restore rate-limiter state from persisted airdrop history.
    pub(super) fn restore_from_airdrops(&mut self, records: &[AirdropRecord]) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let cutoff = now_ms.saturating_sub(24 * 60 * 60 * 1000);

        for record in records {
            if record.timestamp_ms < cutoff {
                continue;
            }
            let entry = RateLimitEntry {
                id: self.next_id(),
                timestamp_ms: record.timestamp_ms,
                amount_licn: record.amount_licn,
            };
            self.by_address
                .entry(record.recipient.clone())
                .or_default()
                .push(entry);
            if let Some(ref ip) = record.ip {
                self.by_ip.entry(ip.clone()).or_default().push(entry);
            }
        }
    }

    pub(super) fn tracked_address_count(&self) -> usize {
        self.by_address.len()
    }

    pub(super) fn tracked_ip_count(&self) -> usize {
        self.by_ip.len()
    }
}
