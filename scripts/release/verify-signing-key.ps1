<# .SYNOPSIS
  Verify Code++ minisign signing key before CI upload or local release.
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

$minisignExe = Resolve-MinisignExe

$testFile = Join-Path $env:TEMP "code-plus-plus-sign-test.txt"
"verify" | Set-Content -Path $testFile -Encoding ascii -NoNewline

try {
    $exit = Invoke-MinisignSign -MinisignExe $minisignExe `
        -ArtifactPath $testFile -SecretKeyPath $keys.PrivateKey -SigPath "$testFile.minisig" -Password $keys.Password
    if ($exit -ne 0) { throw "minisign sign failed" }

    & $minisignExe -Vm $testFile -p $keys.PublicKey -x "$testFile.minisig"
    if ($LASTEXITCODE -ne 0) { throw "minisign verify failed" }

    Write-Host "[+] Signing key and password are valid." -ForegroundColor Green
}
finally {
    Remove-Item $testFile, "$testFile.minisig" -ErrorAction SilentlyContinue
}
