<#
.SYNOPSIS
  Copies the Rust library of the Velopack fork (pre-launch update window included) into
  src-tauri/vendor/velopack.

.DESCRIPTION
  FlowCapture builds against a copy of the fork's `src/lib-rust`, so the build (and the GitHub CI)
  does not need the CSDigitalOrder repository. Run this script again after changing the library
  in the fork.

  Only the sources are copied: src-tauri/vendor/velopack/Cargo.toml is maintained by hand because
  the fork's manifest inherits its dependency versions from the Velopack workspace. When the fork's
  manifest changes, the script stops and shows what to update.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\sync-velopack.ps1
  powershell -ExecutionPolicy Bypass -File scripts\sync-velopack.ps1 -VelopackRoot D:\velopack
#>
param(
    [string]$VelopackRoot = "C:\Pro\CSDigitalOrder\Velopack"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $VelopackRoot "src\lib-rust"
$target = Join-Path $repoRoot "src-tauri\vendor\velopack"

if (-not (Test-Path (Join-Path $source "src\lib.rs"))) {
    throw "Libreria Rust di Velopack non trovata in '$source'. Indica la cartella del fork con -VelopackRoot."
}

# The fork's manifest as it was at the last sync: if it changed, the hand-written Cargo.toml may
# need new dependencies or versions.
$upstreamManifest = Join-Path $source "Cargo.toml"
$knownManifest = Join-Path $target "Cargo.upstream.toml"
if ((Test-Path $knownManifest) -and
    ((Get-Content $upstreamManifest -Raw) -ne (Get-Content $knownManifest -Raw))) {
    Write-Host "Il Cargo.toml della libreria nel fork e' cambiato dall'ultima sincronizzazione:" -ForegroundColor Yellow
    Compare-Object (Get-Content $knownManifest) (Get-Content $upstreamManifest) | Format-Table -AutoSize | Out-String | Write-Host
    Write-Host "Aggiorna a mano src-tauri\vendor\velopack\Cargo.toml (versioni in [workspace.dependencies] di $VelopackRoot\Cargo.toml)," -ForegroundColor Yellow
    Write-Host "poi copia il nuovo manifest in $knownManifest e rilancia lo script." -ForegroundColor Yellow
    exit 1
}

$targetSrc = Join-Path $target "src"
if (Test-Path $targetSrc) {
    Remove-Item $targetSrc -Recurse -Force
}
New-Item -ItemType Directory -Force $target | Out-Null
Copy-Item (Join-Path $source "src") $targetSrc -Recurse
Copy-Item $upstreamManifest $knownManifest -Force
Copy-Item (Join-Path $VelopackRoot "LICENSE") (Join-Path $target "LICENSE") -Force

$commit = ""
try {
    $commit = (git -C $VelopackRoot rev-parse --short HEAD 2>$null)
} catch {
}
$stamp = "Sincronizzato da $VelopackRoot"
if ($commit) {
    $stamp += " (commit $commit)"
}
$stamp += " il $(Get-Date -Format 'yyyy-MM-dd HH:mm')."
Set-Content -Path (Join-Path $target "SYNCED_FROM.txt") -Value $stamp -Encoding UTF8

Write-Host "Libreria Velopack copiata in $target" -ForegroundColor Green
Write-Host $stamp
