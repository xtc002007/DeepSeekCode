# Optional manual setup (same as auto `cargo build` hook configuration).
# Usage:
#   .\scripts\install-git-hooks.ps1
#   .\scripts\install-git-hooks.ps1 -InstallGitleaks

param(
    [switch]$InstallGitleaks
)

$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent

& (Join-Path $PSScriptRoot "ensure-git-hooks.ps1")

if ($InstallGitleaks) {
    try {
        & (Join-Path $PSScriptRoot "bootstrap-gitleaks.ps1")
    } catch {
        Write-Host "[!] gitleaks bootstrap failed: $($_.Exception.Message)" -ForegroundColor Yellow
    }
}

Write-Host ""
Write-Host "Normal git workflow works: git add -> git commit -> git push" -ForegroundColor Cyan
Write-Host "Full-repo scan (release): .\scripts\release\scan-secrets.ps1" -ForegroundColor Cyan
