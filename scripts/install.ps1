# AX installer for Windows — one-line install from GitHub Releases.
#
# Usage (PowerShell 5.1 compatible; plain `irm ... | iex` can fail on 5.1):
#   powershell -ExecutionPolicy Bypass -c "iex ((iwr 'https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1' -UseBasicParsing).Content)"
#
# Environment:
#   AX_VERSION       release tag to install (default: latest)
#   AX_INSTALL_DIR   install directory (default: $env:LOCALAPPDATA\Programs\AX\bin)
#   AX_HOME          AX state directory (default: <install directory>\.ax)
#
# The downloaded archive is verified against the release's SHA256SUMS before
# anything is written to disk. The install directory is added to the current
# user's PATH only if it is not already there. The archive also carries the
# bundled skill packages and an example MCP config, which are placed in
# AX_HOME; existing skills and an existing mcp.toml are never overwritten.

$ErrorActionPreference = "Stop"

$repo = "Axium-Labs/AX"
$githubApi = "https://api.github.com/repos/$repo/releases"
$gitcodeRepo = $env:AX_GITCODE_REPOSITORY
if ($gitcodeRepo -and $gitcodeRepo -notmatch '^[A-Za-z0-9_-]+/[A-Za-z0-9_-]+$') { throw "AX: AX_GITCODE_REPOSITORY must be owner/repository" }

# --- detect architecture ------------------------------------------------------
$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    "X64" { "x86_64" }
    "Arm64" { "aarch64" }
    default { throw "AX: unsupported architecture: $($_)" }
}

# --- resolve version ------------------------------------------------------------
$requestedVersion = if ($env:AX_VERSION) { $env:AX_VERSION } else { "latest" }
$version = $requestedVersion
if ($version -ne "latest" -and -not $version.StartsWith("v")) { $version = "v$version" }
if ($version -eq "latest") {
    try { $version = (Invoke-RestMethod -Uri "$githubApi/latest" -TimeoutSec 12 -Headers @{ "User-Agent" = "AX-installer" }).tag_name }
    catch {
        if (-not $gitcodeRepo) { throw "AX: GitHub could not resolve latest version and AX_GITCODE_REPOSITORY is unset" }
        $version = (Invoke-RestMethod -Uri "https://api.gitcode.com/api/v5/repos/$gitcodeRepo/releases/latest" -TimeoutSec 12).tag_name
    }
}
if (-not $version -or $version -notmatch '^v?[0-9]+(\.[0-9]+)+(?:[-+][A-Za-z0-9.-]+)?$') { throw "AX: invalid release version: $version" }

$asset = "ax-$arch-pc-windows-msvc.zip"

# --- install location ------------------------------------------------------------
$installDir = if ($env:AX_INSTALL_DIR) {
    $env:AX_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA "Programs\AX\bin"
}
$installDir = $installDir.TrimEnd('\')
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$axPath = Join-Path $installDir "ax.exe"

# --- download and verify ----------------------------------------------------------
$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "ax-install-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null
try {
    $zipPath = Join-Path $tmpDir "ax.zip"

    $sumsPath = Join-Path $tmpDir "SHA256SUMS"
    $providers = @(@{ Name = "GitHub"; Api = "$githubApi/tags/$version" })
    if ($gitcodeRepo) { $providers += @{ Name = "GitCode"; Api = "https://api.gitcode.com/api/v5/repos/$gitcodeRepo/releases/tags/$version" } }
    $verified = $false
    $failures = @()
    foreach ($provider in $providers) {
        try {
            $release = Invoke-RestMethod -Uri $provider.Api -TimeoutSec 12 -Headers @{ "User-Agent" = "AX-installer" }
            if ($release.tag_name -ne $version) { throw "release tag mismatch" }
            $assetUrl = ($release.assets | Where-Object name -eq $asset | Select-Object -First 1).browser_download_url
            $sumsUrl = ($release.assets | Where-Object name -eq "SHA256SUMS" | Select-Object -First 1).browser_download_url
            if (-not $assetUrl -or -not $sumsUrl) { throw "release is missing installer assets" }
            Write-Host "AX: downloading verified assets from $($provider.Name)"
            Invoke-WebRequest -Uri $assetUrl -OutFile $zipPath -UseBasicParsing -TimeoutSec 180
            Invoke-WebRequest -Uri $sumsUrl -OutFile $sumsPath -UseBasicParsing -TimeoutSec 30
            $sumsText = Get-Content -Path $sumsPath -Raw
            $expected = @($sumsText -split "`r?`n" | Where-Object { $_ -match "^\s*[a-fA-F0-9]{64}\s+\*?$([regex]::Escape($asset))\s*$" } | ForEach-Object { ($_ -split "[ \t]+")[0] })
            if ($expected.Count -ne 1) { throw "SHA256SUMS has a missing, malformed, or duplicate entry for $asset" }
            $actual = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -ne $expected[0].ToLowerInvariant()) { throw "SHA256 mismatch" }
            $verified = $true
            break
        } catch {
            $failures += "$($provider.Name): $($_.Exception.Message)"
            Remove-Item -LiteralPath $zipPath, $sumsPath -Force -ErrorAction SilentlyContinue
        }
    }
    if (-not $verified) { throw "AX: all release sources failed: $($failures -join '; ')" }

    # --- extract and install -------------------------------------------------------
    Expand-Archive -Path $zipPath -DestinationPath $tmpDir -Force
    # Running AX/Crew processes can keep ax.exe open. Rename the old image rather
    # than overwrite it; existing processes finish using that image, and a failed
    # copy restores the original path. Keep a locked backup until it can be removed.
    $backupPath = "$axPath.install-backup-$([guid]::NewGuid().ToString('N'))"
    $hadExisting = Test-Path -LiteralPath $axPath
    if ($hadExisting) { Move-Item -LiteralPath $axPath -Destination $backupPath }
    try {
        Copy-Item -LiteralPath (Join-Path $tmpDir "ax.exe") -Destination $axPath
    } catch {
        if (Test-Path -LiteralPath $axPath) { Remove-Item -LiteralPath $axPath -Force }
        if ($hadExisting) { Move-Item -LiteralPath $backupPath -Destination $axPath }
        throw
    }
    if ($hadExisting) {
        Remove-Item -LiteralPath $backupPath -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $backupPath) {
            Write-Host "AX: old executable remains at $backupPath until running tasks exit."
        }
    }
    Write-Host "AX: installed to $axPath"

    # --- bundled skills and MCP template --------------------------------------------
    # The archive ships the repository's skill packages and an example MCP config.
    # Skill packages that already exist are left untouched so local edits survive an
    # upgrade; the MCP template is only written when no config exists yet, and every
    # server in it is disabled so nothing tries to launch a missing command.
    $axHome = if ($env:AX_HOME) { $env:AX_HOME } else { Join-Path $installDir ".ax" }

    $bundledSkills = Join-Path $tmpDir "skills"
    if (Test-Path -Path $bundledSkills) {
        $skillsDir = Join-Path $axHome "skills"
        New-Item -ItemType Directory -Force -Path $skillsDir | Out-Null
        Get-ChildItem -Path $bundledSkills -Directory | ForEach-Object {
            $target = Join-Path $skillsDir $_.Name
            if (Test-Path -Path $target) {
                Write-Host "AX: keeping existing skill $($_.Name)"
            }
            else {
                Copy-Item -Path $_.FullName -Destination $target -Recurse
                Write-Host "AX: installed skill $($_.Name) to $target"
            }
        }
    }

    $bundledConfig = Join-Path $tmpDir "mcp.example.toml"
    if (Test-Path -Path $bundledConfig) {
        $mcpConfig = Join-Path $axHome "mcp.toml"
        if (Test-Path -Path $mcpConfig) {
            Write-Host "AX: keeping existing MCP config $mcpConfig"
        }
        else {
            New-Item -ItemType Directory -Force -Path $axHome | Out-Null
            Copy-Item -Path $bundledConfig -Destination $mcpConfig
            Write-Host "AX: wrote example MCP config to $mcpConfig (all servers disabled)"
        }
    }
}
finally {
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
}

# --- add to user PATH (avoid duplicates) -------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$userEntries = @($userPath -split ';' | ForEach-Object { $_.TrimEnd('\') })
if ($userEntries -notcontains $installDir) {
    $newPath = if ($userPath) { "$userPath;$installDir" } else { $installDir }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = "$env:Path;$installDir"
    Write-Host "AX: added $installDir to your user PATH (open a new terminal to use it)"
} else {
    Write-Host "AX: $installDir is already on your PATH"
}

# --- verify -------------------------------------------------------------------------
& $axPath --version
