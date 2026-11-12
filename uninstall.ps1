# Removes what install.ps1 added: the installed app, its shortcuts and the .clothing file type.
# Your projects, and your templates/settings/model in %APPDATA%\clothing-studio, are kept.
$ErrorActionPreference = 'Stop'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Clothing Studio'

if (Get-Process clothing-studio -ErrorAction SilentlyContinue) {
    throw 'Clothing Studio is running. Close it, then run the uninstaller again.'
}

foreach ($link in @(
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'Clothing Studio.lnk'),
    (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Clothing Studio.lnk'))) {
    if (Test-Path $link) { Remove-Item $link }
}
if (Test-Path $dest) { Remove-Item $dest -Recurse }

$classes = 'HKCU:\Software\Classes'
# Only remove the .clothing association if it still points at us.
if ((Get-ItemProperty "$classes\.clothing" -ErrorAction SilentlyContinue).'(default)' -eq 'ClothingStudio.Project') {
    Remove-Item "$classes\.clothing" -Recurse
}
if (Test-Path "$classes\ClothingStudio.Project") { Remove-Item "$classes\ClothingStudio.Project" -Recurse }

Write-Host 'Clothing Studio was removed.' -ForegroundColor Green
Write-Host "Your templates, settings and the background-removal model are still in $env:APPDATA\clothing-studio."
Write-Host 'Delete that folder too if you want to remove everything.'
