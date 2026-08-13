<#
.SYNOPSIS
    Build and package Zenterm for Windows.

.DESCRIPTION
    Runs cargo-packager to produce a Windows installer (NSIS .exe or MSI .msi).

.PARAMETER Debug
    Build and package debug binaries instead of release.

.PARAMETER Format
    Package format(s) to produce (e.g. "nsis", "wix", "all", "default").

.PARAMETER Target
    Rust target triple to build (defaults to the host target).

.EXAMPLE
    .\scripts\package.ps1
    .\scripts\package.ps1 -Debug
    .\scripts\package.ps1 -Format nsis
#>

param(
    [switch]$Debug,
    [string[]]$Format,
    [string]$Target
)

$ErrorActionPreference = "Stop"
$ProjectDir = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectDir

if (-not $Target) {
    $Target = (rustc -vV | Select-String '^host:' | ForEach-Object {
        $_.Line.Split(':', 2)[1].Trim()
    })
}

$Profile = if ($Debug) { "debug" } else { "release" }
$BinaryDir = Join-Path "target" (Join-Path $Target $Profile)

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Zenterm Packager" -ForegroundColor Cyan
Write-Host " Platform : windows" -ForegroundColor Cyan
Write-Host " Profile  : $(if ($Debug) { 'debug' } else { 'release' })" -ForegroundColor Cyan
Write-Host " Target   : $Target" -ForegroundColor Cyan
if ($Format) {
    Write-Host " Formats  : $($Format -join ', ')" -ForegroundColor Cyan
} else {
    Write-Host " Formats  : (platform default)" -ForegroundColor Cyan
}
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

$BuildArgs = @("--package", "zenterm", "--locked", "--target", $Target)
$PackagerArgs = @(
    "--packages", "zenterm",
    "--target", $Target,
    "--out-dir", "dist",
    "--binaries-dir", $BinaryDir
)

if (-not $Debug) {
    $BuildArgs += "--release"
    $PackagerArgs += "--release"
}

if ($Format) {
    $PackagerArgs += "--formats"
    $PackagerArgs += ($Format -join ",")
}

Write-Host "→ Running: cargo build $($BuildArgs -join ' ')" -ForegroundColor Green
Write-Host ""

cargo build @BuildArgs
if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo build failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
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
