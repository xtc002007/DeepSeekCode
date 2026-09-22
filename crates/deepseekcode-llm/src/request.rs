use deepseekcode_schema::tool::ToolDefinition;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: LlmRole,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<super::tool_call::LlmToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// DeepSeek `reasoning_content` — replayed verbatim on subsequent turns
    /// when tools are present so the prefix stays cache-friendly.
    /// `None` (and empty) is omitted on the wire; other providers ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl LlmMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: LlmRole::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_reasoning(content: String, reasoning: Option<String>) -> Self {
        Self {
            role: LlmRole::Assistant,
            content,
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning_content: reasoning.filter(|s| !s.is_empty()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub provider: String,
    pub model: String,
    pub system: String,
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<ToolDefinition>,
    pub base_url: Option<String>,
    /// Thinking depth (`high` default for DeepSeek). `None` = provider default.
    pub effort: Option<String>,
    /// `None` or `Some(0)` = omit on DeepSeek official endpoint (server 384K ceiling).
    pub max_output_tokens: Option<u32>,
}

/// Shared DeepSeek detector (Issue-5): provider flag OR DeepSeek URL.
/// Single source of truth so `compaction` and `LlmRequest` never disagree
/// (previously `session.provider_id`-only vs `base_url`-aware diverged and
/// custom `openai-compatible` + DeepSeek URL silently took the legacy path).
pub fn is_deepseek_request(provider: &str, base_url: Option<&str>) -> bool {
    provider == "deepseek" || base_url.is_some_and(|u| u.contains("deepseek.com"))
}

/// Shared thinking normalizer (Issues 2+4): one mapping for provider extras,
/// request lineage, and compaction lineage. `disabled` default keeps
/// agent/tool loops cheap and prefix-stable.
pub fn normalize_thinking(model: &str, effort: Option<&str>, is_deepseek: bool) -> &'static str {
    if !is_deepseek {
        return "n/a";
    }
    let effort = effort.unwrap_or("").trim().to_ascii_lowercase();
    if matches!(
        effort.as_str(),
        "enabled" | "high" | "max" | "xhigh" | "thinking" | "on"
    ) || (effort.is_empty() && model.contains("thinking"))
    {
        "enabled"
    } else {
        "disabled"
    }
}

impl LlmRequest {
    /// `true` when this request targets the DeepSeek automatic prefix cache.
    pub fn is_deepseek(&self) -> bool {
        is_deepseek_request(&self.provider, self.base_url.as_deref())
    }

    /// Normalized thinking flag for lineage (Issue-2). Delegates to the
    /// shared normalizer so provider extras and lineage never diverge.
    pub fn normalized_thinking(&self) -> &'static str {
        normalize_thinking(&self.model, self.effort.as_deref(), self.is_deepseek())
    }

    /// Cache lineage: changing any component intentionally starts a new
    /// DeepSeek prefix. Callers extend with `tools_hash`.
    pub fn lineage_key(&self, tools_hash: &str) -> String {
        lineage_key_for(
            &self.provider,
            &self.model,
            self.normalized_thinking(),
            tools_hash,
        )
    }
}

/// Shared lineage formatter (Issues 1+4): `provider|model|thinking|tools_hash`.
/// Compaction and request paths must produce byte-identical strings or
/// validation false-positives on every turn.
pub fn lineage_key_for(
    provider: &str,
    model: &str,
    normalized_thinking: &str,
    tools_hash: &str,
) -> String {
    format!("{provider}|{model}|{normalized_thinking}|{tools_hash}")
}

/// Stable hash of a provider-visible `tools_json` value for lineage.
/// The definition arrays are already name-sorted; serializing here keeps one
/// canonical byte form for both request and compaction paths.
pub fn tools_hash_for_json(tools_json: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let serialized = serde_json::to_string(tools_json).unwrap_or_else(|_| "[]".to_string());
    format!("{:x}", Sha256::digest(serialized.as_bytes()))
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub cache_tokens: Option<u32>,
    /// DeepSeek `prompt_cache_hit_tokens` (== `cached_tokens` on OpenAI shape).
    pub cache_hit: Option<u32>,
    /// DeepSeek `prompt_cache_miss_tokens`.
    pub cache_miss: Option<u32>,
    /// Subset of miss used to populate the prefix cache.
    pub cache_write: Option<u32>,
}

impl TokenUsage {
    /// Normalized cache-hit count (prefers explicit hit, falls back to legacy).
    pub fn hit_tokens(&self) -> u32 {
        self.cache_hit.or(self.cache_tokens).unwrap_or(0)
    }

    /// Normalized miss count; derives from `input - hit` when the provider
    /// only reports one side (DeepSeek streams sometimes omit miss).
    pub fn miss_tokens(&self) -> u32 {
        if let Some(m) = self.cache_miss {
            return m;
        }
        match (self.input_tokens, self.hit_tokens()) {
            (Some(input), hit) => input.saturating_sub(hit),
            _ => 0,
        }
    }

    /// `0.0..=1.0` prefix-cache hit rate for this aggregate.
    pub fn hit_rate(&self) -> f64 {
        let hit = self.hit_tokens() as f64;
        let total = hit + self.miss_tokens() as f64;
        if total <= 0.0 {
            0.0
        } else {
            hit / total
        }
    }

    /// One-line receipt fragment: `(10800 cached / 1200 new)`.
    /// `write` (subset of miss billed at a premium) is appended as
    /// `+W write` only when present, so the denominator stays
    /// `hit+miss` (Reasonix `Cost()` convention) while cost attribution
    /// stops going blind.
    pub fn receipt_fragment(&self) -> String {
        match self.cache_write {
            Some(w) if w > 0 => format!(
                " ({} cached / {} new, {} write)",
                self.hit_tokens(),
                self.miss_tokens(),
                w
            ),
            _ => format!(
                " ({} cached / {} new)",
                self.hit_tokens(),
                self.miss_tokens()
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<super::tool_call::LlmToolCall>,
    pub usage: Option<TokenUsage>,
    /// DeepSeek `reasoning_content` for non-streaming responses. Stream path
    /// accumulates `ReasoningDelta`; this keeps `generate` parity so the next
    /// turn can replay verbatim and keep the prefix warm.
    pub reasoning_content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_rate_prefers_explicit_hit_and_derives_miss() {
        let u = TokenUsage {
            input_tokens: Some(12_000),
            cache_hit: Some(10_800),
            cache_miss: None,
            ..Default::default()
        };
        assert_eq!(u.hit_tokens(), 10_800);
        assert_eq!(u.miss_tokens(), 1_200);
        assert!((u.hit_rate() - 0.9).abs() < 1e-9);
        assert_eq!(u.receipt_fragment(), " (10800 cached / 1200 new)");
    }

    #[test]
    fn legacy_cached_tokens_still_counts_as_hit() {
        let u = TokenUsage {
            input_tokens: Some(1_000),
            cache_tokens: Some(900),
            ..Default::default()
        };
        assert_eq!(u.hit_tokens(), 900);
        assert!((u.hit_rate() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn is_deepseek_matches_provider_or_base_url() {
        let base = LlmRequest {
            provider: "deepseek".into(),
            model: "deepseek-chat".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            base_url: None,
            effort: None,
            max_output_tokens: None,
        };
        assert!(base.is_deepseek());
        let via_url = LlmRequest {
            provider: "openai-compatible".into(),
            base_url: Some("https://api.deepseek.com/v1".into()),
            ..base.clone()
        };
        assert!(via_url.is_deepseek());
        let other = LlmRequest {
            provider: "openai".into(),
            base_url: None,
            ..base.clone()
        };
        assert!(!other.is_deepseek());
    }
}
