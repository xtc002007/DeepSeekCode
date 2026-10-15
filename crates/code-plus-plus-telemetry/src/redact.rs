/// Redact known secret patterns from log message text.
pub fn redact(value: &str) -> String {
    redact_with_patterns(value, SECRET_PATTERNS)
}

/// Redact every line in a log file (for diagnostic bundle export).
pub fn redact_log_content(content: &str) -> String {
    content.lines().map(redact).collect::<Vec<_>>().join("\n")
}

const SECRET_PATTERNS: &[&str] = &[
    "Bearer ",
    "bearer ",
    "sk-",
    "sk-ant-",
    "ghp_",
    "xoxb-",
    "api_key",
    "api-key",
    "apikey",
    "client_secret",
    "refresh_token",
    "private_key",
    "secret",
    "token",
    "passwd",
    "password",
    "authorization:",
    "authorization",
    "x-api-key",
    "x-subscription-token",
    "x-code-plus-plus-ticket",
    "CODE_PLUS_PLUS_SERVER_PASSWORD",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "DEEPSEEK_API_KEY",
    "OPENROUTER_API_KEY",
    "BRAVE_SEARCH_API_KEY",
];

/// Structured redaction: replaces only the secret token span, preserving
/// JSON structure (P1-13: previously truncated the whole line tail after
/// `=`/`:` or after `Bearer `, breaking JSON).
///
/// Structured redaction with progress guarantee: the lowercase view is
/// rebuilt after every replacement (indices never go stale across mutations),
/// and searching always resumes AFTER the inserted `[REDACTED]` marker so a
/// replacement can never re-match itself (infinite loop).
fn redact_with_patterns(value: &str, patterns: &[&str]) -> String {
    // ASCII-only patterns: byte indices from the lowercased view are valid in
    // `out` as long as both are rebuilt together each iteration.
    debug_assert!(patterns.iter().all(|p| p.is_ascii()));
    const MARKER: &str = "[REDACTED]";
    let mut out = value.to_string();
    for pattern in patterns {
        let needle = pattern.to_lowercase();
        let mut pos = 0usize;
        loop {
            if pos > out.len() {
                break;
            }
            let lower = out.to_lowercase();
            let Some(rel) = lower[pos..].find(&needle) else {
                break;
            };
            let idx = pos + rel;
            if pattern.eq_ignore_ascii_case("sk-")
                || pattern.eq_ignore_ascii_case("sk-ant-")
                || pattern.eq_ignore_ascii_case("ghp_")
                || pattern.eq_ignore_ascii_case("xoxb-")
            {
                let end = out[idx..]
                    .char_indices()
                    .skip(1)
                    .find(|(_, c)| !c.is_ascii_alphanumeric() && *c != '-' && *c != '_')
                    .map(|(i, _)| idx + i)
                    .unwrap_or(out.len());
                let marker = if pattern.eq_ignore_ascii_case("sk-") {
                    "sk-[REDACTED]"
                } else {
                    "[REDACTED]"
                };
                out.replace_range(idx..end, marker);
                // Continue scanning the whole line (not break): one line may
                // hold multiple keys.
                pos = idx + marker.len();
                continue;
            }
            if pattern.eq_ignore_ascii_case("bearer ") {
                // Replace only the token (up to quote/whitespace/comma/brace).
                let start = idx + pattern.len();
                let len = out[start..]
                    .char_indices()
                    .find(|(_, c)| {
                        c.is_whitespace() || *c == '"' || *c == '\'' || *c == ',' || *c == '}'
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(out[start..].len());
                if len == 0 {
                    break;
                }
                out.replace_range(start..start + len, MARKER);
                pos = start + MARKER.len();
                continue;
            }
            // `key = "secret"` / `key: secret`: replace only the value token.
            let sep_rel = out[idx..].find('=').or_else(|| out[idx..].find(':'));
            let Some(sep_rel) = sep_rel else {
                break;
            };
            let value_start = idx + sep_rel + 1;
            let trim_off = out[value_start..]
                .char_indices()
                .find(|(_, c)| !c.is_whitespace() && *c != '"')
                .map(|(i, _)| i)
                .unwrap_or(out[value_start..].len());
            let abs_start = value_start + trim_off;
            let tok_len = out[abs_start..]
                .char_indices()
                .find(|(_, c)| {
                    c.is_whitespace()
                        || *c == '"'
                        || *c == '\''
                        || *c == ','
                        || *c == '}'
                        || *c == ']'
                })
                .map(|(i, _)| i)
                .unwrap_or(out[abs_start..].len().min(256));
            if tok_len == 0 {
                break;
            }
            out.replace_range(abs_start..abs_start + tok_len, MARKER);
            pos = abs_start + MARKER.len();
        }
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
    s.bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
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
