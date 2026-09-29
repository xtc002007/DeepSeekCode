use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Issue-5: single source of truth for token estimation.
/// Uses `chars/3` (CJK-safe) everywhere so compaction thresholds and runner
/// receipts agree. `len/4` underestimated CJK and caused early/late compacts.
pub fn estimate_str(value: &str) -> u32 {
    if value.is_empty() {
        0
    } else {
        value.chars().count().div_ceil(3).max(1) as u32
    }
}

pub fn estimate_json(value: &Value) -> u32 {
    serde_json::to_string(value)
        .map(|s| estimate_str(&s))
        .unwrap_or(1)
}

/// Provider-measured calibration pair: raw request chars → billed prompt
/// tokens for one model. Learned only from provider-reported `prompt_tokens`
/// (P1-4); locally estimated values must never call `set_calibration_*`,
/// or the ratio would chase its own tail.
#[derive(Debug, Clone, Copy, Default)]
pub struct PromptCalibration {
    pub request_chars: u64,
    pub prompt_tokens: u64,
}

impl PromptCalibration {
    /// Trusted token-per-char ratio, or `None` when the sample is
    /// implausible (zero side, or outside `0.05..2`, i.e. a corrupt sample
    /// or a tokenizer change — fall back to `chars/3` instead of trusting).
    fn ratio(self) -> Option<f64> {
        if self.request_chars == 0 || self.prompt_tokens == 0 {
            return None;
        }
        let r = self.prompt_tokens as f64 / self.request_chars as f64;
        if 0.05 < r && r < 2.0 {
            Some(r)
        } else {
            None
        }
    }
}

fn calibration_store() -> &'static Mutex<HashMap<String, PromptCalibration>> {
    static STORE: OnceLock<Mutex<HashMap<String, PromptCalibration>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Record a provider-measured sample for `model`. Callers must pass the
/// billed `prompt_tokens` and the exact chars sent — never an estimate.
/// Keyed by model id: different tokenizers must not contaminate each other.
pub fn set_calibration_from_usage(model: &str, request_chars: u64, prompt_tokens: u32) {
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        model = %model,
        request_chars = request_chars,
        prompt_tokens = prompt_tokens,
        "token calibration recorded"
    );
    if request_chars == 0 || prompt_tokens == 0 {
        return;
    }
    if let Ok(mut map) = calibration_store().lock() {
        map.insert(
            model.to_string(),
            PromptCalibration {
                request_chars,
                prompt_tokens: prompt_tokens as u64,
            },
        );
    }
}

#[cfg(test)]
pub(crate) fn reset_calibration_for_tests() {
    if let Ok(mut map) = calibration_store().lock() {
        map.clear();
    }
}

/// Raw request chars (no token math) over the exact pieces the estimator
/// covers. `set_calibration_from_usage` and `estimate_llm_request` both go
/// through here, so a learned ratio always applies to the same measure.
pub fn request_chars(
    system: &str,
    messages: &[deepseekcode_llm::LlmMessage],
    tools_json: &Value,
) -> u64 {
    let mut chars = system.chars().count() as u64;
    for m in messages {
        chars += m.content.chars().count() as u64;
        chars += m
            .reasoning_content
            .as_deref()
            .map(|s| s.chars().count())
            .unwrap_or(0) as u64;
        chars += serde_json::to_string(&m.tool_calls)
            .map(|s| s.chars().count())
            .unwrap_or(0) as u64;
    }
    chars += serde_json::to_string(tools_json)
        .map(|s| s.chars().count())
        .unwrap_or(0) as u64;
    chars
}

/// Full DeepSeek request estimate including system, messages (content +
/// reasoning + tool_calls), and tools. Replaces ad-hoc sums. When a trusted
/// calibration exists for `model`, the raw char count is scaled by the
/// learned ratio; otherwise (or when `model` is `None`) it falls back to the
/// `chars/3` heuristic piece by piece, byte-identical to the old behavior.
pub fn estimate_llm_request(
    system: &str,
    messages: &[deepseekcode_llm::LlmMessage],
    tools_json: &Value,
    model: Option<&str>,
) -> u32 {
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        model = ?model,
        messages = messages.len(),
        "estimate_llm_request started"
    );
    if let Some(m) = model {
        let ratio = calibration_store()
            .lock()
            .ok()
            .and_then(|map| map.get(m).copied())
            .and_then(|cal| cal.ratio());
        if let Some(r) = ratio {
            let chars = request_chars(system, messages, tools_json);
            if chars == 0 {
                return 0;
            }
            return ((chars as f64 * r).ceil() as u64)
                .min(u32::MAX as u64)
                .max(1) as u32;
        }
    }
    estimate_str(system)
        + messages
            .iter()
            .map(|m| {
                estimate_str(&m.content)
                    + m.reasoning_content
                        .as_deref()
                        .map(estimate_str)
                        .unwrap_or(0)
                    + estimate_json(&serde_json::json!(m.tool_calls))
            })
            .sum::<u32>()
        + estimate_json(tools_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_counts_by_chars_not_bytes() {
        // 10 CJK chars = 30 bytes; len/4 would say 7, chars/3 says 4.
        assert_eq!(estimate_str("你好世界测试一位两地"), 4);
        assert_eq!(estimate_str(""), 0);
    }

    #[test]
    fn calibration_scales_within_trusted_ratio() {
        reset_calibration_for_tests();
        let msgs: Vec<deepseekcode_llm::LlmMessage> = Vec::new();
        let tools = serde_json::json!([]);
        let text = "x".repeat(300);
        // Ratio 2.0 is OUTSIDE the trusted (0.05, 2) band: heuristic wins.
        set_calibration_from_usage("m", 3000, 6000);
        assert_eq!(
            estimate_llm_request(&text, &msgs, &tools, Some("m")),
            estimate_llm_request(&text, &msgs, &tools, None),
            "out-of-band ratio must fall back to chars/3"
        );
        // Ratio 0.5 is trusted: 302 measured chars (300 system + `[]`
        // tools) -> ceil(151).
        set_calibration_from_usage("m", 3000, 1500);
        assert_eq!(estimate_llm_request(&text, &msgs, &tools, Some("m")), 151);
        // Other models are unaffected (no cross-tokenizer contamination).
        assert_ne!(
            estimate_llm_request(&text, &msgs, &tools, Some("other")),
            151
        );
        reset_calibration_for_tests();
    }

    #[test]
    fn calibration_ignores_degenerate_samples() {
        reset_calibration_for_tests();
        set_calibration_from_usage("m", 0, 100);
        set_calibration_from_usage("m2", 100, 0);
        let msgs: Vec<deepseekcode_llm::LlmMessage> = Vec::new();
        let tools = serde_json::json!([]);
        assert_eq!(
            estimate_llm_request("hello", &msgs, &tools, Some("m")),
            estimate_llm_request("hello", &msgs, &tools, None)
        );
        reset_calibration_for_tests();
    }
}
