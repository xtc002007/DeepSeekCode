/// Provider context-window overflow detection from LLM error responses.
/// The bare `"exceeds"` substring is deliberately NOT a signal: it false-
/// positives on `quota exceeds`, `rate limit exceeded`, etc. Require a
/// context/token/length companion instead.
pub fn is_context_overflow(message: &str) -> bool {
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        msg_summary = %code_plus_plus_telemetry::summarize_prompt(message, false),
        "llm overflow check start"
    );
    let lower = message.to_lowercase();
    // Fast reject: rate/quota/auth errors are never overflow, even when they
    // mention context-adjacent words.
    if lower.contains("rate limit")
        || lower.contains("quota")
        || lower.contains("insufficient_quota")
        || lower.contains("billing")
        || lower.contains("unauthorized")
        || lower.contains("invalid api key")
    {
        // Unless it is explicitly a context-length rate shape.
        if !(lower.contains("context") && lower.contains("exceed")) {
            // `rate limit exceeded` without context => not overflow.
            if lower.contains("rate") || lower.contains("quota") {
                tracing::info!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    msg_summary = %code_plus_plus_telemetry::summarize_prompt(message, false),
                    "llm overflow check: rate/quota fast-reject, not overflow"
                );
                return false;
            }
        }
    }
    const SUBSTRINGS: &[&str] = &[
        "prompt is too long",
        "input is too long for requested model",
        "exceeds the context window",
        "input token count",
        "exceeds the maximum",
        "tokens in request more than max tokens allowed",
        "maximum prompt length is",
        "reduce the length of the messages",
        "maximum context length is",
        "exceeds the limit of",
        "exceeds the available context size",
        "greater than the context length",
        "context window exceeds limit",
        "exceeded model token limit",
        "context_length_exceeded",
        "context length exceeded",
        "request entity too large",
        "context length is only",
        "prompt too long",
        "too large for model with",
        "model_context_window_exceeded",
        "max_tokens exceeded",
        "max tokens exceeded",
    ];
    if SUBSTRINGS.iter().any(|needle| lower.contains(needle)) {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            msg_summary = %code_plus_plus_telemetry::summarize_prompt(message, false),
            "llm context overflow detected via known substring"
        );
        return true;
    }
    // Gated bare-exceeds: only when paired with context/token/length terms.
    if lower.contains("exceed") {
        let companion = lower.contains("context")
            || lower.contains("token")
            || lower.contains("maximum")
            || lower.contains("max tokens")
            || lower.contains("length")
            || lower.contains("input length");
        if companion {
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                msg_summary = %code_plus_plus_telemetry::summarize_prompt(message, false),
                "llm context overflow detected via gated exceeds+companion"
            );
            return true;
        }
        tracing::debug!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            msg_summary = %code_plus_plus_telemetry::summarize_prompt(message, false),
            "llm overflow check: bare exceeds without companion, not overflow"
        );
        return false;
    }
    // Bare 413 without context words is not enough (could be file upload).
    let overflow = (lower.contains("413") || lower.contains("payload too large"))
        && (lower.contains("context")
            || lower.contains("token")
            || lower.contains("prompt")
            || lower.contains("message"));
    if overflow {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            msg_summary = %code_plus_plus_telemetry::summarize_prompt(message, false),
            "llm context overflow detected via 413/payload shape"
        );
    } else {
        tracing::debug!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            msg_summary = %code_plus_plus_telemetry::summarize_prompt(message, false),
            "llm overflow check: no overflow"
        );
    }
    if message.len() > 2000 {
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            msg_len = message.len(),
            "llm overflow check input overlong, summarized for log"
        );
    }
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        overflow,
        "llm overflow check completed"
    );
    overflow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_overflow_messages() {
        assert!(is_context_overflow("prompt is too long: 200k tokens"));
        assert!(is_context_overflow(
            "context_length_exceeded: input exceeds model limit"
        ));
        assert!(!is_context_overflow("rate limit exceeded"));
    }
}
