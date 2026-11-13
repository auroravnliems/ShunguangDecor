; Installer hooks for NSIS (Tauri `bundle.windows.nsis.installerHooks`). App-side code: src-tauri/src/lifecycle.rs.
; The file is saved as UTF-8 WITH BOM: NSIS reads the Unicode text of an !include file correctly only when a BOM is present.
;
; - Before installing over an existing copy: ask the running instance to exit gracefully (`--quit`) so the modules put Windows back as it was
;   and the exe file is not locked.
; - Before uninstalling:
;   1. If Hidden features changes are still not undone (`--features-pending` returns code 1) → ask whether to continue uninstalling.
;      Uninstalling does NOT undo them by itself (they are persistent settings, the user decides). Choosing "No" = abort the uninstall.
;   2. `--uninstall-cleanup` exits the app, then restores the cursors, wallpaper, desktop label colors, the original taskbar,
;      and removes the start-with-Windows entry ("uninstalling the app also restores").
; - After uninstalling, if the user ticks "Delete app data": also delete ShunguangDecor's data folder
;   (the app uses the name "ShunguangDecor" = APP_DIR_NAME of sg_core::paths, which is not the bundle identifier, so Tauri
;   does not delete it by itself). The bin\ folder may still hold a DLL loaded in Explorer → delete it if possible, otherwise leave it
;   (do not use /REBOOTOK: the uninstaller has no admin rights, so it cannot schedule deletion at reboot).

; $R9 = the Hidden features warning in the language chosen in the installer (Windows LANGID code).
!macro SG_FEATURES_PENDING_TEXT
  ${If} $LANGUAGE = 1066
    StrCpy $R9 "Một số tính năng ẩn của Windows do ShunguangDecor đổi vẫn đang có hiệu lực. Gỡ app sẽ KHÔNG hoàn tác chúng.$\r$\n$\r$\nMuốn hoàn tác: mở ShunguangDecor → Tính năng ẩn → Công cụ → “Hoàn tác tất cả thay đổi do ShunguangDecor làm”, rồi gỡ lại.$\r$\n$\r$\nVẫn gỡ và giữ nguyên các thay đổi đó?"
  ${ElseIf} $LANGUAGE = 1049
    StrCpy $R9 "Некоторые скрытые функции Windows, изменённые ShunguangDecor, всё ещё действуют. Удаление НЕ отменит их.$\r$\n$\r$\nЧтобы отменить: откройте ShunguangDecor → Скрытые функции → Инструменты → «Откатить все изменения ShunguangDecor», затем снова запустите удаление.$\r$\n$\r$\nВсё равно удалить и сохранить эти изменения?"
  ${ElseIf} $LANGUAGE = 2052
    StrCpy $R9 "ShunguangDecor 更改过的部分 Windows 隐藏功能仍然有效。卸载不会撤销这些更改。$\r$\n$\r$\n如需撤销：打开 ShunguangDecor → 隐藏功能 → 工具 →“撤销 ShunguangDecor 所做的全部更改”，然后重新卸载。$\r$\n$\r$\n仍要卸载并保留这些更改吗？"
  ${ElseIf} $LANGUAGE = 1042
    StrCpy $R9 "ShunguangDecor가 변경한 Windows 숨겨진 기능 일부가 아직 적용되어 있습니다. 제거해도 되돌려지지 않습니다.$\r$\n$\r$\n되돌리려면: ShunguangDecor → 숨겨진 기능 → 도구 → “ShunguangDecor가 한 모든 변경 되돌리기”를 실행한 뒤 다시 제거하세요.$\r$\n$\r$\n그래도 제거하고 변경 사항을 유지할까요?"
  ${ElseIf} $LANGUAGE = 1041
    StrCpy $R9 "ShunguangDecor が変更した Windows の隠し機能の一部がまだ有効です。アンインストールしても元に戻りません。$\r$\n$\r$\n元に戻すには: ShunguangDecor → 隠し機能 → ツール →「ShunguangDecor が行った変更をすべて元に戻す」を実行してから、もう一度アンインストールしてください。$\r$\n$\r$\nこのままアンインストールして変更を残しますか？"
  ${ElseIf} $LANGUAGE = 1031
    StrCpy $R9 "Einige von ShunguangDecor geänderte versteckte Windows-Funktionen sind noch aktiv. Die Deinstallation macht sie NICHT rückgängig.$\r$\n$\r$\nZum Rückgängigmachen: ShunguangDecor öffnen → Versteckte Funktionen → Werkzeuge → „Alle Änderungen von ShunguangDecor rückgängig machen“, dann erneut deinstallieren.$\r$\n$\r$\nTrotzdem deinstallieren und die Änderungen behalten?"
  ${ElseIf} $LANGUAGE = 1034
    StrCpy $R9 "Algunas funciones ocultas de Windows cambiadas por ShunguangDecor siguen activas. Desinstalar NO las deshará.$\r$\n$\r$\nPara deshacerlas: abre ShunguangDecor → Funciones ocultas → Herramientas → «Deshacer todos los cambios hechos por ShunguangDecor» y vuelve a desinstalar.$\r$\n$\r$\n¿Desinstalar de todos modos y conservar esos cambios?"
  ${ElseIf} $LANGUAGE = 1046
    StrCpy $R9 "Alguns recursos ocultos do Windows alterados pelo ShunguangDecor ainda estão ativos. A desinstalação NÃO vai desfazê-los.$\r$\n$\r$\nPara desfazer: abra o ShunguangDecor → Recursos ocultos → Ferramentas → “Desfazer todas as alterações feitas pelo ShunguangDecor” e desinstale novamente.$\r$\n$\r$\nDesinstalar mesmo assim e manter essas alterações?"
  ${ElseIf} $LANGUAGE = 1036
    StrCpy $R9 "Certaines fonctions cachées de Windows modifiées par ShunguangDecor sont toujours actives. La désinstallation ne les annulera PAS.$\r$\n$\r$\nPour les annuler : ouvrez ShunguangDecor → Fonctions cachées → Outils → « Annuler toutes les modifications faites par ShunguangDecor », puis désinstallez à nouveau.$\r$\n$\r$\nDésinstaller quand même et conserver ces modifications ?"
  ${Else}
    StrCpy $R9 "Some hidden Windows features changed by ShunguangDecor are still in effect. Uninstalling will NOT undo them.$\r$\n$\r$\nTo undo them: open ShunguangDecor → Hidden features → Tools → “Undo all changes made by ShunguangDecor”, then uninstall again.$\r$\n$\r$\nUninstall anyway and keep those changes?"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --quit'
  ${EndIf}
!macroend

; When a new version is installed over an old one, Tauri runs the old version's uninstaller in update mode ($UpdateMode = 1):
; in that case it only exits the app and does NOT restore the cursors/wallpaper (the user is still using them).
!macro NSIS_HOOK_PREUNINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    ${If} $UpdateMode = 1
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --quit'
    ${Else}
      ClearErrors
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --features-pending' $0
      ${IfNot} ${Errors}
      ${AndIf} $0 = 1
        !insertmacro SG_FEATURES_PENDING_TEXT
        ; The default is "No" (safe); a silent uninstall (/S) counts as "Yes".
        MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "$R9" /SD IDYES IDYES +2
        Abort
      ${EndIf}
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-cleanup'
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$APPDATA\ShunguangDecor"
    RMDir /r "$LOCALAPPDATA\ShunguangDecor"
  ${EndIf}
!macroend
