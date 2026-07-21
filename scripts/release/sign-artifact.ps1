<# .SYNOPSIS
  Sign a release artifact with minisign (produces .minisig sidecar).
.PARAMETER ArtifactPath
  Path to the file to sign (e.g. NSIS installer .exe).
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ArtifactPath,
    [switch]$Required
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path $ArtifactPath)) { throw "Artifact not found: $ArtifactPath" }

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Import-Module (Join-Path $PSScriptRoot "release-env.psm1") -Force

$keys = Get-SigningKeyPaths -Root $root

# CI: private key injected as env var content
$privateKeyInline = [Environment]::GetEnvironmentVariable("DEEPSEEKCODE_SIGNING_PRIVATE_KEY")
$secretKeyPath = $keys.PrivateKey

if ($privateKeyInline) {
    $tempKey = Join-Path $env:TEMP "deepseekcode-signing.key"
    Set-Content -Path $tempKey -Value $privateKeyInline -Encoding utf8 -NoNewline
    $secretKeyPath = $tempKey
} elseif (-not (Test-Path $secretKeyPath)) {
    if ($Required) {
        throw "No signing key. Set DEEPSEEKCODE_SIGNING_PRIVATE_KEY (CI) or create signing/deepseekcode.key (local)."
    }
    Write-Host "[!] No signing key; skipping signature (local build). Use -SkipSign to silence, or run generate-signing-key.ps1" -ForegroundColor Yellow
    return
}

$minisignExe = Resolve-MinisignExe

$sigPath = "$ArtifactPath.minisig"
if (Test-Path $sigPath) { Remove-Item $sigPath -Force }

$password = [Environment]::GetEnvironmentVariable("DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PASSWORD")
if ($null -eq $password) { $password = $keys.Password }

try {
    Write-Host "[*] Signing $ArtifactPath ..." -ForegroundColor Cyan
    $exit = Invoke-MinisignSign -MinisignExe $minisignExe `
        -ArtifactPath $ArtifactPath -SecretKeyPath $secretKeyPath -SigPath $sigPath -Password $password
    if ($exit -ne 0) { throw "minisign sign failed with exit code $exit" }
    Write-Host "[+] Signature: $sigPath" -ForegroundColor Green
}
finally {
    if ($privateKeyInline -and (Test-Path $secretKeyPath)) {
        Remove-Item $secretKeyPath -Force -ErrorAction SilentlyContinue
    }
}
