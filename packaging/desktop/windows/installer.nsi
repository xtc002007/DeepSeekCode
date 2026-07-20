; DeepSeekCode Windows NSIS installer
; Build via: packaging/desktop/windows/build-installer.ps1

!include "MUI2.nsh"
!include "FileFunc.nsh"

!ifndef APP_VERSION
  !define APP_VERSION "0.1.0"
!endif

!ifndef STAGING_DIR
  !define STAGING_DIR "staging"
!endif

!ifndef OUT_DIR
  !define OUT_DIR "dist"
!endif

!define APP_NAME "DeepSeekCode"
!define APP_PUBLISHER "DeepSeekCode"
!define APP_EXE "deepseekcode.exe"
!define APP_ID "io.deepseekcode.desktop"
!define INSTALL_DIR "$PROGRAMFILES64\DeepSeekCode"
!define START_MENU_DIR "DeepSeekCode"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "${OUT_DIR}\DeepSeekCode-${APP_VERSION}-x64-setup.exe"
InstallDir "${INSTALL_DIR}"
InstallDirRegKey HKLM "Software\${APP_ID}" "InstallDir"
RequestExecutionLevel admin
Unicode True
SetCompressor /SOLID lzma

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch DeepSeekCode"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  File /r "${STAGING_DIR}\*.*"

  WriteRegStr HKLM "Software\${APP_ID}" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoRepair" 1

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\${START_MENU_DIR}"
  CreateShortcut "$SMPROGRAMS\${START_MENU_DIR}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortcut "$SMPROGRAMS\${START_MENU_DIR}\Uninstall ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$INSTDIR\${APP_EXE}"
  ; Remove any bundled DLLs
  Delete "$INSTDIR\*.dll"
  RMDir /r "$INSTDIR"

  Delete "$SMPROGRAMS\${START_MENU_DIR}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${START_MENU_DIR}\Uninstall ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${START_MENU_DIR}"

  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}"
  DeleteRegKey HKLM "Software\${APP_ID}"
SectionEnd

Function .onInit
  ${GetParameters} $R0
  ${GetOptions} $R0 "/D=" $R1
  StrCmp $R1 "" +2 0
    StrCpy $INSTDIR $R1
FunctionEnd
