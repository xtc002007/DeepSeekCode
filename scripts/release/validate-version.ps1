<# .SYNOPSIS
  Validate release tag matches workspace Cargo.toml version.
.PARAMETER TagName
  Git tag (e.g. v0.1.0).
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$TagName
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$cargoToml = Join-Path $root "Cargo.toml"

if ($TagName -notmatch '^v(?<ver>\d+\.\d+\.\d+(?:-[\w.]+)?)$') {
    throw "Invalid release tag '$TagName'. Expected format: v0.1.0"
}
$tagVersion = $Matches['ver']

$content = Get-Content $cargoToml -Raw
if ($content -notmatch '(?m)^version\s*=\s*"(?<ver>[^"]+)"') {
    throw "Could not read workspace version from Cargo.toml"
}
$cargoVersion = $Matches['ver']

if ($tagVersion -ne $cargoVersion) {
    throw "Release tag v$tagVersion does not match Cargo.toml workspace version $cargoVersion"
}

Write-Host "[+] Version OK: tag v$tagVersion matches Cargo.toml" -ForegroundColor Green
Write-Output $tagVersion
