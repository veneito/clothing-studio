; Windows installer for Clothing Studio (NSIS). Built by .github/workflows/release.yml and
; test-build.yml:  makensis /DVERSION=1.2.3 installer\clothing-studio.nsi
; Per-user install (no admin): %LOCALAPPDATA%\Programs\Clothing Studio, Start menu + desktop
; shortcuts, .clothing project files, and an entry in Settings > Apps with an uninstaller.

Unicode true
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define APP "Clothing Studio"
!define EXE "clothing-studio.exe"
!define PROGID "ClothingStudio.Project"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClothingStudio"
!define REL "..\target\release"

Name "${APP}"
OutFile "${REL}\Clothing.Studio_${VERSION}_x64-setup.exe"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
SetCompressor /SOLID lzma
BrandingText "${APP} ${VERSION}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "FileDescription" "${APP} installer"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"

!include "MUI2.nsh"
!define MUI_ICON "..\assets\icon.ico"
!define MUI_UNICON "..\assets\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Run ${APP}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  ; If the app is open, its files are locked: NSIS then offers Retry after the user closes it.
  File "${REL}\${EXE}"
  File "${REL}\DirectML.dll"
  ; Microsoft C++ runtime, app-local, so it also runs on PCs without the redistributable.
  File /nonfatal "${REL}\vcruntime140.dll"
  File /nonfatal "${REL}\vcruntime140_1.dll"
  File /nonfatal "${REL}\msvcp140.dll"
  File /nonfatal "${REL}\msvcp140_1.dll"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
  CreateShortcut "$DESKTOP\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0

  ; .clothing project files open in the app (current user only)
  WriteRegStr HKCU "Software\Classes\.clothing" "" "${PROGID}"
  WriteRegStr HKCU "Software\Classes\${PROGID}" "" "${APP} project"
  WriteRegStr HKCU "Software\Classes\${PROGID}\DefaultIcon" "" '"$INSTDIR\${EXE}",1'
  WriteRegStr HKCU "Software\Classes\${PROGID}\shell\open\command" "" '"$INSTDIR\${EXE}" "%1"'

  ; Settings > Apps entry
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "veneito"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  ; refresh Explorer's file-type icons
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\DirectML.dll"
  Delete "$INSTDIR\vcruntime140.dll"
  Delete "$INSTDIR\vcruntime140_1.dll"
  Delete "$INSTDIR\msvcp140.dll"
  Delete "$INSTDIR\msvcp140_1.dll"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${APP}.lnk"
  Delete "$DESKTOP\${APP}.lnk"

  ; only remove the .clothing association if it still points at us
  ReadRegStr $0 HKCU "Software\Classes\.clothing" ""
  StrCmp $0 "${PROGID}" 0 +2
    DeleteRegKey HKCU "Software\Classes\.clothing"
  DeleteRegKey HKCU "Software\Classes\${PROGID}"
  DeleteRegKey HKCU "${UNINST_KEY}"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  ; Projects and %APPDATA%\clothing-studio (templates, settings, AI model) are kept on purpose.
SectionEnd
