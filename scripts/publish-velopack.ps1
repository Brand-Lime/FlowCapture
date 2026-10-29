<#
.SYNOPSIS
  Builds FlowCapture for Windows and publishes it with Velopack (setup + auto-update).

.DESCRIPTION
  1. Builds the app with `tauri build --no-bundle` (no NSIS/MSI: Velopack is the installer),
     embedding the update feed (-UpdateSource) in the exe.
  2. Collects exe, ffmpeg and the browser extension in a clean folder.
  3. Runs `vpk pack`: setup.exe, full/delta packages and releases.win.json go to -OutputDir.

  The app checks the feed at every start (window "FlowCapture - Controllo Aggiornamenti", see
  src-tauri/src/updater.rs). -UpdateSource must be the address the clients use to read -OutputDir:
  the web address that serves it, or the share itself (\\server\share\...).

.EXAMPLE
  .\publish.bat 1.0.1
  powershell -ExecutionPolicy Bypass -File scripts\publish-velopack.ps1 -Version 1.0.1
  powershell -ExecutionPolicy Bypass -File scripts\publish-velopack.ps1 -Version 1.0.1 -OutputDir .\Releases -UpdateSource \\srvdati\Ftp\Utilities\FlowCapture
#>
param(
    # Version of the release (SemVer, e.g. 1.0.1). Must be higher than the last published one.
    [string]$Version = "1.1.0",
    # Folder receiving setup and packages (also where vpk looks for the previous release for deltas).
    [string]$OutputDir = "\\srvdati\Ftp\Utilities\FlowCapture",
    # Update feed embedded in the exe: URL (http/https) or folder/share. Empty = no update check.
    [string]$UpdateSource = "\\srvdati\Ftp\Utilities\FlowCapture",
    # Sessions folder proposed on new installations (Impostazioni -> Cartella sessioni), embedded in
    # the exe. %USERNAME% (or any %VARIABLE%) is replaced on each computer, e.g.
    # \\server\FlowCapture\%USERNAME%. Empty = no default folder. Computers that already chose a
    # folder (or removed it) keep their choice.
    [string]$DefaultSessionDir = "\\srvdati\GenAuto13485\%USERNAME%",
    [string]$PackId = "FlowCapture",
    [string]$PackTitle = "FlowCapture",
    [string]$PackAuthors = "Univet",
    # Marks the release as mandatory: the app cannot start without installing it.
    [switch]$Mandatory,
    # Signing parameters for signtool (e.g. '/f cert.pfx /p ... /fd SHA256 /tr http://timestamp.digicert.com /td SHA256').
    [string]$SignParams = "",
    # Markdown file with the release notes.
    [string]$ReleaseNotes = "",
    # Reuses the exe built by the previous run (only repackages).
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

function Write-Step([string]$text) {
    Write-Host ""
    Write-Host "===================================================" -ForegroundColor Cyan
    Write-Host $text -ForegroundColor Cyan
    Write-Host "===================================================" -ForegroundColor Cyan
}

function Fail([string]$text) {
    Write-Host ""
    Write-Host "[ERRORE] $text" -ForegroundColor Red
    exit 1
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$tauriDir = Join-Path $repoRoot "src-tauri"
$workDir = Join-Path $tauriDir "target\velopack"
$stageDir = Join-Path $workDir "app"
$mainExe = "FlowCapture.exe"

# --- Version ------------------------------------------------------------------------------------
$tauriConf = Get-Content (Join-Path $tauriDir "tauri.conf.json") -Raw | ConvertFrom-Json
if (-not $Version) {
    $Version = $tauriConf.version
    Write-Host "Nessuna versione indicata, uso quella di tauri.conf.json: $Version" -ForegroundColor Yellow
    Write-Host "Uso: publish.bat [versione]   (es. publish.bat 1.0.1)"
}
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    Fail "Versione '$Version' non valida: usa il formato 1.2.3 (oppure 1.2.3-beta.1)."
}

# --- Prerequisites ------------------------------------------------------------------------------
foreach ($tool in @("npm", "cargo", "vpk")) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        switch ($tool) {
            "vpk" { Fail "vpk non trovato. Installa la CLI del fork: dotnet tool install -g vpk --add-source C:\Pro\CSDigitalOrder\Velopack\build\Release" }
            default { Fail "$tool non trovato nel PATH." }
        }
    }
}
$ffmpeg = Join-Path $tauriDir "binaries\ffmpeg-x86_64-pc-windows-msvc.exe"
if (-not (Test-Path $ffmpeg)) {
    Write-Host "ffmpeg non ancora scaricato: verra' scaricato dalla build." -ForegroundColor Yellow
}
if (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
    $OutputDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDir))
}
$outputParent = Split-Path -Parent $OutputDir
if (-not (Test-Path $outputParent)) {
    Fail "La cartella '$outputParent' non e' raggiungibile. Controlla la connessione di rete e i permessi."
}

Write-Host "Versione:          $Version"
Write-Host "Cartella rilascio: $OutputDir"
Write-Host "Feed aggiornamenti: $(if ($UpdateSource) { $UpdateSource } else { '(nessuno: aggiornamento automatico disattivato)' })"
Write-Host "Cartella sessioni predefinita: $(if ($DefaultSessionDir) { $DefaultSessionDir } else { '(nessuna)' })"

# --- 1. Build -----------------------------------------------------------------------------------
$builtExe = Join-Path $tauriDir "target\release\flowcapture.exe"
if ($SkipBuild) {
    if (-not (Test-Path $builtExe)) { Fail "-SkipBuild indicato ma $builtExe non esiste." }
} else {
    Write-Step "1. Build di FlowCapture v$Version (tauri build --no-bundle)"
    New-Item -ItemType Directory -Force $workDir | Out-Null
    # Version of this build, merged into tauri.conf.json without changing it.
    $versionConfig = Join-Path $workDir "tauri.version.json"
    Set-Content -Path $versionConfig -Value (@{ version = $Version } | ConvertTo-Json) -Encoding UTF8

    Push-Location $repoRoot
    try {
        if (-not (Test-Path (Join-Path $repoRoot "node_modules"))) {
            npm ci
            if ($LASTEXITCODE -ne 0) { Fail "npm ci non riuscito." }
        }
        $env:FLOWCAPTURE_UPDATE_SOURCE = $UpdateSource
        $env:FLOWCAPTURE_DEFAULT_SESSION_DIR = $DefaultSessionDir
        npm run tauri -- build --no-bundle --config "$versionConfig"
        if ($LASTEXITCODE -ne 0) { Fail "Build non riuscita." }
    } finally {
        Remove-Item Env:FLOWCAPTURE_UPDATE_SOURCE -ErrorAction SilentlyContinue
        Remove-Item Env:FLOWCAPTURE_DEFAULT_SESSION_DIR -ErrorAction SilentlyContinue
        Pop-Location
    }
}

# --- 2. Application folder ----------------------------------------------------------------------
Write-Step "2. Preparazione della cartella dell'applicazione"
if (Test-Path $stageDir) { Remove-Item $stageDir -Recurse -Force }
New-Item -ItemType Directory -Force $stageDir | Out-Null
Copy-Item $builtExe (Join-Path $stageDir $mainExe)
if (-not (Test-Path $ffmpeg)) { Fail "ffmpeg non trovato in $ffmpeg (serve per i video delle sessioni)." }
Copy-Item $ffmpeg (Join-Path $stageDir "ffmpeg.exe")
Copy-Item (Join-Path $repoRoot "browser-extension") (Join-Path $stageDir "browser-extension") -Recurse
Get-ChildItem $stageDir -Recurse -File | ForEach-Object {
    Write-Host ("  {0,-50} {1,10:N0} KB" -f $_.FullName.Substring($stageDir.Length + 1), ($_.Length / 1KB))
}

# --- 3. vpk pack --------------------------------------------------------------------------------
Write-Step "3. Creazione del pacchetto Velopack (vpk pack) v$Version"
$packArgs = @(
    "pack",
    "--packId", $PackId,
    "--packVersion", $Version,
    "--packDir", $stageDir,
    "--mainExe", $mainExe,
    "--packTitle", $PackTitle,
    "--packAuthors", $PackAuthors,
    "--outputDir", $OutputDir,
    "--runtime", "win-x64",
    "--icon", (Join-Path $tauriDir "icons\icon.ico"),
    # The UI is a WebView2 page: setup installs the runtime when it is missing (old Windows 10).
    "--framework", "webview2",
    "--noPortable"
)
if ($Mandatory) { $packArgs += "--mandatory" }
if ($ReleaseNotes) { $packArgs += @("--releaseNotes", $ReleaseNotes) }
if ($SignParams) { $packArgs += @("--signParams", $SignParams) }

& vpk @packArgs
if ($LASTEXITCODE -ne 0) {
    Fail "Creazione pacchetto Velopack fallita! Controlla l'accesso alla cartella di rete, la CLI vpk e che la versione $Version sia maggiore dell'ultima pubblicata."
}

# --- 4. Browser extension packages (for IT: stores, group policies) -------------------------------
Write-Step "4. Pacchetti dell'estensione browser (Chrome, Edge, Brave, Opera, Firefox)"
node (Join-Path $repoRoot "scripts\build-browser-extensions.mjs")
if ($LASTEXITCODE -ne 0) { Fail "Creazione dei pacchetti dell'estensione browser fallita." }
$extensionsOut = Join-Path $OutputDir "Estensione browser"
New-Item -ItemType Directory -Force $extensionsOut | Out-Null
Copy-Item (Join-Path $repoRoot "dist-extensions\*.zip") $extensionsOut -Force
Copy-Item (Join-Path $repoRoot "docs\estensione-browser-IT.md") $extensionsOut -Force

Write-Step "[SUCCESS] FlowCapture v$Version pubblicato in '$OutputDir'"
Write-Host "Installazione per i nuovi utenti: $OutputDir\$PackId-win-Setup.exe"
Write-Host "Estensione browser per il team IT:  $extensionsOut"
exit 0
