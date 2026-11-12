# Installs Clothing Studio for the current Windows user (no admin rights needed):
#   - builds the release version (needs Rust/cargo)
#   - copies it to %LOCALAPPDATA%\Programs\Clothing Studio
#   - adds Start menu and desktop shortcuts
#   - makes .clothing project files open in Clothing Studio when double-clicked
# Your templates, settings and the background-removal model stay in %APPDATA%\clothing-studio.
$ErrorActionPreference = 'Stop'
$repo = $PSScriptRoot
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Clothing Studio'
$target = Join-Path $dest 'clothing-studio.exe'

if (Get-Process clothing-studio -ErrorAction SilentlyContinue) {
    throw 'Clothing Studio is running. Close it, then run the installer again.'
}

Write-Host 'Building Clothing Studio (release)... this can take a few minutes the first time.'
Push-Location $repo
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'The build failed (see the messages above).' }
} finally {
    Pop-Location
}

Write-Host "Copying to $dest"
New-Item -ItemType Directory -Force $dest | Out-Null
# DirectML.dll is required by the built-in ONNX Runtime (background removal).
Copy-Item (Join-Path $repo 'target\release\clothing-studio.exe'), (Join-Path $repo 'target\release\DirectML.dll') $dest -Force

Write-Host 'Adding Start menu and desktop shortcuts'
$shell = New-Object -ComObject WScript.Shell
$links = @(
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'Clothing Studio.lnk'),
    (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Clothing Studio.lnk')
)
foreach ($link in $links) {
    $s = $shell.CreateShortcut($link)
    $s.TargetPath = $target
    $s.WorkingDirectory = $dest
    $s.IconLocation = "$target,0"
    $s.Description = 'Design Roblox classic shirts'
    $s.Save()
}

Write-Host 'Registering .clothing project files (current user only)'
$classes = 'HKCU:\Software\Classes'
$progId = 'ClothingStudio.Project'
New-Item "$classes\.clothing" -Force | Out-Null
Set-ItemProperty "$classes\.clothing" -Name '(default)' -Value $progId
New-Item "$classes\$progId\DefaultIcon" -Force | Out-Null
New-Item "$classes\$progId\shell\open\command" -Force | Out-Null
Set-ItemProperty "$classes\$progId" -Name '(default)' -Value 'Clothing Studio project'
Set-ItemProperty "$classes\$progId\DefaultIcon" -Name '(default)' -Value "`"$target`",1"
Set-ItemProperty "$classes\$progId\shell\open\command" -Name '(default)' -Value "`"$target`" `"%1`""

# Tell Explorer that file associations changed, so icons update without a restart.
Add-Type -Namespace Win32 -Name Shell -MemberDefinition '[DllImport("shell32.dll")] public static extern void SHChangeNotify(int e, int f, System.IntPtr a, System.IntPtr b);'
[Win32.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host ''
Write-Host 'Done. Start Clothing Studio from the Start menu or the desktop shortcut.' -ForegroundColor Green
Write-Host 'To update later, run install.cmd again. To remove it, run uninstall.cmd.'
