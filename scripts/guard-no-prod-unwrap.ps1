# Guard: production code must not gain panic!/unwrap()/expect().
# Test-only (`*/tests/*`, `*/benches/*`, `build.rs`, `src/bin/*`, `#[cfg(test)]`,
# `#[test] fn`) panics are intentional (fail-fast tests, build scripts, CLI tools).
# CI runs: powershell -File scripts/guard-no-prod-unwrap.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$violations = @()
Get-ChildItem -Path (Join-Path $root "crates") -Recurse -Filter "*.rs" | ForEach-Object {
  $path = $_.FullName
  $rel = $path.Substring($root.Length + 1)
  if ($rel -match "[/\\]tests?[/\\]" -or $rel -match "[/\\]benches[/\\]" -or $rel -match "build\.rs$" -or $rel -match "src[/\\]bin[/\\]") { return }
  $lines = Get-Content -LiteralPath $path
  $inTestMod = $false
  for ($i = 0; $i -lt $lines.Count; $i++) {
    $line = $lines[$i]
    if ($line -match "#\[cfg\(test\)\]") { $inTestMod = $true }
    if ($inTestMod) { continue }
    $isTestFn = $false
    for ($k = [Math]::Max(0, $i - 6); $k -lt $i; $k++) {
      if ($lines[$k] -match "#\[test\]") { $isTestFn = $true; break }
    }
    if ($isTestFn) { continue }
    $t = $line.Trim()
    if ($t.StartsWith("//")) { continue }
    if ($line -match "\bpanic!\s*\(" -or $line -match "\.unwrap\(\)" -or $line -match "\.expect\(") {
      $violations += "${rel}:$($i + 1): $t"
    }
  }
}
if ($violations.Count -gt 0) {
  Write-Host "FAIL: production panic sites found:"
  $violations | ForEach-Object { Write-Host "  $_" }
  exit 1
}
Write-Host "OK: no production panic!/unwrap()/expect()."
