# AX installer for Windows — one-line install from GitHub Releases.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1 | iex"
#
# Environment:
#   AX_VERSION       release tag to install (default: latest, e.g. v0.1.0)
#   AX_INSTALL_DIR   install directory (default: $env:LOCALAPPDATA\Programs\AX\bin)
#
# The downloaded archive is verified against the release's SHA256SUMS before
# anything is written to disk. The install directory is added to the current
# user's PATH only if it is not already there.

$ErrorActionPreference = "Stop"

$repo = "Axium-Labs/AX"
$baseUrl = "https://github.com/$repo/releases"

# --- detect architecture ------------------------------------------------------
$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    "X64" { "x86_64" }
    "Arm64" { "aarch64" }
    default { throw "AX: unsupported architecture: $($_)" }
}

# --- resolve version ------------------------------------------------------------
$version = if ($env:AX_VERSION) { $env:AX_VERSION } else { "latest" }
if ($version -ne "latest" -and -not $version.StartsWith("v")) {
    $version = "v$version"
}
$downloadBase = if ($version -eq "latest") {
    "$baseUrl/latest/download"
} else {
    "$baseUrl/download/$version"
}

$asset = "ax-$arch-pc-windows-msvc.zip"
$assetUrl = "$downloadBase/$asset"
$sumsUrl = "$downloadBase/SHA256SUMS"

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

    Write-Host "AX: downloading $assetUrl"
    Invoke-WebRequest -Uri $assetUrl -OutFile $zipPath -UseBasicParsing

    # Download SHA256SUMS to disk and read it as text. In Windows PowerShell 5.1,
    # (Invoke-WebRequest ...).Content is a byte[] for non-text content types,
    # which makes line splitting fail; Get-Content -Raw always yields a string.
    $sumsPath = Join-Path $tmpDir "SHA256SUMS"
    Write-Host "AX: downloading $sumsUrl"
    Invoke-WebRequest -Uri $sumsUrl -OutFile $sumsPath -UseBasicParsing
    $sumsText = Get-Content -Path $sumsPath -Raw

    $expected = @($sumsText -split "`r?`n" |
        Where-Object { $_ -match "[ \t]+$([regex]::Escape($asset))[ \t]*$" } |
        ForEach-Object { ($_ -split "[ \t]+")[0] } |
        Select-Object -First 1)

    if (-not $expected) {
        $known = @($sumsText -split "`r?`n" |
            Where-Object { $_ -match "[ \t]+[^ \t]+[ \t]*$" } |
            ForEach-Object { ($_ -split "[ \t]+")[-1].Trim("`r") }) -join ", "
        throw "AX: $asset is missing from SHA256SUMS (SHA256SUMS contains: $known)"
    }

    $actual = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected.ToLowerInvariant()) {
        throw "AX: checksum mismatch`n  expected: $expected`n  actual:   $actual"
    }

    # --- extract and install -------------------------------------------------------
    Expand-Archive -Path $zipPath -DestinationPath $tmpDir -Force
    Copy-Item -Path (Join-Path $tmpDir "ax.exe") -Destination $axPath -Force
    Write-Host "AX: installed to $axPath"
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
