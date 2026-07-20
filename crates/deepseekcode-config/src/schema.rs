use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::policy::PolicyConfig;

/// How policy prompts are handled at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyMode {
    /// Prompt the user for `prompt` rules (default).
    #[default]
    Interactive,
    /// YOLO: auto-permit non-`block` rules without UI.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            deny: Vec::new(),
        }
    }
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
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
