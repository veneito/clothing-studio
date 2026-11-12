# Builds target\release\Clothing.Studio_<version>_x64-setup.exe from an existing release build.
# Used by the GitHub workflows; also works locally if NSIS is installed.
param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$rel = Join-Path $root 'target\release'
if (-not (Test-Path (Join-Path $rel 'clothing-studio.exe'))) { throw 'Run "cargo build --release" first.' }

# App-local Microsoft C++ runtime (the exe links to it dynamically).
foreach ($dll in 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll') {
    $src = Join-Path $env:WINDIR "System32\$dll"
    if (Test-Path $src) { Copy-Item $src $rel -Force }
}

$makensis = @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $makensis) {
    Write-Host 'NSIS not found; installing it with Chocolatey...'
    choco install nsis -y --no-progress | Out-Null
    $makensis = @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $makensis) { throw 'NSIS (makensis) is required: https://nsis.sourceforge.io' }
}
& $makensis "/DVERSION=$Version" (Join-Path $PSScriptRoot 'clothing-studio.nsi')
if ($LASTEXITCODE -ne 0) { throw "makensis failed ($LASTEXITCODE)" }
Get-ChildItem (Join-Path $rel '*-setup.exe') | ForEach-Object { Write-Host "Built $($_.FullName) ($([math]::Round($_.Length / 1MB, 1)) MB)" }
