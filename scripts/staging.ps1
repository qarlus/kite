<#
.SYNOPSIS
Runs the current checkout against a copy of your real Kite data, so a change meets real
Proton and Outlook mail before it is pushed.

.DESCRIPTION
Builds `kite` with the `perf` profile, copies %APPDATA%\Kite into a staging folder, and starts
the build with APPDATA pointed at that copy. Your real database and settings are never written.

The accounts are real: staging uses the same sign-ins from Windows Credential Manager, so it
syncs live mail, and archiving, sending or deleting there acts on your real mailboxes. Close
your everyday Kite first: both copies refreshing the same Proton session can sign one out.

.PARAMETER Keep
Reuse the existing staging copy instead of taking a fresh one.

.PARAMETER Folder
Where the staging copy lives. Defaults to %LOCALAPPDATA%\Kite-staging.
#>
param(
    [switch]$Keep,
    [string]$Folder = (Join-Path $env:LOCALAPPDATA 'Kite-staging')
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$live = Join-Path $env:APPDATA 'Kite'
$copy = Join-Path $Folder 'Kite'

$running = Get-Process kite -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and -not $_.Path.StartsWith($root) -and -not $_.Path.Contains('cargo-target') }
if ($running) {
    throw "Kite is running ($($running[0].Path)). Close it first; staging shares its sign-ins."
}

cargo build --profile perf -p kite --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE) { throw 'Build failed' }
$target = (cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $root 'Cargo.toml') |
    ConvertFrom-Json).target_directory
$exe = Join-Path $target 'perf\kite.exe'

if (-not $Keep -or -not (Test-Path $copy)) {
    if (Test-Path $copy) { Remove-Item $copy -Recurse -Force }
    New-Item -ItemType Directory -Force $copy | Out-Null
    if (Test-Path $live) { Copy-Item (Join-Path $live '*') $copy -Recurse -Force }
    Write-Host "Copied $live to $copy"
}

$env:APPDATA = $Folder
Write-Host "Starting $exe with data in $copy (live accounts: actions are real)"
& $exe
