# Issue-11/16: DeepSeek prefix-cache impact guard.
#
# Usage:
#   scripts/check-cache-impact.ps1            # cold: fresh session, fair comparison
#   scripts/check-cache-impact.ps1 -Mode warm # warm: prime once, then measure steady-state
#   $env:REASONIX_RELEASE_CACHE_GUARD=1       # release: always enforce 90%
#
# SCOPE NOTE (P2-6): `cold`/`warm` here describe the LOCAL aggregation math
# (synthetic payloads in `cache_hit_guard.rs`), not DeepSeek server-side
# prefix warmth. The suite locks that the accounting stays correct; true
# server hit-rate is observed live via `GET /session/{id}/cache-status`.
#
# Fails when:
# - cache-sensitive files changed without accompanying cache tests
#   (requires a `Cache-impact:` note in the change), or
# - the 90% aggregate e2e guard fails (release, or any PR touching
#   sensitive files).
param([string]$Mode = "cold")

$ErrorActionPreference = "Stop"

$Sensitive = @(
  "crates/code-plus-plus-llm/src/axis/protocol.rs",
  "crates/code-plus-plus-llm/src/axis/framing.rs",
  "crates/code-plus-plus-llm/src/providers/deepseek.rs",
  "crates/code-plus-plus-llm/src/request.rs",
  "crates/code-plus-plus-llm/src/event.rs",
  "crates/code-plus-plus-core/src/session/runner/mod.rs",
  "crates/code-plus-plus-core/src/session/runner/repair.rs",
  "crates/code-plus-plus-core/src/compaction.rs",
  "crates/code-plus-plus-core/src/tools/registry.rs",
  "crates/code-plus-plus-core/src/tools/use_capability.rs",
  "crates/code-plus-plus-core/src/tools/tool_result_page.rs",
  "crates/code-plus-plus-core/src/system_context/",
  "crates/code-plus-plus-core/src/memory.rs",
  "crates/code-plus-plus-core/src/skills.rs",
  "crates/code-plus-plus-core/src/token.rs",
  "crates/code-plus-plus-core/src/cache_status.rs",
  "crates/code-plus-plus-core/src/session/projector.rs",
  "crates/code-plus-plus-core/src/session/runner/context_reconciler.rs",
  "crates/code-plus-plus-core/src/session/runner/tool_executor.rs",
  "crates/code-plus-plus-llm/src/overflow.rs",
  "crates/code-plus-plus-storage/src/tool_output.rs",
  "crates/code-plus-plus-storage/src/session_store.rs",
  "crates/code-plus-plus-config/src/schema.rs"
)
# P1-4: `skills.rs` (catalog bytes enter the prefix), `token.rs` (rewrites
# move the estimation denominator), `session_store.rs` (lineage/covered
# persistence), and the runner/projector/reconciler/executor/overflow chain
# (request-shape changes) all move cache behavior — no bypass.

$Changed = @()
try {
  $Changed += (git diff --name-only HEAD 2>$null) -split "`n" | Where-Object { $_ -ne "" }
  $Changed += (git status --short 2>$null) | ForEach-Object { ($_ -split " ")[-1] } | Where-Object { $_ -ne "" }
} catch { }

$Touched = $Sensitive | Where-Object {
  $prefix = $_
  $Changed | Where-Object { $_ -like "$prefix*" }
}

$SensitiveTouched = $Touched.Count -gt 0
if ($SensitiveTouched) {
  Write-Host "Cache-sensitive files touched:"
  $Touched | ForEach-Object { Write-Host "  - $_" }
  # Sensitive changes must be accompanied by cache tests (or an explicit
  # Cache-impact note). Gate on test-file presence, not just log text.
  $TestTouched = @($Changed | Where-Object {
    $_ -like "*test*" -or $_ -like "*tests*" -or $_ -like "*cache_hit_guard*"
  })
  if ($TestTouched.Count -eq 0) {
    $Note = ""
    try { $Note = (git log -1 --pretty=%B 2>$null) } catch { }
    if ($Note -notmatch "Cache-impact:") {
      Write-Error ("Cache-sensitive change without accompanying cache tests. " +
        "Update/add tests under crates/*/tests or *test*.rs, or include a " +
        "`Cache-impact:` line in the commit message explaining why no test " +
        "change is needed.")
      exit 1
    }
    Write-Host "No test files touched, but Cache-impact note found; continuing."
  }
  Write-Host "Mode: $Mode (cold=fresh fair comparison, warm=primed steady-state)"
} else {
  Write-Host "No cache-sensitive changes detected. Running cache test suite anyway (Mode=$Mode)."
}

Write-Host "Running DeepSeek cache test suite..."
# P1-4: `token.rs` moves the estimation denominator for every threshold and
# receipt, so its changes always require the compaction + guard legs below
# (already unconditional here — this comment locks the requirement).
if ($Mode -eq "warm") {
  # Warm = prime once (compilation + steady-state), then measure. The prime
  # run is discarded; the second run is the gate.
  Write-Host "Warm prime run (discarded)..."
  cargo test -p code-plus-plus-core --test cache_hit_guard cache_hit_warm_steady_state 2>&1 | Select-Object -Last 5
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
cargo test -p code-plus-plus-llm cache 2>&1 | Select-Object -Last 5
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test -p code-plus-plus-llm protocol 2>&1 | Select-Object -Last 5
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test -p code-plus-plus-llm framing 2>&1 | Select-Object -Last 5
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test -p code-plus-plus-core cache_status 2>&1 | Select-Object -Last 5
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test -p code-plus-plus-core compaction 2>&1 | Select-Object -Last 5
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test -p code-plus-plus-core --test cache_hit_guard 2>&1 | Select-Object -Last 10
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# 90% gate: release always; PRs touching sensitive files always (not only
# release). Non-sensitive changes keep the informational run above.
# `CACHE_SENSITIVE_TOUCHED=1` forces the Rust test itself to enforce even
# without the release env var.
if (($env:REASONIX_RELEASE_CACHE_GUARD -eq "1") -or $SensitiveTouched) {
  Write-Host "Enforcing 90% aggregate threshold via cache_hit_guard e2e..."
  $env:CACHE_SENSITIVE_TOUCHED = "1"
  cargo test -p code-plus-plus-core --test cache_hit_guard release_guard_90 2>&1 | Select-Object -Last 10
  if ($LASTEXITCODE -ne 0) {
    Write-Error "Cache hit guard failed: long sessions must hold 90%+ hit rate."
    exit 1
  }
}

Write-Host "Cache impact guard passed."
