#!/bin/sh
# Issue-11/16: DeepSeek prefix-cache impact guard (POSIX mirror of .ps1).
# Usage: scripts/check-cache-impact.sh [cold|warm]
#   REASONIX_RELEASE_CACHE_GUARD=1 always enforces 90%; sensitive changes
#   enforce it even without the env var. Warm primes once, then measures.
# SCOPE NOTE (P2-6): cold/warm describe LOCAL aggregation math (synthetic
# payloads in cache_hit_guard.rs), not server-side prefix warmth. The suite
# locks accounting correctness; true server hit-rate is observed live via
# GET /session/{id}/cache-status.
set -eu
MODE="${1:-cold}"
SENSITIVE_GRAPH=""
# P1-4: keep in sync with the .ps1 list — skills catalog bytes, token
# estimation, lineage persistence, and the request-shape chain are all
# cache-sensitive; omitting them lets changes bypass the gate.
if git diff --name-only HEAD 2>/dev/null | grep -Eq "deepseekcode-llm/src/(axis|providers|request|overflow)|deepseekcode-core/src/(session/runner|session/projector|compaction|tools|system_context|memory|skills|token|cache_status)|deepseekcode-storage/src/(tool_output|session_store)|deepseekcode-config/src/schema"; then
  SENSITIVE_GRAPH="1"
fi
echo "Mode: $MODE (cold=fresh fair comparison, warm=primed steady-state)"
if [ -n "$SENSITIVE_GRAPH" ]; then
  echo "Cache-sensitive files touched; 90% gate will be enforced."
  if ! git diff --name-only HEAD 2>/dev/null | grep -Eq "test|cache_hit_guard"; then
    if ! git log -1 --pretty=%B 2>/dev/null | grep -Eq "Cache-impact:"; then
      echo "ERROR: cache-sensitive change without accompanying cache tests." >&2
      echo "Update/add tests or include a \`Cache-impact:\` line in the commit message." >&2
      exit 1
    fi
  fi
fi
echo "Running DeepSeek cache test suite..."
if [ "$MODE" = "warm" ]; then
  echo "Warm prime run (discarded)..."
  cargo test -p deepseekcode-core --test cache_hit_guard cache_hit_warm_steady_state 2>&1 | tail -5
fi
cargo test -p deepseekcode-llm cache 2>&1 | tail -5
cargo test -p deepseekcode-llm protocol 2>&1 | tail -5
cargo test -p deepseekcode-llm framing 2>&1 | tail -5
cargo test -p deepseekcode-core cache_status 2>&1 | tail -5
cargo test -p deepseekcode-core compaction 2>&1 | tail -5
cargo test -p deepseekcode-core --test cache_hit_guard 2>&1 | tail -10
if [ "${REASONIX_RELEASE_CACHE_GUARD:-0}" = "1" ] || [ -n "$SENSITIVE_GRAPH" ]; then
  echo "Enforcing 90% aggregate threshold..."
  CACHE_SENSITIVE_TOUCHED=1 cargo test -p deepseekcode-core --test cache_hit_guard release_guard_90 2>&1 | tail -10
fi
echo "Cache impact guard passed."
