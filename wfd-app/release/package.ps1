<#
Build the release files into dist\ (used by .github/workflows/release.yml, works locally too).

    powershell -ExecutionPolicy Bypass -File wfd-app\release\package.ps1 [-Tag v1.0.0] [-NoBuild]

dist\wfd-app-<tag>-windows-x64.zip   wfd-app.exe + wfd-flash.exe + install.ps1 + README.txt
dist\wfd-clock-firmware.bin          OTA image   } copied from wfd-firmware\releases\
dist\wfd-clock-merged.bin            USB image   }
dist\notes.md                        release notes header

The tag must match the version in wfd-app\Cargo.toml (v1.0.0 <-> 1.0.0).
#>
param([string]$Tag = '', [switch]$NoBuild)
$ErrorActionPreference = 'Stop'

$crate = Split-Path $PSScriptRoot -Parent
$repo  = Split-Path $crate -Parent
$dist  = Join-Path $repo 'dist'

if (-not $NoBuild) {
    & cargo build --release --manifest-path (Join-Path $crate 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
$exe = Join-Path $crate 'target\release\wfd-app.exe'
$flash = Join-Path $crate 'target\release\wfd-flash.exe'

# the exe says its own version and the firmware it ships
$about = (& $exe --version | Out-String).Trim()
if ($about -notmatch '^wfd-app (\S+), ships (.+)$') { throw "unexpected --version output: $about" }
$version, $fw = $Matches[1], $Matches[2]
if (-not $Tag) { $Tag = "v$version" }
if ($Tag -ne "v$version") { throw "tag $Tag doesn't match Cargo.toml version $version" }

Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
$stage = Join-Path $dist "wfd-app-$Tag-windows-x64"
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe, $flash, (Join-Path $crate 'install.ps1'), (Join-Path $PSScriptRoot 'README.txt') $stage
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath "$stage.zip"
Remove-Item $stage -Recurse -Force

$fwDir = Join-Path $repo 'wfd-firmware\releases'
Copy-Item (Join-Path $fwDir 'wfd-clock-firmware.bin'), (Join-Path $fwDir 'wfd-clock-merged.bin') $dist

@"
**PC app $version** for Windows 10/11 (x64). Ships clock firmware **$fw**.

- ``wfd-app-$Tag-windows-x64.zip``: unzip, then
  - **first time:** plug the clock into a USB 2.0 port and run ``wfd-flash.exe``. It backs up the clock, then flashes it.
  - **the PC app:** run ``install.ps1`` (starts at login). No Rust or other runtime needed.
- ``wfd-clock-merged.bin``: the full USB image, for esptool at ``0x0`` (``wfd-flash.exe`` has it built in).
- ``wfd-clock-firmware.bin``: OTA update. The app's **Update clock** button does this for you.

See the [README](https://github.com/bstag/elekswfd-custom-firmware#quick-start) to get started.
"@ | Set-Content (Join-Path $dist 'notes.md') -Encoding utf8

Get-ChildItem $dist | ForEach-Object { '{0,-40} {1,8:N0} KB' -f $_.Name, ($_.Length / 1KB) }
