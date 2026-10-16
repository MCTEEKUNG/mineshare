; Both downloads install the SAME MSI. There is no second NSIS product/uninstaller.
Unicode true
RequestExecutionLevel admin
Name "MineShare"
OutFile "${OUTPUT_FILE}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "MineShare"
VIAddVersionKey "FileDescription" "MineShare installer and updater"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 MineShare Contributors"
Function .onInit
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=MineShare.msi "${MSI_FILE}"
  ; /S remains available for unattended deployment; normal double-click uses
  ; the MSI's install/update UI. Do not interpret paths as shell commands.
  StrCpy $1 ""
  IfSilent silent interactive
  silent:
  StrCpy $1 "/qn AUTOLAUNCHAPP=0"
  interactive:
  ExecWait '"$SYSDIR\msiexec.exe" /i "$PLUGINSDIR\MineShare.msi" /norestart $1' $0
  SetErrorLevel $0
  Quit
FunctionEnd
Section
SectionEnd
