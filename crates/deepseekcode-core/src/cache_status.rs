//! Issue-15: per-turn cache receipts + session cache-status aggregation.
//!
//! `TokenUsage::receipt_fragment` carries `(H cached / M new)`; this module
//! formats the one-line turn receipt gated by `ui.show_turn_usage` (display
//! only — accounting always runs) and aggregates `MessageCompleted` event
//! payloads into a session-level `CacheStatus` for `GET /cache-status`.

use deepseekcode_llm::TokenUsage;
use serde::{Deserialize, Serialize};

/// Session-level cache accounting served by `GET /session/{id}/cache-status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStatus {
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_hit: u64,
    pub cache_miss: u64,
    /// Subset of miss used to populate the cache (premium-billed). Kept
    /// separate so `hit_rate` stays `hit/(hit+miss)` per Reasonix `Cost()`.
    pub cache_write: u64,
    pub hit_rate: f64,
    pub last_receipt: String,
    /// Compaction summary cost (from `SessionCompacted summary_*`). Folded
    /// into the totals so long sessions stop overestimating hit rate.
    pub summary_input_tokens: u64,
    pub summary_output_tokens: u64,
    pub summary_cache_hit: u64,
    pub summary_cache_miss: u64,
    pub summary_cache_write: u64,
    pub summary_count: u64,
    /// P2-6: first billed turn in isolation. The first request always pays
    /// a cold miss for a new prefix; folding it into the session average
    /// hides whether the session warmed up afterwards. `receipt()` keeps the
    /// aggregate shape so existing dashboards do not break.
    pub first_input_tokens: u64,
    pub first_cache_hit: u64,
    pub first_cache_miss: u64,
    pub first_hit_rate: f64,
}

impl CacheStatus {
    pub fn receipt(&self) -> String {
        let base = if self.cache_write > 0 {
            format!(
                "in {} ({} cached / {} new, {} write) · out {} · {:.1}% hit over {} turns",
                self.input_tokens,
                self.cache_hit,
                self.cache_miss,
                self.cache_write,
                self.output_tokens,
                self.hit_rate * 100.0,
                self.turns
            )
        } else {
            format!(
                "in {} ({} cached / {} new) · out {} · {:.1}% hit over {} turns",
                self.input_tokens,
                self.cache_hit,
                self.cache_miss,
                self.output_tokens,
                self.hit_rate * 100.0,
                self.turns
            )
        };
        if self.summary_count > 0 {
            format!("{base} · incl. {} compact", self.summary_count)
        } else {
            base
        }
    }
}

/// Billing rate card: per-1M-token prices in the configured currency.
/// Mirrors `BillingConfig` so cost math has exactly one shape; convert with
/// `RateCard::from` to keep the aggregation crate decoupled from config.
#[derive(Debug, Clone)]
pub struct RateCard {
    pub currency: String,
    pub cache_hit_per_1m: f64,
    pub input_per_1m: f64,
    pub output_per_1m: f64,
}

impl Default for RateCard {
    /// DeepSeek official list defaults per 1M tokens (CNY).
    fn default() -> Self {
        Self {
            currency: "CNY".to_string(),
            cache_hit_per_1m: 0.10,
            input_per_1m: 3.0,
            output_per_1m: 9.0,
        }
    }
}

impl From<&deepseekcode_config::schema::BillingConfig> for RateCard {
    fn from(cfg: &deepseekcode_config::schema::BillingConfig) -> Self {
        Self {
            currency: cfg.currency.clone(),
            cache_hit_per_1m: cfg.cache_hit_per_1m,
            input_per_1m: cfg.input_per_1m,
            output_per_1m: cfg.output_per_1m,
        }
    }
}

/// Estimated billed cost for an aggregated status, in `card.currency`.
/// Formula mirrors the reference convention:
/// `(hit*CacheHit + (miss-write+billedWrite)*Input + completion*Output)/1e6`,
/// with `billedWrite = min(write, miss)` and the `hit+miss==0 → miss=input`
/// fallback so uninstrumented sessions still price. `write` is premium cache
/// fill billed at the input price here; the fields stay separate so a future
/// discounted write schedule needs no aggregation change.
pub fn estimated_cost(status: &CacheStatus, card: &RateCard) -> f64 {
    let (hit, mut miss) = (status.cache_hit as f64, status.cache_miss as f64);
    if hit + miss == 0.0 {
        miss = status.input_tokens as f64;
    }
    // `write` (cache fill) is a subset of miss billed at the input price in
    // the current schedule, so the billable miss stays `miss`; the split is
    // kept explicit so a future discounted write schedule only touches this
    // function instead of the aggregation.
    let write = (status.cache_write as f64).min(miss);
    let fresh_miss = miss - write;
    let billed_write = write;
    let completion = status.output_tokens as f64 + status.summary_output_tokens as f64;
    (hit * card.cache_hit_per_1m
        + (fresh_miss + billed_write) * card.input_per_1m
        + completion * card.output_per_1m)
        / 1_000_000.0
}

/// One-line turn receipt straight from a `MessageCompleted` event payload.
/// Prefers the server-computed `cache_receipt` when present (identical bytes
/// to what `/cache-status` aggregates), otherwise rebuilds it from the
/// numeric fields. Returns `""` when `show` is false so `ui.show_turn_usage`
/// gates display only — accounting always runs.
pub fn receipt_from_payload(payload: &serde_json::Value, show: bool) -> String {
    if !show {
        return String::new();
    }
    if let Some(receipt) = payload.get("cache_receipt").and_then(|v| v.as_str()) {
        if !receipt.is_empty() {
            let input = payload
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = payload
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            return format!(
                "  · {} tok · in {input}{receipt} · out {output}",
                input + output
            );
        }
    }
    let usage = TokenUsage {
        input_tokens: payload
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        output_tokens: payload
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        reasoning_tokens: payload
            .get("reasoning_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        cache_tokens: payload
            .get("cache_hit")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        cache_hit: payload
            .get("cache_hit")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        cache_miss: payload
            .get("cache_miss")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        cache_write: payload
            .get("cache_write")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    };
    format_turn_receipt(&usage, true)
}

/// One-line turn receipt. Returns `""` when `show` is false so callers can
/// skip printing while still recording the underlying fields.
pub fn format_turn_receipt(usage: &TokenUsage, show: bool) -> String {
    if !show {
        return String::new();
    }
    let reasoning = usage
        .reasoning_tokens
        .map(|r| format!(" ({r} reasoning)"))
        .unwrap_or_default();
    format!(
        "  · {} tok · in {}{} · out {}{}",
        usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0),
        usage.input_tokens.unwrap_or(0),
        usage.receipt_fragment(),
        usage.output_tokens.unwrap_or(0),
        reasoning
    )
}

/// Aggregate `MessageCompleted` payloads (as stored in `EventStore`) into a
/// session status. Unknown/missing fields count as zero — never fail.
/// Interrupted turns (`interrupted:true` with no token fields) are skipped:
/// they have no billed tokens and would otherwise dilute `hit_rate`.
pub fn aggregate_message_completed(payloads: &[serde_json::Value]) -> CacheStatus {
    aggregate_with_compaction(payloads, &[])
}

/// Same as `aggregate_message_completed` plus `SessionCompacted` payloads.
/// Summary cost (`summary_input/output/hit/miss/write`) is folded into the
/// totals and flagged via `incl. N compact` so long sessions stop
/// overestimating hit rate by ignoring compaction spend.
pub fn aggregate_with_compaction(
    payloads: &[serde_json::Value],
    compacted: &[serde_json::Value],
) -> CacheStatus {
    let mut status = CacheStatus::default();
    for p in payloads {
        if p.get("interrupted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && p.get("input_tokens").and_then(|v| v.as_u64()).is_none()
        {
            continue;
        }
        // P2-6: snapshot the first billed turn before it dissolves into the
        // average, so cold-start cost stays distinguishable from regression.
        if status.turns == 0 {
            let first_hit = p.get("cache_hit").and_then(|v| v.as_u64()).unwrap_or(0);
            let first_miss = p
                .get("cache_miss")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    p.get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|i| i.saturating_sub(first_hit))
                })
                .unwrap_or(0);
            status.first_input_tokens = p.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            status.first_cache_hit = first_hit;
            status.first_cache_miss = first_miss;
            let first_total = first_hit + first_miss;
            status.first_hit_rate = if first_total == 0 {
                0.0
            } else {
                first_hit as f64 / first_total as f64
            };
        }
        status.turns += 1;
        status.input_tokens += p.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        status.output_tokens += p.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        status.reasoning_tokens += p
            .get("reasoning_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        // Prefer explicit hit/miss; derive miss from input-hit when needed.
        let hit = p.get("cache_hit").and_then(|v| v.as_u64()).unwrap_or(0);
        let miss = p
            .get("cache_miss")
            .and_then(|v| v.as_u64())
            .or_else(|| {
                p.get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|i| i.saturating_sub(hit))
            })
            .unwrap_or(0);
        status.cache_hit += hit;
        status.cache_miss += miss;
        status.cache_write += p.get("cache_write").and_then(|v| v.as_u64()).unwrap_or(0);
        if let Some(r) = p.get("cache_receipt").and_then(|v| v.as_str()) {
            status.last_receipt = r.to_string();
        }
    }
    for p in compacted {
        status.summary_count += 1;
        let si = p
            .get("summary_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let so = p
            .get("summary_output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let sh = p
            .get("summary_cache_hit")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let sm = p
            .get("summary_cache_miss")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let sw = p
            .get("summary_cache_write")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        status.summary_input_tokens += si;
        status.summary_output_tokens += so;
        status.summary_cache_hit += sh;
        status.summary_cache_miss += sm;
        status.summary_cache_write += sw;
        // Fold into billed totals so `hit_rate` reflects all spend.
        status.input_tokens += si;
        status.output_tokens += so;
        status.cache_hit += sh;
        status.cache_miss += sm;
        status.cache_write += sw;
    }
    let total = status.cache_hit + status.cache_miss;
    status.hit_rate = if total == 0 {
        0.0
    } else {
        status.cache_hit as f64 / total as f64
    };
    if status.last_receipt.is_empty() && status.turns > 0 {
        status.last_receipt = TokenUsage {
            input_tokens: Some(status.input_tokens.min(u32::MAX as u64) as u32),
            cache_hit: Some(status.cache_hit.min(u32::MAX as u64) as u32),
            cache_miss: Some(status.cache_miss.min(u32::MAX as u64) as u32),
            ..Default::default()
        }
        .receipt_fragment();
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aggregates_hit_miss_and_formats_receipt() {
        let payloads = vec![
            json!({"input_tokens": 12000u64, "output_tokens": 100u64, "cache_hit": 10800u64, "cache_miss": 1200u64, "cache_write": 300u64, "cache_receipt": " (10800 cached / 1200 new)"}),
            json!({"input_tokens": 13000u64, "output_tokens": 100u64, "cache_hit": 12000u64, "cache_miss": 1000u64}),
        ];
        let status = aggregate_message_completed(&payloads);
        assert_eq!(status.turns, 2);
        assert_eq!(status.cache_hit, 22800);
        assert_eq!(status.cache_write, 300);
        assert!((status.hit_rate - 22800.0 / 25000.0).abs() < 1e-9);
        assert!(status.receipt().contains("hit over 2 turns"));
        let usage = TokenUsage {
            input_tokens: Some(12000),
            output_tokens: Some(100),
            cache_hit: Some(10800),
            cache_miss: Some(1200),
            ..Default::default()
        };
        assert!(format_turn_receipt(&usage, true).contains("10800 cached"));
        assert_eq!(format_turn_receipt(&usage, false), "");
        // P1-4: payload-direct helper prefers the server receipt, honors the
        // display gate, and rebuilds when the receipt is absent.
        let line = receipt_from_payload(
            &json!({"input_tokens": 12000u64, "output_tokens": 100u64, "cache_receipt": " (10800 cached / 1200 new)"}),
            true,
        );
        assert!(line.contains("10800 cached") && line.contains("out 100"));
        assert_eq!(
            receipt_from_payload(
                &json!({"input_tokens": 12000u64, "output_tokens": 100u64, "cache_receipt": " (10800 cached / 1200 new)"}),
                false
            ),
            ""
        );
        let rebuilt = receipt_from_payload(
            &json!({"input_tokens": 12000u64, "output_tokens": 100u64, "cache_hit": 10800u64, "cache_miss": 1200u64}),
            true,
        );
        assert!(rebuilt.contains("10800 cached"));
        let usage_w = TokenUsage {
            cache_write: Some(300),
            ..usage.clone()
        };
        assert!(format_turn_receipt(&usage_w, true).contains("10800 cached"));
        assert!(usage_w.receipt_fragment().contains("write"));
    }

    #[test]
    fn estimated_cost_follows_hit_miss_write_formula() {
        // (hit*CacheHit + (miss-write+billedWrite)*Input + completion*Output)
        // / 1e6 with billedWrite = write at the input price. 1M-scale numbers
        // keep the arithmetic exact.
        let status = CacheStatus {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_hit: 800_000,
            cache_miss: 200_000,
            cache_write: 50_000,
            ..CacheStatus::default()
        };
        let card = RateCard {
            currency: "CNY".to_string(),
            cache_hit_per_1m: 0.10,
            input_per_1m: 3.0,
            output_per_1m: 9.0,
        };
        let cost = estimated_cost(&status, &card);
        let expected = (800_000.0 * 0.10 + 200_000.0 * 3.0 + 1_000_000.0 * 9.0) / 1_000_000.0;
        assert!(
            (cost - expected).abs() < 1e-9,
            "cost {cost} != formula {expected}"
        );
        // Uninstrumented sessions price the whole prompt at input price.
        let bare = CacheStatus {
            input_tokens: 1_000_000,
            ..Default::default()
        };
        assert!(
            (estimated_cost(&bare, &card) - 3.0).abs() < 1e-9,
            "hit+miss==0 must fall back to miss=input"
        );
        // Defaults mirror the documented official list prices.
        let default_card = RateCard::default();
        assert_eq!(default_card.currency, "CNY");
        assert!((default_card.cache_hit_per_1m - 0.10).abs() < 1e-12);
    }

    #[test]
    fn first_turn_stays_distinguishable_from_average() {
        // P2-6: cold-start miss must not hide inside the session average.
        let payloads = vec![
            json!({"input_tokens": 10000u64, "output_tokens": 50u64, "cache_hit": 0u64, "cache_miss": 10000u64}),
            json!({"input_tokens": 10000u64, "output_tokens": 50u64, "cache_hit": 9500u64, "cache_miss": 500u64}),
        ];
        let status = aggregate_message_completed(&payloads);
        assert_eq!(status.first_input_tokens, 10000);
        assert_eq!(status.first_cache_hit, 0);
        assert_eq!(status.first_cache_miss, 10000);
        assert!((status.first_hit_rate - 0.0).abs() < 1e-12);
        // Aggregate warmed up even though the first turn was cold.
        assert!(status.hit_rate > status.first_hit_rate);
        // Interrupted turns never become "first".
        let interrupted_first = vec![
            json!({"message_id": "a", "interrupted": true}),
            json!({"input_tokens": 8000u64, "output_tokens": 10u64, "cache_hit": 8000u64, "cache_miss": 0u64}),
        ];
        let skipped = aggregate_message_completed(&interrupted_first);
        assert_eq!(skipped.first_input_tokens, 8000);
        assert_eq!(skipped.turns, 1);
    }

    #[test]
    fn skips_interrupted_turns_and_folds_summary_cost() {
        let payloads = vec![
            json!({"message_id": "a", "interrupted": true}),
            json!({"input_tokens": 10000u64, "output_tokens": 50u64, "cache_hit": 9000u64, "cache_miss": 1000u64}),
        ];
        let compacted = vec![
            json!({"summary_input_tokens": 5000u64, "summary_output_tokens": 200u64, "summary_cache_hit": 4000u64, "summary_cache_miss": 1000u64, "summary_cache_write": 100u64}),
        ];
        let status = aggregate_with_compaction(&payloads, &compacted);
        assert_eq!(status.turns, 1, "interrupted turn must not dilute hit_rate");
        assert_eq!(status.summary_count, 1);
        assert_eq!(status.input_tokens, 15000);
        assert_eq!(status.cache_hit, 13000);
        assert!(status.receipt().contains("incl. 1 compact"));
    }
}
