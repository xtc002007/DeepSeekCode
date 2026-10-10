//! Official per-model price book, mirrored from
//! `third-party-code/DeepSeek-Reasonix/internal/billing/catalog.go`.
//!
//! Reasonix keeps a dated dual-currency book (CNY/USD) with DeepSeek
//! peak/off-peak bands; DeepSeekCode previously had a single global
//! `BillingConfig` card (CNY `0.10/3/9`, i.e. the August Flash peak).
//! This module brings the September-2026 book in so every surface
//! (CLI receipt, desktop meter, `GET /cache-status`, `GET /billing/rates`)
//! prices the *current model*, not a global default.
//!
//! Units: per-1M-token prices, field order `cache_hit / input / output`.
//! Display currency never rewrites list prices; it only selects which
//! regional row is shown (Reasonix `BILLING.md` rule).

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::cache_status::RateCard;

/// One official list-price row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceEntry {
    pub provider: &'static str,
    pub model: &'static str,
    pub currency: &'static str,
    pub cache_hit: f64,
    pub input: f64,
    pub output: f64,
    /// `peak` / `off_peak` for DeepSeek scheduled rows, else `""`.
    pub band: &'static str,
    pub doc_url: &'static str,
}

/// UI/API row: one model with both bands + both currencies when available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceRow {
    pub provider: String,
    pub model: String,
    pub cny_peak: Option<[f64; 3]>,
    pub cny_off_peak: Option<[f64; 3]>,
    pub usd_peak: Option<[f64; 3]>,
    pub usd_off_peak: Option<[f64; 3]>,
    /// Static (non-banded) fallback, e.g. LongCat/MiMo/Claude.
    pub cny_static: Option<[f64; 3]>,
    pub usd_static: Option<[f64; 3]>,
    pub doc_url: String,
}

pub const BAND_PEAK: &str = "peak";
pub const BAND_OFF_PEAK: &str = "off_peak";

pub const DOC_DEEPSEEK: &str = "https://api-docs.deepseek.com/quick_start/pricing";
pub const DOC_LONGCAT_CNY: &str = "https://longcat.chat/platform/docs/zh/pricing/long-cat-2.0";
pub const DOC_LONGCAT_USD: &str = "https://longcat.chat/platform/docs/Pricing/LongCat-2.0.html";
pub const DOC_MIMO: &str = "https://mimo.mi.com/docs/price/pay-as-you-go";

/// Current (2026-09-10 12:00 Beijing生效) official book.
/// DeepSeek `deepseek-flash` 系三ID同价：`deepseek-flash`,
/// `deepseek-v4-flash`, `deepseek-v4-flash-vision-exp`（厂商把后两个路由到 V4.1 Flash）。
pub fn official_catalog() -> Vec<PriceEntry> {
    vec![
        // DeepSeek Flash系 — CNY
        PriceEntry { provider: "deepseek", model: "deepseek-flash", currency: "CNY", cache_hit: 0.04, input: 2.0, output: 8.0, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-flash", currency: "CNY", cache_hit: 0.02, input: 1.0, output: 4.0, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash", currency: "CNY", cache_hit: 0.04, input: 2.0, output: 8.0, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash", currency: "CNY", cache_hit: 0.02, input: 1.0, output: 4.0, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash-vision-exp", currency: "CNY", cache_hit: 0.04, input: 2.0, output: 8.0, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash-vision-exp", currency: "CNY", cache_hit: 0.02, input: 1.0, output: 4.0, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        // DeepSeek Flash系 — USD
        PriceEntry { provider: "deepseek", model: "deepseek-flash", currency: "USD", cache_hit: 0.006, input: 0.3, output: 1.2, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-flash", currency: "USD", cache_hit: 0.003, input: 0.15, output: 0.6, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash", currency: "USD", cache_hit: 0.006, input: 0.3, output: 1.2, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash", currency: "USD", cache_hit: 0.003, input: 0.15, output: 0.6, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash-vision-exp", currency: "USD", cache_hit: 0.006, input: 0.3, output: 1.2, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-flash-vision-exp", currency: "USD", cache_hit: 0.003, input: 0.15, output: 0.6, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        // DeepSeek Pro — CNY/USD × peak/off-peak
        PriceEntry { provider: "deepseek", model: "deepseek-v4-pro", currency: "CNY", cache_hit: 0.30, input: 9.0, output: 27.0, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-pro", currency: "CNY", cache_hit: 0.15, input: 4.5, output: 13.5, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-pro", currency: "USD", cache_hit: 0.044, input: 1.32, output: 3.96, band: BAND_PEAK, doc_url: DOC_DEEPSEEK },
        PriceEntry { provider: "deepseek", model: "deepseek-v4-pro", currency: "USD", cache_hit: 0.022, input: 0.66, output: 1.98, band: BAND_OFF_PEAK, doc_url: DOC_DEEPSEEK },
        // LongCat-2.0 — 静态价，无峰谷
        PriceEntry { provider: "longcat", model: "LongCat-2.0", currency: "CNY", cache_hit: 0.04, input: 2.0, output: 8.0, band: "", doc_url: DOC_LONGCAT_CNY },
        PriceEntry { provider: "longcat", model: "LongCat-2.0", currency: "USD", cache_hit: 0.006, input: 0.30, output: 1.20, band: "", doc_url: DOC_LONGCAT_USD },
        // MiMo — 静态价，仅CNY
        PriceEntry { provider: "mimo", model: "mimo-v2.5-pro", currency: "CNY", cache_hit: 0.025, input: 3.0, output: 6.0, band: "", doc_url: DOC_MIMO },
        PriceEntry { provider: "mimo", model: "mimo-v2-pro", currency: "CNY", cache_hit: 0.025, input: 3.0, output: 6.0, band: "", doc_url: DOC_MIMO },
        PriceEntry { provider: "mimo", model: "mimo-v2.5", currency: "CNY", cache_hit: 0.02, input: 1.0, output: 2.0, band: "", doc_url: DOC_MIMO },
        PriceEntry { provider: "mimo", model: "mimo-v2-omni", currency: "CNY", cache_hit: 0.02, input: 1.0, output: 2.0, band: "", doc_url: DOC_MIMO },
        PriceEntry { provider: "mimo", model: "mimo-v2-flash", currency: "CNY", cache_hit: 0.07, input: 0.70, output: 2.10, band: "", doc_url: DOC_MIMO },
        // Claude — 示例模板价，仅USD（Reasonix reasonix.example.toml:123）
        PriceEntry { provider: "claude", model: "claude-opus-4-8", currency: "USD", cache_hit: 0.5, input: 5.0, output: 25.0, band: "", doc_url: "" },
    ]
}

/// Beijing peak windows: 工作日 09:00–12:00, 14:00–18:00（左闭右开），其余+周末全off-peak。
/// 输入为UTC，与 Reasonix `DeepSeekRateBand` 一致。
pub fn is_peak_beijing(at: DateTime<Utc>) -> bool {
    // Beijing = UTC+8
    let beijing = at + chrono::Duration::hours(8);
    let weekday = beijing.weekday();
    if weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun {
        return false;
    }
    let minutes = beijing.hour() * 60 + beijing.minute();
    (minutes >= 9 * 60 && minutes < 12 * 60) || (minutes >= 14 * 60 && minutes < 18 * 60)
}

/// 归一化模型名：大小写不敏感，LongCat特殊处理。
fn norm_model(model: &str) -> String {
    model.trim().to_lowercase()
}

fn model_matches(entry_model: &str, query: &str) -> bool {
    let e = entry_model.to_lowercase();
    let q = query.trim().to_lowercase();
    if e == q {
        return true;
    }
    // DeepSeek Flash系三ID互通（V4.1 Flash统一定价）
    let flash_ids = ["deepseek-flash", "deepseek-v4-flash", "deepseek-v4-flash-vision-exp"];
    if flash_ids.contains(&e.as_str()) && flash_ids.contains(&q.as_str()) {
        return true;
    }
    false
}

fn norm_currency(currency: &str) -> &str {
    match currency.trim().to_uppercase().as_str() {
        "CNY" | "RMB" | "CNH" | "¥" | "￥" => "CNY",
        _ => "USD",
    }
}

/// 当前时刻该模型的费率卡 + 命中的band。未知模型返回None，调用方回退到全局BillingConfig。
pub fn rate_for(model: &str, currency: &str) -> Option<(RateCard, &'static str)> {
    rate_for_at(model, currency, Utc::now())
}

pub fn rate_for_at(
    model: &str,
    currency: &str,
    at: DateTime<Utc>,
) -> Option<(RateCard, &'static str)> {
    let cur = norm_currency(currency);
    let query = norm_model(model);
    if query.is_empty() {
        return None;
    }
    let catalog = official_catalog();
    // DeepSeek带band的按峰谷选行
    let band = if is_peak_beijing(at) { BAND_PEAK } else { BAND_OFF_PEAK };
    // 1) 精确 band 行
    for e in &catalog {
        if !e.band.is_empty() && e.band == band && e.currency == cur && model_matches(e.model, &query) {
            return Some((
                RateCard {
                    currency: cur.to_string(),
                    cache_hit_per_1m: e.cache_hit,
                    input_per_1m: e.input,
                    output_per_1m: e.output,
                },
                e.band,
            ));
        }
    }
    // 2) 静态行（无band）
    for e in &catalog {
        if e.band.is_empty() && e.currency == cur && model_matches(e.model, &query) {
            return Some((
                RateCard {
                    currency: cur.to_string(),
                    cache_hit_per_1m: e.cache_hit,
                    input_per_1m: e.input,
                    output_per_1m: e.output,
                },
                "",
            ));
        }
    }
    // 3) 跨币种兜底：MiMo只有CNY，Claude只有USD — 用原币价避免按全局默认误算
    for e in &catalog {
        if model_matches(e.model, &query) && (e.band.is_empty() || e.band == band) {
            return Some((
                RateCard {
                    currency: e.currency.to_string(),
                    cache_hit_per_1m: e.cache_hit,
                    input_per_1m: e.input,
                    output_per_1m: e.output,
                },
                e.band,
            ));
        }
    }
    None
}

/// 按模型的单轮费用，与 `cache_status::estimated_cost` 同公式。
pub fn cost_for_tokens(hit: u64, miss: u64, completion: u64, card: &RateCard) -> f64 {
    let (h, m) = (hit as f64, miss as f64);
    (h * card.cache_hit_per_1m + m * card.input_per_1m + completion as f64 * card.output_per_1m)
        / 1_000_000.0
}

/// `¥0.0123` / `$0.0018`：CNY保留4位小数对齐官方毫厘价，USD保留4位。
pub fn format_cost(currency: &str, amount: f64) -> String {
    let symbol = match norm_currency(currency) {
        "CNY" => "¥",
        _ => "$",
    };
    format!("{symbol}{amount:.4}")
}

/// 供 `/billing/rates` 与桌面价格表：按模型聚合双币种双band。
pub fn price_table() -> Vec<PriceRow> {
    let catalog = official_catalog();
    // 固定展示顺序：用户最常用的在前
    let order = [
        ("deepseek", "deepseek-flash"),
        ("deepseek", "deepseek-v4-pro"),
        ("deepseek", "deepseek-v4-flash-vision-exp"),
        ("longcat", "LongCat-2.0"),
        ("mimo", "mimo-v2.5-pro"),
        ("mimo", "mimo-v2.5"),
        ("mimo", "mimo-v2-flash"),
        ("claude", "claude-opus-4-8"),
    ];
    let mut rows = Vec::new();
    for (prov, model) in order {
        let get = |cur: &str, band: &str| -> Option<[f64; 3]> {
            catalog
                .iter()
                .find(|e| {
                    e.provider == prov
                        && e.model.eq_ignore_ascii_case(model)
                        && e.currency == cur
                        && e.band == band
                })
                .map(|e| [e.cache_hit, e.input, e.output])
        };
        let doc = catalog
            .iter()
            .find(|e| e.provider == prov && e.model.eq_ignore_ascii_case(model))
            .map(|e| e.doc_url.to_string())
            .unwrap_or_default();
        rows.push(PriceRow {
            provider: prov.to_string(),
            model: model.to_string(),
            cny_peak: get("CNY", BAND_PEAK),
            cny_off_peak: get("CNY", BAND_OFF_PEAK),
            usd_peak: get("USD", BAND_PEAK),
            usd_off_peak: get("USD", BAND_OFF_PEAK),
            cny_static: get("CNY", ""),
            usd_static: get("USD", ""),
            doc_url: doc,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn flash_trio_share_september_price() {
        // 2026-09-10后：Flash peak CNY 0.04/2/8
        let at = Utc.with_ymd_and_hms(2026, 9, 11, 2, 0, 0).unwrap(); // 北京时间10:00 peak
        for m in ["deepseek-flash", "deepseek-v4-flash", "deepseek-v4-flash-vision-exp"] {
            let (card, band) = rate_for_at(m, "CNY", at).expect("flash priced");
            assert_eq!(band, BAND_PEAK);
            assert!((card.input_per_1m - 2.0).abs() < 1e-12, "{m} {card:?}");
            assert!((card.output_per_1m - 8.0).abs() < 1e-12);
        }
    }

    #[test]
    fn weekend_is_off_peak_half_price() {
        // 周六北京时间 → off-peak半价 CNY 0.02/1/4
        let at = Utc.with_ymd_and_hms(2026, 9, 12, 2, 0, 0).unwrap(); // 周六
        let (card, band) = rate_for_at("deepseek-flash", "CNY", at).expect("priced");
        assert_eq!(band, BAND_OFF_PEAK);
        assert!((card.input_per_1m - 1.0).abs() < 1e-12);
    }

    #[test]
    fn static_models_resolve_and_cross_currency_fallback() {
        let (mimo, _) = rate_for_at("mimo-v2.5-pro", "CNY", Utc::now()).expect("mimo");
        assert!((mimo.input_per_1m - 3.0).abs() < 1e-12);
        // MiMo只有CNY：请求USD也回退到CNY原币价，不按全局默认误算
        let (fallback, _) = rate_for_at("mimo-v2.5", "USD", Utc::now()).expect("fallback");
        assert_eq!(fallback.currency, "CNY");
        let (claude, _) = rate_for_at("claude-opus-4-8", "CNY", Utc::now()).expect("claude");
        assert_eq!(claude.currency, "USD");
        assert!((claude.output_per_1m - 25.0).abs() < 1e-12);
    }

    #[test]
    fn unknown_model_returns_none_so_caller_falls_back_to_billing_config() {
        assert!(rate_for_at("some-future-model", "CNY", Utc::now()).is_none());
    }
}
