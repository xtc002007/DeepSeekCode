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
    $privateRel = Get-ReleaseEnvValue "CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PATH" "signing/code-plus-plus.key"
    $publicRel = Get-ReleaseEnvValue "CODE_PLUS_PLUS_SIGNING_PUBLIC_KEY_PATH" "signing/code-plus-plus.key.pub"
    return @{
        PrivateKey = Resolve-ReleasePath -Root $Root -PathValue $privateRel
        PublicKey = Resolve-ReleasePath -Root $Root -PathValue $publicRel
        Password = Get-ReleaseEnvValue "CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PASSWORD"
    }
}

function Resolve-MinisignExe {
    $cmd = Get-Command minisign -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    $candidates = @(
        (Join-Path $env:TEMP "minisign\minisign-win64\x86_64\minisign.exe")
    )
    if ($env:RUNNER_TEMP) {
        $candidates = @((Join-Path $env:RUNNER_TEMP "minisign.exe")) + $candidates
    }
    foreach ($path in $candidates) {
        if ($path -and (Test-Path $path)) { return $path }
    }

    throw "minisign not found. Install from https://github.com/jedisct1/minisign or run release workflow."
}

function Invoke-MinisignSign {
    param(
        [Parameter(Mandatory = $true)][string]$MinisignExe,
        [Parameter(Mandatory = $true)][string]$ArtifactPath,
        [Parameter(Mandatory = $true)][string]$SecretKeyPath,
        [Parameter(Mandatory = $true)][string]$SigPath,
        [string]$Password
    )
    if ($Password -and $Password.Length -gt 0) {
        $pwFile = Join-Path $env:TEMP "code-plus-plus-minisign-pw-$([Guid]::NewGuid().ToString('n')).txt"
        try {
            [System.IO.File]::WriteAllText($pwFile, $Password)
            $cmdLine = "type `"$pwFile`" | `"$MinisignExe`" -Sm `"$ArtifactPath`" -s `"$SecretKeyPath`" -x `"$SigPath`""
            cmd /c $cmdLine | Out-Null
            return $LASTEXITCODE
        }
        finally {
            Remove-Item $pwFile -Force -ErrorAction SilentlyContinue
        }
    }
    else {
        & $MinisignExe -Sm $ArtifactPath -s $SecretKeyPath -x $SigPath
    }
    return $LASTEXITCODE
}

Export-ModuleMember -Function Import-ReleaseDotEnv, Get-ReleaseEnvValue, Resolve-ReleasePath, Get-SigningKeyPaths, Resolve-MinisignExe, Invoke-MinisignSign
