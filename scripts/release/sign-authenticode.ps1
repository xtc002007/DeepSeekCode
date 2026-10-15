<# .SYNOPSIS
  Optional Authenticode signing for Windows installer (requires signtool + PFX).
.PARAMETER ArtifactPath
  Path to .exe installer.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ArtifactPath
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path $ArtifactPath)) { throw "Artifact not found: $ArtifactPath" }

$pfxBase64 = [Environment]::GetEnvironmentVariable("CODE_PLUS_PLUS_WINDOWS_PFX_BASE64")
$pfxPassword = [Environment]::GetEnvironmentVariable("CODE_PLUS_PLUS_WINDOWS_PFX_PASSWORD")
if (-not $pfxBase64) {
    Write-Host "[*] CODE_PLUS_PLUS_WINDOWS_PFX_BASE64 not set; skipping Authenticode" -ForegroundColor DarkYellow
    return
}

$pfxPath = Join-Path $env:TEMP "code-plus-plus-signing.pfx"
[System.IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($pfxBase64))

$signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
if (-not $signtool) {
    # Windows SDK signtool
    $sdkPath = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path $sdkPath) {
        $latest = Get-ChildItem $sdkPath -Directory | Sort-Object Name -Descending | Select-Object -First 1
        $candidate = Join-Path $latest.FullName "x64\signtool.exe"
        if (Test-Path $candidate) { $signtool = $candidate }
    }
}
if (-not $signtool) {
    Write-Host "[!] signtool not found; skipping Authenticode" -ForegroundColor Yellow
    Remove-Item $pfxPath -Force -ErrorAction SilentlyContinue
    return
}

$timestampUrl = "http://timestamp.digicert.com"
Write-Host "[*] Authenticode signing $ArtifactPath ..." -ForegroundColor Cyan
& $signtool sign /f $pfxPath /p $pfxPassword /tr $timestampUrl /td sha256 /fd sha256 $ArtifactPath
if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }
Write-Host "[+] Authenticode signature applied" -ForegroundColor Green
Remove-Item $pfxPath -Force -ErrorAction SilentlyContinue
