<# .SYNOPSIS
  Write latest.json updater manifest for signed desktop release (future auto-update).
.PARAMETER Version
  Release version without v prefix.
.PARAMETER InstallerName
  NSIS installer filename.
.PARAMETER Sha256
  SHA256 hex hash of installer.
.PARAMETER OutputPath
  Path to write latest.json.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$InstallerName,
    [Parameter(Mandatory = $true)]
    [string]$Sha256,
    [Parameter(Mandatory = $true)]
    [string]$OutputPath
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$publicKeyPath = Join-Path $root "signing\deepseekcode.key.pub"
$publicKey = [Environment]::GetEnvironmentVariable("DEEPSEEKCODE_SIGNING_PUBLIC_KEY")
if (-not $publicKey -and (Test-Path $publicKeyPath)) {
    $publicKey = (Get-Content $publicKeyPath -Raw).Trim()
}

$repo = [Environment]::GetEnvironmentVariable("GITHUB_REPOSITORY")
if (-not $repo) {
    # Fail closed: an empty `url` previously shipped in latest.json, pointing
    # auto-update at nothing. Pass -EndpointUrl explicitly for local builds.
    throw "GITHUB_REPOSITORY is empty; cannot build a download URL. Set GITHUB_REPOSITORY (CI) or extend this script with -EndpointUrl for local manifests."
}
$endpoint = "https://github.com/$repo/releases/download/v$Version/$InstallerName"

$manifest = [ordered]@{
    version = $Version
    platform = "windows-x64"
    url = $endpoint
    sha256 = $Sha256
    signature = "$InstallerName.minisig"
    pubkey = $publicKey
    notes = "DeepSeekCode $Version"
}

$manifest | ConvertTo-Json -Depth 4 | Set-Content -Path $OutputPath -Encoding utf8
Write-Host "[+] Updater manifest: $OutputPath" -ForegroundColor Green
