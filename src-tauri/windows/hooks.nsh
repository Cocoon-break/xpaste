!macro NSIS_HOOK_POSTUNINSTALL
  ; 开机启动项（tauri-plugin-autostart → auto-launch，值名为 productName）
  ; 与 Tauri 模板一致：更新模式（/UPDATE）下保留，只有真正卸载时删除
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "xPaste"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "xPaste"
  ${EndIf}
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$APPDATA\xPaste"
  ${EndIf}
!macroend
