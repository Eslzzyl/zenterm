<#
.SYNOPSIS
    Build and package Zenterm for Windows.

.DESCRIPTION
    Runs cargo-packager to produce a Windows installer (NSIS .exe or MSI .msi).

.PARAMETER Debug
    Build and package debug binaries instead of release.

.PARAMETER Format
    Package format(s) to produce (e.g. "nsis", "wix", "all", "default").

.EXAMPLE
    .\scripts\package.ps1
    .\scripts\package.ps1 -Debug
    .\scripts\package.ps1 -Format nsis
#>

param(
    [switch]$Debug,
    [string[]]$Format
)

$ErrorActionPreference = "Stop"
$ProjectDir = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $ProjectDir

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Zenterm Packager" -ForegroundColor Cyan
Write-Host " Platform : windows" -ForegroundColor Cyan
Write-Host " Profile  : $(if ($Debug) { 'debug' } else { 'release' })" -ForegroundColor Cyan
if ($Format) {
    Write-Host " Formats  : $($Format -join ', ')" -ForegroundColor Cyan
} else {
    Write-Host " Formats  : (platform default)" -ForegroundColor Cyan
}
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

$PackagerArgs = @("-p", "zenterm")

if (-not $Debug) {
    $PackagerArgs += "--release"
}

if ($Format) {
    $PackagerArgs += "--formats"
    $PackagerArgs += ($Format -join ",")
}

Write-Host "→ Running: cargo packager $($PackagerArgs -join ' ')" -ForegroundColor Green
Write-Host ""

cargo packager @PackagerArgs

if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo-packager failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host ""
Write-Host "✔ Done! Packages are in the output directory." -ForegroundColor Green
