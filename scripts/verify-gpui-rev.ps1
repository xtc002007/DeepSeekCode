# Verify: desktop GPUI git rev pinned in Cargo.toml matches Cargo.lock.
# GPUI is not on crates.io; the git rev is the single source of truth.
# CI runs: powershell -File scripts/verify-gpui-rev.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$toml = Get-Content -LiteralPath (Join-Path $root "crates/code-plus-plus-desktop/Cargo.toml") -Raw
$lock = Get-Content -LiteralPath (Join-Path $root "Cargo.lock") -Raw
$revs = [regex]::Matches($toml, 'rev\s*=\s*"([0-9a-f]{4,40})"') | ForEach-Object { $_.Groups[1].Value } | Select-Object -Unique
if ($revs.Count -eq 0) { Write-Host "FAIL: no gpui rev found in desktop Cargo.toml"; exit 1 }
$fail = $false
foreach ($rev in $revs) {
  if ($lock -notmatch [regex]::Escape($rev)) { Write-Host "FAIL: Cargo.lock missing gpui rev $rev (run cargo check -p code-plus-plus-desktop to refresh)"; $fail = $true }
  else { Write-Host "OK: Cargo.lock pins gpui rev $rev" }
}
if ($fail) { exit 1 }
Write-Host "OK: gpui rev consistent. Offline rebuilds reuse Cargo git cache; no vendor dir needed."
