; DeepSeekCode Windows NSIS installer
; Build via: packaging/desktop/windows/build-installer.ps1
;
; Install scope: per-user by default, per-machine when elevated. The mode
; page lets admins choose; non-admins get CurrentUser only (no forced
; elevation). Registry/start-menu writes follow the selected scope via
; SHCTX + SetShellVarContext.

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define MULTIUSER_EXECUTIONLEVEL Highest
!define MULTIUSER_MUI
!define MULTIUSER_INSTALLMODE_DEFAULT_CURRENTUSER
!define MULTIUSER_INSTALLMODE_COMMANDLINE
!define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "Software\io.deepseekcode.desktop"
!define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "InstallDir"
!define MULTIUSER_INSTALLMODE_INSTDIR "DeepSeekCode"
!define MULTIUSER_INSTALLMODE_INSTDIR_REGISTRY_KEY "Software\io.deepseekcode.desktop"
!define MULTIUSER_INSTALLMODE_INSTDIR_REGISTRY_VALUENAME "InstallDir"
!include "MultiUser.nsh"

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
!define APP_EXE "deepseekcode-desktop.exe"
!define APP_ID "io.deepseekcode.desktop"
!define START_MENU_DIR "DeepSeekCode"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "${OUT_DIR}\DeepSeekCode-${APP_VERSION}-x64-setup.exe"
; No InstallDirRegKey: MultiUser resolves $INSTDIR from the defines above
; (InstallDirRegKey accepts only literal HKLM/HKCU, not the scope-aware SHCTX).
Unicode True
SetCompressor /SOLID lzma

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch DeepSeekCode"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MULTIUSER_PAGE_INSTALLMODE
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
  ${If} $MultiUser.InstallMode == "AllUsers"
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}
  SetOutPath "$INSTDIR"
  File /r "${STAGING_DIR}\*.*"

  WriteRegStr SHCTX "Software\${APP_ID}" "InstallDir" "$INSTDIR"
  WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayName" "${APP_NAME}"
  WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegStr SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegDWORD SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoModify" 1
  WriteRegDWORD SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoRepair" 1

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\${START_MENU_DIR}"
  CreateShortcut "$SMPROGRAMS\${START_MENU_DIR}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortcut "$SMPROGRAMS\${START_MENU_DIR}\Uninstall ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  ${If} $MultiUser.InstallMode == "AllUsers"
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$INSTDIR\${APP_EXE}"
  ; Remove any bundled DLLs
  Delete "$INSTDIR\*.dll"
  RMDir /r "$INSTDIR"

  Delete "$SMPROGRAMS\${START_MENU_DIR}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${START_MENU_DIR}\Uninstall ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${START_MENU_DIR}"

  DeleteRegKey SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}"
  DeleteRegKey SHCTX "Software\${APP_ID}"
SectionEnd

Function .onInit
  !insertmacro MULTIUSER_INIT
  ${GetParameters} $R0
  ${GetOptions} $R0 "/D=" $R1
  StrCmp $R1 "" +2 0
    StrCpy $INSTDIR $R1
FunctionEnd

Function un.onInit
  !insertmacro MULTIUSER_UNINIT
FunctionEnd
