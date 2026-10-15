<# .SYNOPSIS
  Generate minisign ed25519 keypair for Code++ release signing.
#>
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$signingDir = Join-Path $root "signing"
$privateKey = Join-Path $signingDir "code-plus-plus.key"
$publicKey = Join-Path $signingDir "code-plus-plus.key.pub"

New-Item -ItemType Directory -Force -Path $signingDir | Out-Null

if ((Test-Path $privateKey) -or (Test-Path $publicKey)) {
    throw "Keys already exist under signing/. Remove them first if you intend to regenerate."
}

$minisign = Get-Command minisign -ErrorAction SilentlyContinue
if (-not $minisign) {
    Write-Host "[*] minisign not found; trying cargo install..." -ForegroundColor Cyan
    cargo install minisign --locked
    $minisign = Get-Command minisign -ErrorAction SilentlyContinue
    if (-not $minisign) { throw "minisign is required. Install from https://github.com/jedisct1/minisign" }
}

Write-Host "[*] Generating ed25519 keypair (password-protected)..." -ForegroundColor Cyan
& minisign -G -f -p $publicKey -s $privateKey
if ($LASTEXITCODE -ne 0) { throw "minisign key generation failed" }

Write-Host "[+] Private key: $privateKey (DO NOT commit)" -ForegroundColor Green
Write-Host "[+] Public key:  $publicKey (safe to commit)" -ForegroundColor Green
Write-Host "[*] Next: copy .env.example to .env, then run scripts/release/verify-signing-key.ps1" -ForegroundColor Cyan
