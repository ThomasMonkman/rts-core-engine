# Puts rts-core-engine in place of a Forged Alliance install's bin\SupremeCommander.exe,
# so whatever launches the game (LOUD's LOVE-LOUD.exe, a shortcut with /init) runs this
# engine instead. The original is kept as bin\SupremeCommander.original.exe; -Restore puts
# it back. Steam's "Verify integrity of game files" also restores it. See docs/guides/packaging.md.
#
#   powershell -ExecutionPolicy Bypass -File dev\package\install-as-supcom.ps1 -GameDir <FA> [-Exe <rts-core-engine.exe>]
#   powershell -ExecutionPolicy Bypass -File dev\package\install-as-supcom.ps1 -GameDir <FA> -Restore
param(
    [Parameter(Mandatory = $true)][string]$GameDir,
    [string]$Exe,
    [switch]$Restore
)
$ErrorActionPreference = "Stop"
# Not a param default: Windows PowerShell 5.1 leaves $PSScriptRoot empty there.
if (-not $Exe) { $Exe = "$PSScriptRoot\..\..\target\package\release\rts-core-engine.exe" }
$target = Join-Path $GameDir "bin\SupremeCommander.exe"
$backup = Join-Path $GameDir "bin\SupremeCommander.original.exe"
if (-not (Test-Path (Join-Path $GameDir "gamedata\units.scd"))) { throw "$GameDir isn't a Forged Alliance folder (no gamedata\units.scd)" }

if ($Restore) {
    if (-not (Test-Path $backup)) { throw "no $backup to restore" }
    Copy-Item $backup $target -Force
    Remove-Item $backup
    Write-Output "restored the original $target"
    exit 0
}

if (-not (Test-Path $Exe)) { throw "no $Exe (build it with: `$env:CARGO_TARGET_DIR='target\package'; `$env:RUSTFLAGS='-C target-feature=+crt-static'; cargo build --release)" }
# Back up the original once: a second install must not overwrite it with ours.
if (-not (Test-Path $backup)) {
    Copy-Item $target $backup
    Write-Output "kept the original as $backup"
}
Copy-Item $Exe $target -Force
Write-Output "installed $Exe as $target"
