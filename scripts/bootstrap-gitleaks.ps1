# Ensure gitleaks is available (PATH or .tools/). Does not scan.
# Usage: .\scripts\bootstrap-gitleaks.ps1 [-Quiet]

param(
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent

function Write-Info([string]$Message) {
    if (-not $Quiet) {
        Write-Host $Message -ForegroundColor Cyan
    }
}

function Find-Gitleaks {
    $cmd = Get-Command gitleaks -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $local = Join-Path $Root ".tools/gitleaks.exe"
    if (Test-Path $local) { return $local }
    return $null
}

$gitleaks = Find-Gitleaks
if ($gitleaks) {
    if (-not $Quiet) {
        Write-Host "[+] gitleaks: $gitleaks" -ForegroundColor Green
    }
    exit 0
}

$toolsDir = Join-Path $Root ".tools"
New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null
$zip = Join-Path $env:TEMP "gitleaks.zip"
$url = "https://github.com/gitleaks/gitleaks/releases/download/v8.24.2/gitleaks_8.24.2_windows_x64.zip"

Write-Info "[*] Downloading gitleaks (first use)..."
try {
    Invoke-WebRequest -Uri $url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $toolsDir -Force
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
} catch {
    Write-Host "[!] gitleaks download failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

$gitleaks = Join-Path $toolsDir "gitleaks.exe"
if (-not (Test-Path $gitleaks)) {
    Write-Host "[!] gitleaks install failed — binary missing after extract." -ForegroundColor Red
    exit 1
}

Write-Info "[+] gitleaks installed: $gitleaks"
exit 0
