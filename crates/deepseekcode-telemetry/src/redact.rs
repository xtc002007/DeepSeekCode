/// Redact known secret patterns from log message text.
pub fn redact(value: &str) -> String {
    redact_with_patterns(value, SECRET_PATTERNS)
}

/// Redact every line in a log file (for diagnostic bundle export).
pub fn redact_log_content(content: &str) -> String {
    content
        .lines()
        .map(redact)
        .collect::<Vec<_>>()
        .join("\n")
}

const SECRET_PATTERNS: &[&str] = &[
    "Bearer ",
    "bearer ",
    "sk-",
    "api_key",
    "api-key",
    "password",
    "authorization:",
    "x-subscription-token",
    "DEEPSEEKCODE_SERVER_PASSWORD",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "DEEPSEEK_API_KEY",
    "OPENROUTER_API_KEY",
    "BRAVE_SEARCH_API_KEY",
];

fn redact_with_patterns(value: &str, patterns: &[&str]) -> String {
    let mut out = value.to_string();
    for pattern in patterns {
        if pattern.eq_ignore_ascii_case("sk-") {
            if let Some(idx) = out.to_lowercase().find("sk-") {
                if let Some(rest) = out.get(idx..) {
                    let end = rest
                        .char_indices()
                        .skip(1)
                        .find(|(_, c)| !c.is_ascii_alphanumeric() && *c != '-' && *c != '_')
                        .map(|(i, _)| idx + i)
                        .unwrap_or(out.len());
                    out.replace_range(idx..end, "sk-[REDACTED]");
                }
            }
            continue;
        }
        if let Some(idx) = out.to_lowercase().find(&pattern.to_lowercase()) {
            let line = &out[idx..];
            if pattern.eq_ignore_ascii_case("bearer ") {
                let start = idx + pattern.len();
                if start <= out.len() {
                    out.replace_range(start.., "[REDACTED]");
                }
                continue;
            }
            if let Some(sep_rel) = line.find('=').or_else(|| line.find(':')) {
                let value_start = idx + sep_rel + 1;
                let trim_start = out[value_start..]
                    .char_indices()
                    .find(|(_, c)| !c.is_whitespace() && *c != '"')
                    .map(|(i, _)| value_start + i)
                    .unwrap_or(value_start);
                if trim_start <= out.len() {
                    out.replace_range(trim_start.., "[REDACTED]");
                }
            }
        }
    }
    if out.contains("Authorization:") || out.contains("authorization:") {
        out = out
            .split_whitespace()
            .map(|w| {
                if w.len() > 20 && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    "[REDACTED]"
                } else {
                    w
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    out
}

/// Summarize a user prompt for logs (hash + length + prefix).
pub fn summarize_prompt(prompt: &str, include_full: bool) -> String {
    if include_full {
        return redact(prompt);
    }
    let len = prompt.chars().count();
    let prefix: String = prompt.chars().take(80).collect();
    let hash = simple_hash(prompt);
    format!("len={len} hash={hash:08x} prefix={}", redact(&prefix))
}

fn simple_hash(s: &str) -> u32 {
    s.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_prompt_truncates() {
        let long = "a".repeat(200);
        let summary = summarize_prompt(&long, false);
        assert!(summary.contains("len=200"));
        assert!(!summary.contains(&"a".repeat(200)));
    }

    #[test]
    fn redact_sk_prefix() {
        let input = "key=sk-abcdefghijklmnopqrstuvwxyz123456";
        let out = redact(input);
        assert!(out.contains("sk-[REDACTED]"));
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyz123456"));
    }

    #[test]
    fn redact_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let out = redact(input);
        assert!(!out.contains("eyJhbGciOi"));
    }

    #[test]
    fn redact_env_var_assignment() {
        let input = "OPENAI_API_KEY=sk-abc123secretvalue";
        let out = redact(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("abc123secretvalue"));
    }

    #[test]
    fn redact_log_content_applies_per_line() {
        let input = "line1 OPENAI_API_KEY=secret\nline2 ok";
        let out = redact_log_content(input);
        assert!(out.contains("[REDACTED]"));
        assert!(out.contains("line2 ok"));
    }
}
