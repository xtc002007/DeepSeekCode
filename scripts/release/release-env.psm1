<# .SYNOPSIS
  Load .env and resolve signing key paths for release scripts.
#>
function Import-ReleaseDotEnv {
    param([string]$Root)
    $envFile = Join-Path $Root ".env"
    if (-not (Test-Path $envFile)) { return }
    Get-Content $envFile | ForEach-Object {
        if ($_ -match '^\s*([^#=]+)=(.*)\s*$') {
            $key = $matches[1].Trim()
            $value = $matches[2].Trim()
            if ($value -match '^"(.*)"$') { $value = $matches[1] }
            Set-Item -Path "env:$key" -Value $value
        }
    }
}

function Get-ReleaseEnvValue {
    param(
        [string]$Name,
        [string]$Default = $null
    )
    $fromEnv = [Environment]::GetEnvironmentVariable($Name)
    if ($fromEnv) { return $fromEnv.Trim() }
    return $Default
}

function Resolve-ReleasePath {
    param(
        [string]$Root,
        [string]$PathValue
    )
    if ([string]::IsNullOrWhiteSpace($PathValue)) { return $null }
    if ([System.IO.Path]::IsPathRooted($PathValue)) { return $PathValue }
    return Join-Path $Root $PathValue
}

function Get-SigningKeyPaths {
    param([string]$Root)
    Import-ReleaseDotEnv -Root $Root
    $privateRel = Get-ReleaseEnvValue "DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PATH" "signing/deepseekcode.key"
    $publicRel = Get-ReleaseEnvValue "DEEPSEEKCODE_SIGNING_PUBLIC_KEY_PATH" "signing/deepseekcode.key.pub"
    return @{
        PrivateKey = Resolve-ReleasePath -Root $Root -PathValue $privateRel
        PublicKey = Resolve-ReleasePath -Root $Root -PathValue $publicRel
        Password = Get-ReleaseEnvValue "DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PASSWORD"
    }
}

Export-ModuleMember -Function Import-ReleaseDotEnv, Get-ReleaseEnvValue, Resolve-ReleasePath, Get-SigningKeyPaths
