<#
Install WFD Control (the Rust app) for the current user and start it at login.

    powershell -ExecutionPolicy Bypass -File wfd-app\install.ps1 [-NoStart]

Works from the repo (builds the exe) or from the unzipped release download (uses the
wfd-app.exe next to this script; no Rust needed).

- in the repo: builds the release exe (cargo build --release)
- copies it to %LOCALAPPDATA%\WFD Clock\wfd-app.exe (stops a running copy first)
- first install only: copies your settings from wfd-control\wfd_config.json if it exists
- adds a "WFD Clock" shortcut to your Startup folder (runs wfd-app.exe --no-browser)
- turns off the old Python app's login script by renaming it to *.vbs.disabled
- starts the app (unless -NoStart)

Undo: delete the "WFD Clock" shortcut from shell:startup and rename the .vbs.disabled back.
#>
param([switch]$NoStart)
$ErrorActionPreference = 'Stop'

$crate   = $PSScriptRoot
$repo    = Split-Path $crate -Parent
$dest    = Join-Path $env:LOCALAPPDATA 'WFD Clock'
$exe     = Join-Path $dest 'wfd-app.exe'
$config  = Join-Path $dest 'wfd_config.json'
$startup = [Environment]::GetFolderPath('Startup')

if (Test-Path (Join-Path $crate 'Cargo.toml')) {
    Write-Host "Building..."
    & cargo build --release --manifest-path (Join-Path $crate 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    $built = Join-Path $crate 'target\release\wfd-app.exe'
} else {
    $built = Join-Path $crate 'wfd-app.exe'                     # the release download
    if (-not (Test-Path $built)) { throw "wfd-app.exe isn't next to install.ps1" }
}

# a running copy (or the Python app) holds the clock's COM port and port 8737
Get-Process wfd-app -ErrorAction SilentlyContinue | Stop-Process -Force
$listener = Get-NetTCPConnection -LocalPort 8737 -State Listen -ErrorAction SilentlyContinue
if ($listener) { Stop-Process -Id $listener.OwningProcess -Force }
Start-Sleep -Milliseconds 500

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item $built $exe -Force
Write-Host "Installed $exe"

$pyConfig = Join-Path $repo 'wfd-control\wfd_config.json'
if (-not (Test-Path $config) -and (Test-Path $pyConfig)) {
    Copy-Item $pyConfig $config
    Write-Host "Copied your settings from $pyConfig"
}

$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut((Join-Path $startup 'WFD Clock.lnk'))
$lnk.TargetPath = $exe
$lnk.Arguments = '--no-browser'
$lnk.WorkingDirectory = $dest
$lnk.Description = 'WFD Control: drives the WFD clock'
$lnk.Save()
Write-Host "Login shortcut: $(Join-Path $startup 'WFD Clock.lnk')"

Get-ChildItem $startup -Filter '*.vbs' | Where-Object {
    (Get-Content $_.FullName -Raw) -match 'wfd-control\\app\.py|wfd_daemon'
} | ForEach-Object {
    Rename-Item $_.FullName ($_.Name + '.disabled')
    Write-Host "Turned off the old Python login script: $($_.Name) -> $($_.Name).disabled"
}

if (-not $NoStart) {
    Start-Process $exe -ArgumentList '--no-browser' -WorkingDirectory $dest
    Write-Host "Started. Dashboard: http://127.0.0.1:8737"
}
