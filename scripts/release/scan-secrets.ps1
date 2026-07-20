# Scan the repository for leaked secrets (gitleaks).
# Install: https://github.com/gitleaks/gitleaks#installing
# Usage: .\scripts\release\scan-secrets.ps1 [-InstallIfMissing]

param(
    [switch]$InstallIfMissing
)

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
if (-not $gitleaks -and $InstallIfMissing) {
    $toolsDir = Join-Path $Root ".tools"
    New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null
    $zip = Join-Path $env:TEMP "gitleaks.zip"
    $url = "https://github.com/gitleaks/gitleaks/releases/download/v8.24.2/gitleaks_8.24.2_windows_x64.zip"
    Write-Host "[*] Downloading gitleaks..." -ForegroundColor Cyan
    Invoke-WebRequest -Uri $url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $toolsDir -Force
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    $gitleaks = Join-Path $toolsDir "gitleaks.exe"
}

if (-not $gitleaks) {
    Write-Host "[!] gitleaks not found. Install from https://github.com/gitleaks/gitleaks or re-run with -InstallIfMissing" -ForegroundColor Yellow
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
