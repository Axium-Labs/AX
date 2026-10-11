# AX incremental build-cache cleaner
# Usage:
#   .\scripts\clean-target.ps1         # remove debug/incremental only, keep deps
#   .\scripts\clean-target.ps1 -All    # also remove debug/deps, full rebuild next time
param(
    [switch]$All
)

$ErrorActionPreference = 'Stop'
$target = Join-Path $PSScriptRoot '..\target\debug'
$targets = @()

$inc = Join-Path $target 'incremental'
if (Test-Path $inc) { $targets += $inc }
if ($All) {
    $deps = Join-Path $target 'deps'
    if (Test-Path $deps) { $targets += $deps }
}

if ($targets.Count -eq 0) {
    Write-Host 'Nothing to clean (debug/incremental already gone).' -ForegroundColor Green
    exit 0
}

foreach ($t in $targets) {
    $size = (Get-ChildItem $t -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
    $gb = [math]::Round($size / 1GB, 2)
    Write-Host ("Deleting [{0}] ({1} GB) ..." -f $t, $gb) -ForegroundColor Yellow
    cmd /c rd /s /q "$t"
    if (Test-Path $t) { Remove-Item $t -Recurse -Force }
    Write-Host "  Done, freed $gb GB" -ForegroundColor Green
}

Write-Host 'Finished. Incremental cache cleaned; deps kept for faster next build.' -ForegroundColor Green
