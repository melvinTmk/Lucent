!macro NSIS_HOOK_PREUNINSTALL
  ; Stop every lucent.exe (app and watchdog), then restore the taskbar explicitly
  ; (--restore-taskbar also makes apps re-add their tray icons to Explorer).
  nsExec::Exec 'taskkill /F /IM lucent.exe'
  Sleep 800
  nsExec::Exec '"$INSTDIR\lucent.exe" --restore-taskbar'
  ; Remove autostart entry.
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Lucent"
  ${EndIf}
!macroend
