//! Issue-15: per-turn cache receipts + session cache-status aggregation.
//!
//! `TokenUsage::receipt_fragment` carries `(H cached / M new)`; this module
//! formats the one-line turn receipt gated by `ui.show_turn_usage` (display
//! only — accounting always runs) and aggregates `MessageCompleted` event
//! payloads into a session-level `CacheStatus` for `GET /cache-status`.
//!
//! # Language
//!
//! A receipt is read by a person, so it is rendered through the message
//! catalog for the active locale. It is composed from parts rather than
//! assembled with `format!` so word order and unit placement stay under the
//! translator's control — CJK and Russian put the label after the number.
//!
//! Two consequences worth knowing:
//!
//! * `TokenUsage::receipt_fragment` stays English because it is part of the
//!   stored event payload. Localized receipts are built here from the numeric
//!   fields, which are the real source of truth.
//! * The *labels* are passed in, not looked up here, so a caller that must
//!   render a specific locale (the CLI, an HTTP client) can. `receipt_from_*`
//!   uses the process locale, which is what every current caller wants.

use deepseekcode_llm::TokenUsage;
use serde::{Deserialize, Serialize};

/// `{count} cached` / `{count} new` / `{count} write` in the active locale.
fn cached_label(count: u64) -> String {
    deepseekcode_i18n::tr!("cache.cached", "count" => count)
}

fn new_label(count: u64) -> String {
    deepseekcode_i18n::tr!("cache.new", "count" => count)
}

fn write_label(count: u64) -> String {
    deepseekcode_i18n::tr!("cache.write", "count" => count)
}

/// The `(H cached / M new)` fragment, optionally with the premium cache-write
/// count. Localized rather than `TokenUsage::receipt_fragment`'s English, so
/// the numbers and the words agree.
fn fragment(hit: u32, miss: u32, write: Option<u32>) -> String {
    let cached = cached_label(hit as u64);
    let new = new_label(miss as u64);
    match write {
        Some(write) if write > 0 => deepseekcode_i18n::tr!(
            "cache.fragment_write",
            "cached" => cached.as_str(),
            "new" => new.as_str(),
            "write" => write_label(write as u64).as_str()
        ),
        _ => {
            deepseekcode_i18n::tr!("cache.fragment", "cached" => cached.as_str(), "new" => new.as_str())
        }
    }
}

/// `  · 12,300 tok · in 12,000 (10,800 cached / 1,200 new) · out 100`
fn turn_receipt(
    input: u32,
    output: u32,
    hit: u32,
    miss: u32,
    write: Option<u32>,
    reasoning: Option<u32>,
) -> String {
    let tokens = deepseekcode_i18n::tr!(
        "cache.tokens",
        "count" => (input as u64) + (output as u64)
    );
    let reasoning_suffix = reasoning
        .filter(|r| *r > 0)
        .map(|r| deepseekcode_i18n::tr!("cache.reasoning_suffix", "count" => r as u64));
    deepseekcode_i18n::tr!(
        "cache.turn_receipt",
        "tokens" => tokens.as_str(),
        "in" => deepseekcode_i18n::tr!("cache.in").as_str(),
        "input" => deepseekcode_i18n::count(input as u64).as_str(),
        "fragment" => fragment(hit, miss, write).as_str(),
        "out" => deepseekcode_i18n::tr!("cache.out").as_str(),
        "output" => deepseekcode_i18n::count(output as u64).as_str(),
        "reasoning" => reasoning_suffix.unwrap_or_default().as_str()
    )
}

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
        let rate = deepseekcode_i18n::tr!(
            "cache.rate",
            "rate" => deepseekcode_i18n::percent(self.hit_rate).as_str()
        );
        let base = deepseekcode_i18n::tr!(
            "cache.session_receipt",
            "in" => deepseekcode_i18n::tr!("cache.in").as_str(),
            "input" => deepseekcode_i18n::count(self.input_tokens).as_str(),
            "fragment" => fragment(
                self.cache_hit.min(u32::MAX as u64) as u32,
                self.cache_miss.min(u32::MAX as u64) as u32,
                Some(self.cache_write.min(u32::MAX as u64) as u32),
            )
            .as_str(),
            "out" => deepseekcode_i18n::tr!("cache.out").as_str(),
            "output" => deepseekcode_i18n::count(self.output_tokens).as_str(),
            "rate" => rate.as_str(),
            "over" => deepseekcode_i18n::tr!("cache.over_turns", "count" => self.turns).as_str()
        );
        if self.summary_count > 0 {
            deepseekcode_i18n::tr!(
                "cache.compact_suffix",
                "base" => base.as_str(),
                "count" => self.summary_count
            )
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
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        turns = status.turns,
        input_tokens = status.input_tokens,
        "estimated_cost computed"
    );
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
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        show = show,
        "receipt_from_payload requested"
    );
    if !show {
        return String::new();
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
    turn_receipt(
        usage.input_tokens.unwrap_or(0),
        usage.output_tokens.unwrap_or(0),
        usage.cache_hit.unwrap_or(0),
        usage.cache_miss.unwrap_or(0),
        usage.cache_write,
        usage.reasoning_tokens,
    )
}

/// One-line turn receipt with a pre-computed cost suffix (`· ¥0.0123`).
/// `cost_display` is already formatted (e.g. via `pricing::format_cost`);
/// `None`/empty means no suffix so old callers stay byte-identical.
pub fn format_turn_receipt_with_cost(
    usage: &TokenUsage,
    show: bool,
    cost_display: Option<&str>,
) -> String {
    let base = format_turn_receipt(usage, show);
    match cost_display {
        Some(c) if show && !base.is_empty() && !c.is_empty() => format!("{base} · {c}"),
        _ => base,
    }
}

/// Single-turn cost from a `MessageCompleted` payload + resolved card.
/// Mirrors `estimated_cost` for one turn so CLI/desktop never diverge.
pub fn turn_cost_from_payload(payload: &serde_json::Value, card: &RateCard) -> f64 {
    let hit = payload.get("cache_hit").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
    let mut miss = payload
        .get("cache_miss")
        .and_then(|v| v.as_u64())
        .map(|v| v as f64)
        .unwrap_or_else(|| {
            let input = payload.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
            input - hit.min(input)
        });
    if hit + miss == 0.0 {
        miss = payload.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
    }
    let completion = payload.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
    (hit * card.cache_hit_per_1m + miss * card.input_per_1m + completion * card.output_per_1m)
        / 1_000_000.0
}

/// Per-model session slice for `GET /cache-status.models[]`.
/// Groups by payload `model` (fallback `"unknown"` for history written
/// before model stamping), prices each group with its official row when
/// known, else the global fallback card.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModelCost {
    pub model: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit: u64,
    pub cache_miss: u64,
    pub currency: String,
    pub estimated_cost: f64,
    pub rate_band: String,
}

pub fn aggregate_by_model(
    payloads: &[serde_json::Value],
    display_currency: &str,
    fallback: &RateCard,
) -> Vec<ModelCost> {
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Acc {
        turns: u64,
        input: u64,
        output: u64,
        hit: u64,
        miss: u64,
    }
    let mut groups: BTreeMap<String, Acc> = BTreeMap::new();
    for p in payloads {
        if p.get("interrupted").and_then(|v| v.as_bool()).unwrap_or(false)
            && p.get("input_tokens").and_then(|v| v.as_u64()).is_none()
        {
            continue;
        }
        let model = p
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("unknown")
            .to_string();
        let acc = groups.entry(model).or_default();
        acc.turns += 1;
        acc.input += p.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        acc.output += p.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
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
        acc.hit += hit;
        acc.miss += miss;
    }
    let mut out = Vec::new();
    for (model, acc) in groups {
        let (card, band) = if model == "unknown" {
            (fallback.clone(), String::new())
        } else {
            match crate::pricing::rate_for(&model, display_currency) {
                Some((c, b)) => (c, b.to_string()),
                None => (fallback.clone(), String::new()),
            }
        };
        let (mut h, mut m) = (acc.hit as f64, acc.miss as f64);
        if h + m == 0.0 {
            m = acc.input as f64;
        }
        let cost = (h * card.cache_hit_per_1m + m * card.input_per_1m + acc.output as f64 * card.output_per_1m)
            / 1_000_000.0;
        let _ = &mut h;
        out.push(ModelCost {
            model,
            turns: acc.turns,
            input_tokens: acc.input,
            output_tokens: acc.output,
            cache_hit: acc.hit,
            cache_miss: acc.miss,
            currency: card.currency.clone(),
            estimated_cost: cost,
            rate_band: band,
        });
    }
    out
}

/// One-line turn receipt rendered for an explicit locale.
///
/// The HTTP server uses this to answer a request in the client's
/// `Accept-Language` without mutating the process locale, which is shared by
/// every other in-flight request.
pub fn format_turn_receipt_in(locale: deepseekcode_i18n::Locale, usage: &TokenUsage) -> String {
    let previous = deepseekcode_i18n::locale();
    if deepseekcode_i18n::locale() != locale {
        deepseekcode_i18n::set_locale(locale);
    }
    let rendered = format_turn_receipt(usage, true);
    if deepseekcode_i18n::locale() != previous {
        deepseekcode_i18n::set_locale(previous);
    }
    rendered
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
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        payloads = payloads.len(),
        compacted = compacted.len(),
        "aggregate_with_compaction started"
    );
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
    tracing::info!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        turns = status.turns,
        hit_rate = status.hit_rate,
        "aggregate_with_compaction completed"
    );
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_i18n::Locale;
    use serde_json::json;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Receipts render in the process locale, so the locale-sensitive tests
    /// serialize and restore it.
    fn lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct Reset(Locale);

    impl Drop for Reset {
        fn drop(&mut self) {
            deepseekcode_i18n::set_locale(self.0);
        }
    }

    fn sample_usage() -> TokenUsage {
        TokenUsage {
            input_tokens: Some(12000),
            output_tokens: Some(100),
            cache_hit: Some(10800),
            cache_miss: Some(1200),
            ..Default::default()
        }
    }

    #[test]
    fn aggregates_hit_miss_and_formats_receipt() {
        let _guard = lock();
        let _reset = Reset(deepseekcode_i18n::locale());
        deepseekcode_i18n::set_locale(Locale::En);

        let payloads = vec![
            json!({"input_tokens": 12000u64, "output_tokens": 100u64, "cache_hit": 10800u64, "cache_miss": 1200u64, "cache_write": 300u64, "cache_receipt": " (10800 cached / 1200 new)"}),
            json!({"input_tokens": 13000u64, "output_tokens": 100u64, "cache_hit": 12000u64, "cache_miss": 1000u64}),
        ];
        let status = aggregate_message_completed(&payloads);
        assert_eq!(status.turns, 2);
        assert_eq!(status.cache_hit, 22800);
        assert_eq!(status.cache_write, 300);
        assert!((status.hit_rate - 22800.0 / 25000.0).abs() < 1e-9);

        // The session receipt is assembled from the aggregate fields, and its
        // numbers are grouped for the locale rather than printed raw.
        let session = status.receipt();
        assert!(session.contains("22,800"), "{session}");
        assert!(session.contains("2 turns"), "{session}");
        assert!(session.contains("91.2%"), "{session}");

        let usage = sample_usage();
        let line = format_turn_receipt(&usage, true);
        assert!(line.contains("12,100"), "{line}");
        assert!(line.contains("(10,800"), "{line}");
        assert!(line.contains("1,200 new)"), "{line}");
        assert_eq!(format_turn_receipt(&usage, false), "");

        // The display gate still returns an empty string for a suppressed turn.
        assert_eq!(
            receipt_from_payload(
                &json!({"input_tokens": 12000u64, "output_tokens": 100u64}),
                false
            ),
            ""
        );

        // A payload's English `cache_receipt` is deliberately ignored: it is a
        // stored string in the server's language, and reusing it would print
        // English to a Chinese user.
        let with_english_receipt = receipt_from_payload(
            &json!({
                "input_tokens": 12000u64,
                "output_tokens": 100u64,
                "cache_receipt": " (10800 cached / 1200 new)",
                "cache_hit": 10800u64,
                "cache_miss": 1200u64,
            }),
            true,
        );
        assert!(
            with_english_receipt.contains("10,800"),
            "{with_english_receipt}"
        );

        let rebuilt = receipt_from_payload(
            &json!({"input_tokens": 12000u64, "output_tokens": 100u64, "cache_hit": 10800u64, "cache_miss": 1200u64}),
            true,
        );
        assert!(rebuilt.contains("10,800"), "{rebuilt}");

        // The premium cache-write count only appears when it is non-zero.
        let with_write = format_turn_receipt(
            &TokenUsage {
                cache_write: Some(300),
                ..sample_usage()
            },
            true,
        );
        assert!(with_write.contains("300 write"), "{with_write}");
        let without_write = format_turn_receipt(&sample_usage(), true);
        assert!(!without_write.contains("write"), "{without_write}");
    }

    #[test]
    fn reasoning_tokens_appear_only_when_present() {
        let _guard = lock();
        let _reset = Reset(deepseekcode_i18n::locale());
        deepseekcode_i18n::set_locale(Locale::En);
        let plain = format_turn_receipt(&sample_usage(), true);
        assert!(!plain.contains("reasoning"), "{plain}");
        let with_reasoning = format_turn_receipt(
            &TokenUsage {
                reasoning_tokens: Some(512),
                ..sample_usage()
            },
            true,
        );
        assert!(with_reasoning.contains("512 reasoning"), "{with_reasoning}");
    }

    #[test]
    fn receipts_render_in_every_shipped_locale() {
        let _guard = lock();
        let _reset = Reset(deepseekcode_i18n::locale());
        let usage = sample_usage();
        let sample = CacheStatus {
            turns: 2,
            input_tokens: 12_000,
            output_tokens: 100,
            cache_hit: 10_800,
            cache_miss: 1_200,
            cache_write: 300,
            hit_rate: 0.9,
            ..CacheStatus::default()
        };

        deepseekcode_i18n::set_locale(Locale::En);
        let english_turn = format_turn_receipt(&usage, true);
        let english_session = sample.receipt();

        let mut renderings: Vec<(Locale, String, String)> = Vec::new();
        for locale in Locale::ALL {
            if locale == Locale::En {
                renderings.push((locale, english_turn.clone(), english_session.clone()));
                continue;
            }
            deepseekcode_i18n::set_locale(locale);
            let receipt = format_turn_receipt(&usage, true);
            let aggregate = sample.receipt();
            for (label, text) in [("turn", &receipt), ("session", &aggregate)] {
                assert!(
                    !text.is_empty(),
                    "{locale:?} rendered an empty {label} receipt"
                );
                assert!(
                    !text.contains('{') && !text.contains('}'),
                    "{locale:?} {label} receipt leaked a placeholder: {text}"
                );
            }
            // The numbers are identical in every locale, so byte-equality with
            // the English rendering can only mean the catalog entry was missed.
            assert_ne!(
                receipt, english_turn,
                "{locale:?} rendered the English turn receipt verbatim"
            );
            assert_ne!(
                aggregate, english_session,
                "{locale:?} rendered the English session receipt verbatim"
            );
            renderings.push((locale, receipt, aggregate));
        }

        // No two locales may share a rendering: identical output for two
        // locales means one of them never went through the catalog.
        for (label, pick) in [("turn", 1usize), ("session", 2usize)] {
            let unique: std::collections::BTreeSet<&String> = renderings
                .iter()
                .map(|row| match pick {
                    1 => &row.1,
                    _ => &row.2,
                })
                .collect();
            assert_eq!(
                unique.len(),
                renderings.len(),
                "two locales rendered an identical {label} receipt: {:?}",
                renderings.iter().map(|r| r.0).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn compaction_suffix_is_localized() {
        let _guard = lock();
        let _reset = Reset(deepseekcode_i18n::locale());
        let payloads = vec![
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

        deepseekcode_i18n::set_locale(Locale::En);
        let english = status.receipt();
        assert!(english.contains("incl. 1 compact"), "{english}");
        deepseekcode_i18n::set_locale(Locale::Ru);
        let russian = status.receipt();
        assert!(!russian.contains("compact"), "{russian}");
        assert_ne!(english, russian);
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
}
