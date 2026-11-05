; MesoVibe's additions to Tauri's NSIS installer (tauri.conf.json bundle.windows.nsis).

; Uninstalling removes the "Open with MapleStory Classic" login item (presence.rs LOGIN_ITEM), so
; Windows doesn't try to start a MesoVibe that's gone. An update runs the old uninstaller with
; /UPDATE first ($UpdateMode is 1): the login item stays, or every update would turn it off.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MesoVibe"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "MesoVibe"
  ${EndIf}
!macroend
