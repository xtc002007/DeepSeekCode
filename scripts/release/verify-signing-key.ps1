<# .SYNOPSIS
  Verify DeepSeekCode minisign signing key before CI upload or local release.
#>
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Import-Module (Join-Path $PSScriptRoot "release-env.psm1") -Force

$keys = Get-SigningKeyPaths -Root $root
if (-not (Test-Path $keys.PrivateKey)) {
    throw "Missing private key: $($keys.PrivateKey). Run scripts/release/generate-signing-key.ps1"
}
if (-not (Test-Path $keys.PublicKey)) {
    throw "Missing public key: $($keys.PublicKey)"
}

$minisign = Get-Command minisign -ErrorAction SilentlyContinue
if (-not $minisign) {
    cargo install minisign --locked 2>$null
    $minisign = Get-Command minisign -ErrorAction SilentlyContinue
}
if (-not $minisign) { throw "minisign not found" }

$testFile = Join-Path $env:TEMP "deepseekcode-sign-test.txt"
"verify" | Set-Content -Path $testFile -Encoding ascii -NoNewline

try {
    $signArgs = @("-Sm", $testFile, "-s", $keys.PrivateKey, "-x", "$testFile.minisig")
    if ($null -ne $keys.Password -and $keys.Password.Length -gt 0) {
        $signArgs += @("-W")
        $env:MINISIGN_PASSWORD = $keys.Password
    }
    & minisign @signArgs
    if ($LASTEXITCODE -ne 0) { throw "minisign sign failed" }

    & minisign -Vm $testFile -p $keys.PublicKey -x "$testFile.minisig"
    if ($LASTEXITCODE -ne 0) { throw "minisign verify failed" }

    Write-Host "[+] Signing key and password are valid." -ForegroundColor Green
}
finally {
    Remove-Item $testFile, "$testFile.minisig" -ErrorAction SilentlyContinue
    Remove-Item Env:MINISIGN_PASSWORD -ErrorAction SilentlyContinue
}
