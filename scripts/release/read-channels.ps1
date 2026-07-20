<# .SYNOPSIS
  Read packaging/channels.toml and emit channel enable flags.
#>
$ErrorActionPreference = "Stop"
$root = if ($env:GITHUB_WORKSPACE) { $env:GITHUB_WORKSPACE } else { Resolve-Path (Join-Path $PSScriptRoot "..\..") }
$channelsFile = Join-Path $root "packaging\channels.toml"
if (-not (Test-Path $channelsFile)) { throw "Missing $channelsFile" }

$content = Get-Content $channelsFile -Raw

function Get-SectionEnabled {
    param([string]$SectionName)
    $escaped = [regex]::Escape($SectionName)
    if ($content -match "(?ms)\[$escaped\][^\[]*?enabled\s*=\s*(true|false)") {
        return $Matches[1] -eq "true"
    }
    return $false
}

function Get-PlatformEnabled {
    param([string]$PlatformId)
    $blocks = [regex]::Split($content, '\[\[channels\.desktop\.platforms\]\]')
foreach ($block in $blocks) {
    if ($block -match "id\s*=\s*`"$([regex]::Escape($PlatformId))`"") {
        if ($block -match 'enabled\s*=\s*(true|false)') {
            return $Matches[1] -eq "true"
        }
    }
}
    return $false
}

$desktopEnabled = Get-SectionEnabled "channels.desktop"

[PSCustomObject]@{
    CliEnabled            = Get-SectionEnabled "channels.cli"
    DesktopEnabled        = $desktopEnabled
    CratesEnabled         = Get-SectionEnabled "channels.crates"
    DesktopWindowsEnabled = $desktopEnabled -and (Get-PlatformEnabled "windows-x64")
}
