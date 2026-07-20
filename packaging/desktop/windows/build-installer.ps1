<#
.SYNOPSIS
  Build DeepSeekCode Windows NSIS installer from a release binary staging directory.
.PARAMETER Version
  Release version without leading v (e.g. 0.1.0).
.PARAMETER TargetDir
  Cargo target directory (default: target).
.PARAMETER RustTarget
  Rust target triple (default: x86_64-pc-windows-msvc).
.PARAMETER SkipSign
  Skip minisign artifact signing.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    [string]$TargetDir = "target",
    [string]$RustTarget = "x86_64-pc-windows-msvc",
    [switch]$SkipSign
)

$ErrorActionPreference = "Stop"
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
Set-Location $repoRoot

function Import-DotEnv {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return }
    Get-Content $Path | ForEach-Object {
        if ($_ -match '^\s*([^#=]+)=(.*)\s*$') {
            $key = $matches[1].Trim()
            $value = $matches[2].Trim()
            if ($value -match '^"(.*)"$') { $value = $matches[1] }
            Set-Item -Path "env:$key" -Value $value
        }
    }
}

Import-DotEnv (Join-Path $repoRoot ".env")

$releaseDir = Join-Path $repoRoot "$TargetDir\$RustTarget\release"
$exePath = Join-Path $releaseDir "deepseekcode.exe"
if (-not (Test-Path $exePath)) {
    throw "Release binary not found: $exePath`nRun: cargo build --release -p deepseekcode-desktop --target $RustTarget"
}

$packagingRoot = Join-Path $repoRoot "packaging\desktop\windows"
$stagingDir = Join-Path $packagingRoot "staging"
$distDir = Join-Path $packagingRoot "dist"

if (Test-Path $stagingDir) { Remove-Item $stagingDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null
New-Item -ItemType Directory -Force -Path $distDir | Out-Null

Copy-Item $exePath (Join-Path $stagingDir "deepseekcode.exe") -Force
Get-ChildItem $releaseDir -Filter "*.dll" -ErrorAction SilentlyContinue | ForEach-Object {
    Copy-Item $_.FullName $stagingDir -Force
}

$licensePath = Join-Path $repoRoot "LICENSE"
if (Test-Path $licensePath) {
    Copy-Item $licensePath (Join-Path $stagingDir "LICENSE") -Force
}

Write-Host "[*] Staging directory:" $stagingDir -ForegroundColor Cyan
Get-ChildItem $stagingDir | ForEach-Object { Write-Host "    $($_.Name)" }

function Resolve-MakensisPath {
    $cmd = Get-Command makensis -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"),
        (Join-Path $env:ProgramFiles "NSIS\makensis.exe"),
        "C:\Program Files (x86)\NSIS\makensis.exe",
        "C:\Program Files\NSIS\makensis.exe"
    )
    if ($env:NSIS_HOME) {
        $candidates += (Join-Path $env:NSIS_HOME "makensis.exe")
    }

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return $candidate
        }
    }
    return $null
}

$makensisPath = Resolve-MakensisPath
if (-not $makensisPath) {
    throw @"
makensis not found. NSIS is installed but not on PATH, or not installed.

Install options:
  winget install NSIS.NSIS
  https://nsis.sourceforge.io/Download

After install, either add NSIS to PATH or re-run this script (it auto-detects default install paths).
"@
}
Write-Host "[*] Using makensis:" $makensisPath -ForegroundColor Cyan

$nsisArgs = @(
    "/DAPP_VERSION=$Version",
    "/DSTAGING_DIR=$stagingDir",
    "/DOUT_DIR=$distDir",
    (Join-Path $packagingRoot "installer.nsi")
)
Write-Host "[*] Running makensis..." -ForegroundColor Cyan
& $makensisPath @nsisArgs
if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE" }

$installerName = "DeepSeekCode-$Version-x64-setup.exe"
$installerPath = Join-Path $distDir $installerName
if (-not (Test-Path $installerPath)) {
    throw "Installer not produced: $installerPath"
}

Write-Host "[+] Installer:" $installerPath -ForegroundColor Green

if (-not $SkipSign) {
    $signScript = Join-Path $repoRoot "scripts\release\sign-artifact.ps1"
    if (Test-Path $signScript) {
        & $signScript -ArtifactPath $installerPath
    } else {
        Write-Host "[!] sign-artifact.ps1 missing; skipping signature" -ForegroundColor Yellow
    }
}

# SHA256 checksum for release manifest
$hash = (Get-FileHash -Algorithm SHA256 $installerPath).Hash.ToLower()
$checksumFile = Join-Path $distDir "DeepSeekCode-$Version-x64-setup.sha256"
Set-Content -Path $checksumFile -Value "$hash  $installerName" -Encoding ascii -NoNewline
Write-Host "[+] Checksum:" $checksumFile -ForegroundColor Green

Write-Host "[+] Done." -ForegroundColor Green
