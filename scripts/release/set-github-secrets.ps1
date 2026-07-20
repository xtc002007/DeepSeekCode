<# .SYNOPSIS
  Upload DeepSeekCode release signing secrets to GitHub Actions.
.PARAMETER SkipVerify
  Skip local minisign verification before upload.
#>
param([switch]$SkipVerify)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Import-Module (Join-Path $PSScriptRoot "release-env.psm1") -Force

$repo = Get-ReleaseEnvValue "DEEPSEEKCODE_GITHUB_REPO"
if (-not $repo) {
    $remote = git -C $root remote get-url origin 2>$null
    if ($remote -match 'github\.com[:/](.+?)(?:\.git)?$') {
        $repo = $matches[1]
    }
}
if (-not $repo) {
    throw "Set DEEPSEEKCODE_GITHUB_REPO in .env or configure git remote origin"
}

$keys = Get-SigningKeyPaths -Root $root
if (-not (Test-Path $keys.PrivateKey)) { throw "Missing $($keys.PrivateKey)" }
if (-not (Test-Path $keys.PublicKey)) { throw "Missing $($keys.PublicKey)" }

if ($null -eq $keys.Password) {
    throw "Set DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PASSWORD in .env (use empty value if key has no password)"
}

gh auth status *> $null
if ($LASTEXITCODE -ne 0) { throw "gh is not logged in. Run: gh auth login" }

if (-not $SkipVerify) {
    Write-Host "[*] Verifying signing key locally..." -ForegroundColor Cyan
    & (Join-Path $PSScriptRoot "verify-signing-key.ps1")
}

$privateKey = (Get-Content $keys.PrivateKey -Raw).TrimEnd("`r", "`n")
$publicKey = (Get-Content $keys.PublicKey -Raw).TrimEnd("`r", "`n")

Write-Host "[*] Uploading secrets to $repo ..." -ForegroundColor Cyan
$privateKey | gh secret set DEEPSEEKCODE_SIGNING_PRIVATE_KEY -R $repo
if ([string]::IsNullOrEmpty($keys.Password)) {
    '' | gh secret set DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PASSWORD -R $repo
} else {
    gh secret set DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PASSWORD -R $repo -b="$($keys.Password)"
}
$publicKey | gh secret set DEEPSEEKCODE_SIGNING_PUBLIC_KEY -R $repo

Write-Host "[+] GitHub secrets configured for $repo" -ForegroundColor Green
Write-Host "    Trigger release: git tag v0.1.0 && git push origin v0.1.0" -ForegroundColor Cyan
Write-Host "    Or: gh workflow run Release -R $repo -f tag_name=v0.1.0" -ForegroundColor Cyan
