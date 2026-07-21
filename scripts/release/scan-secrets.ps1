# Scan the repository for leaked secrets (gitleaks).
# Auto-installs gitleaks to .tools/ on first run if missing.
# Usage: .\scripts\release\scan-secrets.ps1

$ErrorActionPreference = "Stop"
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
Set-Location $Root

function Find-Gitleaks {
    $cmd = Get-Command gitleaks -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $local = Join-Path $Root ".tools/gitleaks.exe"
    if (Test-Path $local) { return $local }
    return $null
}

$gitleaks = Find-Gitleaks
if (-not $gitleaks) {
    & (Join-Path $Root "scripts/bootstrap-gitleaks.ps1")
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $gitleaks = Find-Gitleaks
}

if (-not $gitleaks) {
    Write-Host "[!] gitleaks not available after bootstrap." -ForegroundColor Red
    exit 2
}

Write-Host "[*] Scanning $Root with gitleaks..." -ForegroundColor Cyan
$config = Join-Path $Root ".gitleaks.toml"
$args = @("detect", "--source", $Root, "--config", $config, "--verbose", "--redact")
if (Test-Path (Join-Path $Root ".git")) {
    $args += @("--log-opts", "--all")
}

& $gitleaks @args
if ($LASTEXITCODE -ne 0) {
    Write-Host "[!] Secret scan failed — review findings above." -ForegroundColor Red
    exit $LASTEXITCODE
}

Write-Host "[+] No secrets detected." -ForegroundColor Green
