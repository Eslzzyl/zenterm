#!/usr/bin/env pwsh
# Bump the workspace version, update Cargo.lock, and create a release tag.
#
# Usage: .\scripts\bump-version.ps1 0.2.0
#
# Requirements: cargo-edit (for `cargo set-version`).

$ErrorActionPreference = 'Stop'

if ($args.Count -ne 1) {
    Write-Host "Usage: $($MyInvocation.MyCommand.Name) <new-version>"
    Write-Host "  e.g. $($MyInvocation.MyCommand.Name) 0.2.0"
    exit 1
}

$NewVersion = $args[0]
if ($NewVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$') {
    Write-Error "Version must be in semver format (e.g. 0.2.0, 0.2.0-beta.1)"
    exit 1
}

$RootDir = Split-Path -Parent $PSScriptRoot
Set-Location $RootDir

if (git status --short) {
    Write-Error "Working tree is not clean; commit or stash changes first."
    exit 1
}

if (-not (Get-Command cargo-set-version -ErrorAction SilentlyContinue)) {
    Write-Error "cargo-edit is required. Install it with: cargo install cargo-edit --locked"
    exit 1
}

Write-Host "==> Bumping workspace version to $NewVersion ..."
cargo set-version --workspace $NewVersion
if ($LASTEXITCODE -ne 0) { throw "cargo set-version failed" }

Write-Host ""
Write-Host "==> Regenerating Cargo.lock ..."
cargo generate-lockfile
if ($LASTEXITCODE -ne 0) { throw "cargo generate-lockfile failed" }

Write-Host ""
Write-Host "==> Verifying workspace metadata ..."
cargo metadata --no-deps --format-version 1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }

Write-Host ""
Write-Host "==> Staging version changes ..."
$ManifestFiles = Get-ChildItem -Path crates -Filter Cargo.toml -Recurse | ForEach-Object { $_.FullName }
git add -- Cargo.toml Cargo.lock $ManifestFiles
if ($LASTEXITCODE -ne 0) { throw "git add failed" }

Write-Host ""
Write-Host "==> Creating commit and tag ..."
git commit -m "chore: bump version to $NewVersion"
if ($LASTEXITCODE -ne 0) { throw "git commit failed" }
git tag -a "v$NewVersion" -m "Zenterm v$NewVersion"
if ($LASTEXITCODE -ne 0) { throw "git tag failed" }

$Head = git rev-parse HEAD
Write-Host ""
Write-Host "============================================"
Write-Host "  Version bumped to $NewVersion"
Write-Host "  Commit : $Head"
Write-Host "  Tag    : v$NewVersion"
Write-Host "============================================"
Write-Host ""
Write-Host "Next step — push to remote:"
Write-Host "  git push origin master --follow-tags"
