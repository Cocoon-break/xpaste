!macro NSIS_HOOK_POSTUNINSTALL
  ; 开机启动项（tauri-plugin-autostart → auto-launch，值名为 productName）
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "xPaste"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "xPaste"
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$APPDATA\xPaste"
  ${EndIf}
!macroend
