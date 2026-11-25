!include WinVer.nsh
!include x64.nsh
!include LogicLib.nsh

!macro NSIS_HOOK_PREINSTALL
  ; 系统检查（docs/设备支持.md）：Windows 10 on ARM 不能运行 x64 应用，装上也打不开，直接拦下；
  ; 低于 Windows 10 21H2（build 19044）没测过，提示后由用户决定。静默安装（/S）时按默认：ARM 拦下，旧版本继续
  ${If} ${IsNativeARM64}
  ${AndIfNot} ${AtLeastBuild} 22000
    MessageBox MB_ICONSTOP|MB_OK "xPaste 在 ARM 设备上需要 Windows 11：Windows 10 on ARM 不能运行 x64 应用。" /SD IDOK
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastBuild} 19044
    ${If} ${Cmd} `MessageBox MB_ICONEXCLAMATION|MB_YESNO "当前系统低于 Windows 10 21H2，xPaste 没有在这个版本上测试过，截图、文字识别等功能可能不能用。仍要安装吗？" /SD IDYES IDNO`
      Abort
    ${EndIf}
  ${EndIf}
!macroend

; 系统的“打开方式”（Pro 的改字）：只登记成可选，不改默认应用
!macro XPASTE_EACH_IMAGE_EXT DO
  !insertmacro ${DO} "png"
  !insertmacro ${DO} "jpg"
  !insertmacro ${DO} "jpeg"
  !insertmacro ${DO} "webp"
  !insertmacro ${DO} "bmp"
  !insertmacro ${DO} "tif"
  !insertmacro ${DO} "tiff"
  !insertmacro ${DO} "gif"
!macroend
!macro XPASTE_OPENWITH_ADD EXT
  WriteRegStr HKCU "Software\Classes\.${EXT}\OpenWithProgids" "xPaste.Image" ""
!macroend
!macro XPASTE_OPENWITH_DEL EXT
  DeleteRegValue HKCU "Software\Classes\.${EXT}\OpenWithProgids" "xPaste.Image"
!macroend
!macro XPASTE_OPENWITH_CLEAR
  !insertmacro XPASTE_EACH_IMAGE_EXT XPASTE_OPENWITH_DEL
  DeleteRegKey HKCU "Software\Classes\xPaste.Image"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; 打包时 Makefile 在带 pro/ 时设 XPASTE_PRO=1；没有 Pro 的安装程序不登记，装在带 Pro 的上面时清掉原来的
  !if "$%XPASTE_PRO%" == "1"
    WriteRegStr HKCU "Software\Classes\xPaste.Image" "" "图片"
    WriteRegStr HKCU "Software\Classes\xPaste.Image\DefaultIcon" "" "$INSTDIR\${MAINBINARYNAME}.exe,0"
    WriteRegStr HKCU "Software\Classes\xPaste.Image\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
    !insertmacro XPASTE_EACH_IMAGE_EXT XPASTE_OPENWITH_ADD
  !else
    !insertmacro XPASTE_OPENWITH_CLEAR
  !endif
  ; SHCNE_ASSOCCHANGED：让资源管理器重读
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro XPASTE_OPENWITH_CLEAR
  ${EndIf}
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
