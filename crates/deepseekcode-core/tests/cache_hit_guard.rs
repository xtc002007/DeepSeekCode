//! Issue-16 (+P2-6 scope note): DeepSeek prefix-cache 90% aggregate guard.
//!
//! Simulates a 14-turn long session without compaction: turn 1 is a cold
//! miss (new prefix), turns 2..14 are warm (stable prefix, ~95% hit).
//! Aggregate must exceed 90% — the reference Reasonix bar. `warm` mode primes
//! one turn first (steady-state); `cold` measures from fresh (fair). The math
//! is identical; only the narrative differs. Release CI sets
//! `REASONIX_RELEASE_CACHE_GUARD=1` to enforce.
//!
//! SCOPE: these are SYNTHETIC payloads exercising the LOCAL aggregation math
//! (`aggregate_message_completed`), not live DeepSeek server-side prefix
//! warmth. They lock that accounting stays correct under cold/warm shapes;
//! true server hit-rate is observed live via `GET /session/{id}/cache-status`.

use deepseekcode_core::cache_status::aggregate_message_completed;
use serde_json::json;

fn fourteen_turn_payloads() -> Vec<serde_json::Value> {
    let mut payloads = vec![json!({
        "input_tokens": 10000u64,
        "output_tokens": 200u64,
        "cache_hit": 0u64,
        "cache_miss": 10000u64,
        "cache_receipt": " (0 cached / 10000 new)",
    })];
    // Warm turns hold ~96.7% (only the new tail misses); cold turn 1
    // amortizes over 14 turns to just above 90%.
    for _ in 0..13 {
        payloads.push(json!({
            "input_tokens": 12000u64,
            "output_tokens": 200u64,
            "cache_hit": 11600u64,
            "cache_miss": 400u64,
            "cache_receipt": " (11600 cached / 400 new)",
        }));
    }
    payloads
}

#[test]
fn cache_hit_climbs_without_compaction() {
    let status = aggregate_message_completed(&fourteen_turn_payloads());
    assert_eq!(status.turns, 14);
    assert!(
        status.hit_rate > 0.90,
        "14-turn warm session must hold 90%+ hit, got {:.3}",
        status.hit_rate
    );
    assert!(status.receipt().contains("hit over 14 turns"));
}

#[test]
fn release_guard_90() {
    // Issue-11: enforced on release AND on any PR touching sensitive files
    // (the gate script invokes this test in both cases). Kept env-gated for
    // direct `cargo test` runs so unrelated local runs stay informational.
    if std::env::var("REASONIX_RELEASE_CACHE_GUARD")
        .ok()
        .as_deref()
        != Some("1")
        && std::env::var("CACHE_SENSITIVE_TOUCHED").ok().as_deref() != Some("1")
    {
        return;
    }
    let status = aggregate_message_completed(&fourteen_turn_payloads());
    assert!(
        status.hit_rate >= 0.90,
        "RELEASE guard: aggregate {:.4} < 0.90",
        status.hit_rate
    );
}

#[test]
fn cache_hit_warm_steady_state() {
    // Issue-11 warm mode: prime (turn 1) is discarded; steady-state turns
    // 2..14 alone must hold >=95%. This is what `check-cache-impact -Mode
    // warm` primes then measures.
    let payloads = fourteen_turn_payloads();
    let steady: Vec<serde_json::Value> = payloads[1..].to_vec();
    let status = aggregate_message_completed(&steady);
    assert_eq!(status.turns, 13);
    assert!(
        status.hit_rate >= 0.95,
        "warm steady-state must hold 95%+, got {:.4}",
        status.hit_rate
    );
}

#[test]
fn cold_resume_does_not_rewrite_history_contract() {
    // Documents the invariant: cold resume only records cache state, never
    // rewrites the canonical transcript. The guard itself is behavioral
    // (see `message_visible_to_runner`), this test locks the accounting
    // side: a cold turn contributes a miss, warm turns recover it.
    let mut payloads = vec![json!({
        "input_tokens": 10000u64, "output_tokens": 50u64,
        "cache_hit": 0u64, "cache_miss": 10000u64,
    })];
    for _ in 0..5 {
        payloads.push(json!({
            "input_tokens": 10000u64, "output_tokens": 50u64,
            "cache_hit": 9500u64, "cache_miss": 500u64,
        }));
    }
    let status = aggregate_message_completed(&payloads);
    assert!(
        status.hit_rate > 0.79,
        "5 warm turns must recover cold miss"
    );
}
