!include nsDialogs.nsh
!include LogicLib.nsh
Var ClearFrameContextConsent
Var ClearFrameContextCheckbox
Var ClearFrameContextVisited
LangString ClearFrameContextTitle 1033 "Explorer right-click actions"
LangString ClearFrameContextTitle 2052 "资源管理器右键操作"
LangString ClearFrameContextTitle 1041 "エクスプローラーの右クリック操作"
LangString ClearFrameContextInfo 1033 "Single results are saved beside the source. Batch results go into a new ClearFrame Cutouts folder. Existing files are kept. Prepare IS-Net in the app before using these actions."
LangString ClearFrameContextInfo 2052 "单图结果保存在原图旁；批量结果保存在新建的 ClearFrame Cutouts 文件夹中。已有文件不会覆盖。使用前请在应用中准备好 IS-Net 模型。"
LangString ClearFrameContextInfo 1041 "単一画像の結果は元画像の横、一括処理の結果は新しい ClearFrame Cutouts フォルダーに保存します。既存ファイルは保持します。先にアプリで IS-Net モデルを準備してください。"
LangString ClearFrameContextOption 1033 "Enable single-image and batch background removal in right-click menus"
LangString ClearFrameContextOption 2052 "启用右键单图与批量抠图"
LangString ClearFrameContextOption 1041 "右クリックで単一画像・一括の背景削除を有効にする"
LangString ClearFrameContextError 1033 "Could not register the right-click actions. The application remains installed."
LangString ClearFrameContextError 2052 "无法注册右键菜单，应用仍已安装。"
LangString ClearFrameContextError 1041 "右クリック操作を登録できませんでした。アプリのインストールは完了しています。"
; This page is included before Tauri's standard pages. Consent is explicit;
; silent/passive installs never set it and therefore never enable the extension.
Page custom ClearFrameContextPage ClearFrameContextLeave
Function ClearFrameContextPage
  IfSilent clearframe_no_consent
  ClearErrors
  ${GetOptions} $CMDLINE "/P" $0
  IfErrors +2
  Goto clearframe_no_consent
  nsDialogs::Create 1018
  Pop $0
  StrCmp $0 error clearframe_no_consent
  ${NSD_CreateLabel} 0 0 100% 20u "$(ClearFrameContextTitle)"
  Pop $0
  ${NSD_CreateLabel} 0 30u 100% 60u "$(ClearFrameContextInfo)"
  Pop $0
  ${NSD_CreateCheckbox} 0 100u 100% 35u "$(ClearFrameContextOption)"
  Pop $ClearFrameContextCheckbox
  ${If} $ClearFrameContextVisited != 1
    StrCpy $ClearFrameContextConsent ${BST_CHECKED}
    StrCpy $ClearFrameContextVisited 1
  ${EndIf}
  ${NSD_SetState} $ClearFrameContextCheckbox $ClearFrameContextConsent
  nsDialogs::Show
  Return
  clearframe_no_consent:
    StrCpy $ClearFrameContextConsent 0
    Abort
FunctionEnd
Function ClearFrameContextLeave
  ${NSD_GetState} $ClearFrameContextCheckbox $ClearFrameContextConsent
FunctionEnd

!define CF_CONTEXT_CLSID "{C6678B57-1379-4D98-9B5C-59933C6D42B2}"
!macro CF_CONTEXT_REMOVE
  SetRegView 64
  DeleteRegKey HKCU "Software\Classes\CLSID\${CF_CONTEXT_CLSID}"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.png\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpg\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpeg\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpe\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jfif\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.webp\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.gif\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.bmp\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.tif\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.tiff\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.ico\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.tga\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.dds\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.pnm\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.pbm\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.pgm\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.ppm\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.pam\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.qoi\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.ff\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\Directory\shellex\ContextMenuHandlers\ClearFrame"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shellex\ContextMenuHandlers\ClearFrame"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  SetRegView last
  ClearErrors
!macroend
!macro CF_CONTEXT_HANDLER key
  WriteRegStr HKCU "Software\Classes\${key}\shellex\ContextMenuHandlers\ClearFrame" "" "${CF_CONTEXT_CLSID}"
!macroend
!macro NSIS_HOOK_POSTINSTALL
  !insertmacro CF_CONTEXT_REMOVE
  ${If} $ClearFrameContextConsent == ${BST_CHECKED}
    SetRegView 64
    WriteRegStr HKCU "Software\Classes\CLSID\${CF_CONTEXT_CLSID}" "" "ClearFrame context actions"
    WriteRegStr HKCU "Software\Classes\CLSID\${CF_CONTEXT_CLSID}\InprocServer32" "" "$INSTDIR\clearframe-shell.dll"
    WriteRegStr HKCU "Software\Classes\CLSID\${CF_CONTEXT_CLSID}\InprocServer32" "ThreadingModel" "Apartment"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.png"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.jpg"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.jpeg"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.jpe"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.jfif"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.webp"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.gif"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.bmp"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.dib"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.tif"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.tiff"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.ico"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.tga"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.dds"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.pnm"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.pbm"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.pgm"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.ppm"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.pam"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.qoi"
    !insertmacro CF_CONTEXT_HANDLER "SystemFileAssociations\.ff"
    !insertmacro CF_CONTEXT_HANDLER "Directory"
    !insertmacro CF_CONTEXT_HANDLER "Directory\Background"
    SetRegView last
    ${If} ${Errors}
      !insertmacro CF_CONTEXT_REMOVE
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(ClearFrameContextError)"
    ${Else}
      System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
    ${EndIf}
  ${EndIf}
  ClearErrors
!macroend
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CF_CONTEXT_REMOVE
  Delete /REBOOTOK "$INSTDIR\clearframe-shell.dll"
!macroend
