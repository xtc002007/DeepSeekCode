/// Provider context-window overflow detection from LLM error responses.
pub fn is_context_overflow(message: &str) -> bool {
    let lower = message.to_lowercase();
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
        "input length",
        "exceeds",
        "prompt too long",
        "too large for model with",
        "model_context_window_exceeded",
        "max_tokens exceeded",
        "max tokens exceeded",
    ];
    if SUBSTRINGS.iter().any(|needle| lower.contains(needle)) {
        // Require stronger signals for ambiguous substrings.
        if lower.contains("rate limit") && !lower.contains("context") {
            return false;
        }
        return true;
    }
    lower.contains("413") && lower.contains("context")
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
