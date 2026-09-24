use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::policy::PolicyConfig;

/// How policy prompts are handled at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PolicyMode {
    /// Prompt the user for `prompt` rules (default).
    #[default]
    #[serde(
        alias = "interactive",
        alias = "normal",
        alias = "Normal",
        alias = "INTERACTIVE"
    )]
    Interactive,
    /// YOLO: auto-permit non-`block` rules without UI.
    #[serde(alias = "auto", alias = "yolo", alias = "YOLO", alias = "AUTO")]
    Auto,
}

impl PolicyMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "interactive" | "normal" => Some(Self::Interactive),
            "auto" | "yolo" => Some(Self::Auto),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeepSeekCodeConfig {
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
    #[serde(default)]
    pub permissions: PermissionsConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
    /// When `auto`, `prompt` rules are permitted without user confirmation.
    #[serde(default)]
    pub policy_mode: Option<PolicyMode>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub model: Option<ModelConfig>,
    #[serde(default)]
    pub mcp: Option<McpConfig>,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub compaction: Option<CompactionConfig>,
    #[serde(default)]
    pub tool_output: Option<ToolOutputConfig>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub autoupdate: Option<AutoupdateConfig>,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub snapshots: Option<SnapshotsConfig>,
    #[serde(default)]
    pub skills_index_url: Option<String>,
    #[serde(default)]
    pub websearch: Option<WebsearchConfig>,
    /// Issue-15: per-turn cache receipts. `show_turn_usage=true` (default)
    /// prints `in N (H cached / M new) · out …`; false still records the
    /// same fields in events/DB for `/cache-status` auditing.
    #[serde(default)]
    pub ui: UiConfig,
    /// P1-5: billing rate card. Read-only cost attribution for
    /// `/cache-status` (`estimated_cost`); never gates requests.
    /// Defaults mirror the DeepSeek official list prices per 1M tokens.
    #[serde(default)]
    pub billing: BillingConfig,
    /// P2-7: context epoch behavior. All fields default to the stable
    /// tail-preserving semantics; see `ContextConfig`.
    #[serde(default)]
    pub context: ContextConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextConfig {
    /// When `false` (default), edits to standing sources (`AGENTS.md`,
    /// configured instructions, `MEMORY.md`) ride the turn tail as
    /// `[System update]` messages and never rewrite the system prefix.
    /// When `true`, such edits replace the baseline instead (one expected
    /// prefix miss for a short tail). Long-lived sessions with frequently
    /// edited standing docs should opt in; everyone else keeps the default.
    #[serde(default)]
    pub standing_replacement_on_edit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingConfig {
    #[serde(default = "default_billing_currency")]
    pub currency: String,
    /// Cache-hit price per 1M prompt tokens (default 0.10).
    #[serde(default = "default_price_cache_hit")]
    pub cache_hit_per_1m: f64,
    /// Full-price input per 1M prompt tokens (default 3.0).
    #[serde(default = "default_price_input")]
    pub input_per_1m: f64,
    /// Output price per 1M completion tokens (default 9.0).
    #[serde(default = "default_price_output")]
    pub output_per_1m: f64,
}

impl Default for BillingConfig {
    fn default() -> Self {
        Self {
            currency: default_billing_currency(),
            cache_hit_per_1m: default_price_cache_hit(),
            input_per_1m: default_price_input(),
            output_per_1m: default_price_output(),
        }
    }
}

fn default_billing_currency() -> String {
    "CNY".to_string()
}

fn default_price_cache_hit() -> f64 {
    0.10
}

fn default_price_input() -> f64 {
    3.0
}

fn default_price_output() -> f64 {
    9.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_true_ui")]
    pub show_turn_usage: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_turn_usage: true,
        }
    }
}

fn default_true_ui() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebsearchConfig {
    /// Search provider: `duckduckgo` (default) or `brave` (requires BRAVE_SEARCH_API_KEY env).
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub tools: Option<Vec<String>>,
    pub max_turns: Option<u32>,
    pub steps: Option<u32>,
    #[serde(default)]
    pub policy: Option<PolicyConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    /// Legacy `ask` list (maps to `prompt`). Kept for `permissions.ask`
    /// configs; `allow/ask/deny` all deserialize via PolicyAction::parse.
    #[serde(default)]
    pub ask: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider: Option<String>,
    pub id: Option<String>,
    /// Thinking depth for DeepSeek (`disabled` default for agent loops;
    /// `enabled`/`high` enables thinking, e.g. for `*thinking*` models).
    /// Changing it starts a new prefix-cache lineage by design.
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
    /// Issue-7: when `true`, MCP tools are exposed directly (`mcp_srv_tool`)
    /// and every connect/disconnect rewrites the provider `tools` array
    /// (a full DeepSeek prefix miss). Default `false` keeps a single stable
    /// `use_capability` proxy so MCP churn never touches the prefix.
    #[serde(default)]
    pub direct_tools: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    #[serde(default = "default_compaction_auto")]
    pub auto: bool,
    #[serde(default = "default_compaction_buffer")]
    pub buffer: u32,
    #[serde(default)]
    pub keep: Option<CompactionKeepConfig>,
    /// Sole auto trigger for the DeepSeek prefix-cache path (default 0.80).
    /// When set, `compact_if_needed` uses `ratio * window` instead of the
    /// legacy `window - buffer` threshold. Old keys keep loading for compat.
    #[serde(default)]
    pub compact_ratio: Option<f64>,
    /// Recent-tail budget as a fraction of the window (default 0.16).
    #[serde(default)]
    pub recent_tail_ratio: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionKeepConfig {
    pub tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutputConfig {
    pub max_lines: Option<u32>,
    pub max_bytes: Option<u32>,
}

fn default_compaction_auto() -> bool {
    true
}

fn default_compaction_buffer() -> u32 {
    20_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoupdateConfig {
    Bool(bool),
    Notify(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

fn default_log_format() -> LogFormat {
    if cfg!(debug_assertions) {
        LogFormat::Pretty
    } else {
        LogFormat::Json
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLoggingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub include_tool_output: bool,
    #[serde(default)]
    pub include_prompts: bool,
}

impl Default for AuditLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            include_tool_output: false,
            include_prompts: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_console_dev")]
    pub console: bool,
    #[serde(default = "default_true")]
    pub file: bool,
    #[serde(default = "default_log_format")]
    pub format: LogFormat,
    pub dir: Option<String>,
    #[serde(default = "default_max_file_mb")]
    pub max_file_mb: u64,
    #[serde(default = "default_max_run_dirs")]
    pub max_run_dirs: usize,
    #[serde(default)]
    pub modules: HashMap<String, String>,
    #[serde(default)]
    pub audit: AuditLoggingConfig,
}

fn default_log_level() -> String {
    "info".into()
}

fn default_console_dev() -> bool {
    cfg!(debug_assertions)
}

fn default_true() -> bool {
    true
}

fn default_max_file_mb() -> u64 {
    50
}

fn default_max_run_dirs() -> usize {
    20
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            console: default_console_dev(),
            file: default_true(),
            format: default_log_format(),
            dir: None,
            max_file_mb: default_max_file_mb(),
            max_run_dirs: default_max_run_dirs(),
            modules: HashMap::new(),
            audit: AuditLoggingConfig::default(),
        }
    }
}
