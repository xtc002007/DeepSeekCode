<# .SYNOPSIS
  Install NSIS via winget (if missing) and verify makensis is available.
#>
$ErrorActionPreference = "Stop"

function Find-Makensis {
    $cmd = Get-Command makensis -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $paths = @(
        "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
        "$env:ProgramFiles\NSIS\makensis.exe"
    )
    foreach ($p in $paths) {
        if (Test-Path $p) { return $p }
    }
    return $null
}

$found = Find-Makensis
if ($found) {
    Write-Host "[+] NSIS already available: $found" -ForegroundColor Green
    & $found /VERSION
    exit 0
}

Write-Host "[*] NSIS not found; installing via winget..." -ForegroundColor Cyan
winget install --id NSIS.NSIS -e --accept-source-agreements --accept-package-agreements
if ($LASTEXITCODE -ne 0) {
    throw "winget install failed. Install manually: https://nsis.sourceforge.io/Download"
}

$found = Find-Makensis
if (-not $found) {
    throw "NSIS installed but makensis.exe not found. Restart terminal or add NSIS to PATH."
}
Write-Host "[+] NSIS installed: $found" -ForegroundColor Green
& $found /VERSION
