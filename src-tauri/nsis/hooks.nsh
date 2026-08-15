; Registers and removes the privileged helper service as part of the normal
; installer flow, so a Windows user never has to run a script by hand. The
; installer already runs elevated, which is exactly what service registration
; needs — asking again afterwards would be a worse experience for no gain.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering the Claude Awake helper service..."
  nsExec::ExecToLog '"$INSTDIR\claude-awake-helperd.exe" --install-service'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Helper service registration failed ($0). Run scripts\install-helper.ps1 as Administrator."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing the Claude Awake helper service..."
  ; The service reverts the power settings on its own stop path, so this must run
  ; before the files are deleted.
  nsExec::ExecToLog '"$INSTDIR\claude-awake-helperd.exe" --uninstall-service'
  Pop $0
!macroend
