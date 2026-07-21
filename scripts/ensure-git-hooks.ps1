# Point this clone at tracked git hooks (core.hooksPath). Idempotent.
# Usually auto-run by `cargo build`; manual use is optional.

param(
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
$HooksPath = "scripts/git-hooks"

if (-not (Test-Path (Join-Path $Root ".git"))) {
    if (-not $Quiet) {
        Write-Host "[!] Not a git repository — skipping hook setup." -ForegroundColor Yellow
    }
    exit 0
}

$preCommit = Join-Path (Join-Path $Root $HooksPath) "pre-commit"
if (-not (Test-Path $preCommit)) {
    Write-Error "Missing $HooksPath/pre-commit"
}

$current = git -C $Root config --get core.hooksPath 2>$null
if ($current -eq $HooksPath) {
    if (-not $Quiet) {
        Write-Host "[+] Git hooks already configured (core.hooksPath=$HooksPath)" -ForegroundColor Green
    }
    exit 0
}

git -C $Root config core.hooksPath $HooksPath
if (-not $Quiet) {
    Write-Host "[+] Configured git hooks (core.hooksPath=$HooksPath)" -ForegroundColor Green
    Write-Host "    Commits will scan staged secrets automatically." -ForegroundColor Cyan
}
