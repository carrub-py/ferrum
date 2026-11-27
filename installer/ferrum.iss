; Ferrum Browser — Instalador Windows x64 (sin permisos de administrador)
; Compilado automáticamente por GitHub Actions con Inno Setup 6 (`iscc`)

#ifndef AppVersion
  #define AppVersion "0.6.1"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist"
#endif

[Setup]
AppId={{8E8DAFDA-DB69-4AA1-84E3-4F29A9EF7325}
AppName=Ferrum Browser
AppVersion={#AppVersion}
AppVerName=Ferrum Browser {#AppVersion}
AppPublisher=carrub-py
AppPublisherURL=https://github.com/carrub-py/ferrum
AppSupportURL=https://github.com/carrub-py/ferrum/issues
AppUpdatesURL=https://github.com/carrub-py/ferrum/releases
DefaultDirName={localappdata}\Programs\FerrumBrowser
DefaultGroupName=Ferrum Browser
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE
OutputDir={#SourceDir}
OutputBaseFilename=ferrum-browser-setup-x64
SetupIconFile={#SourceDir}\ferrum.ico
UninstallDisplayIcon={app}\ferrum-browser.exe
UninstallDisplayName=Ferrum Browser
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Crear acceso directo en el Escritorio"; GroupDescription: "Accesos directos:"; Flags: unchecked
Name: "registerbrowser"; Description: "Registrar en Aplicaciones predeterminadas de Windows"; GroupDescription: "Integración con Windows:"

[Files]
Source: "{#SourceDir}\ferrum-browser.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Ferrum Browser"; Filename: "{app}\ferrum-browser.exe"; WorkingDir: "{app}"; Comment: "Ferrum Browser — Navegador web minimalista y seguro en Rust"
Name: "{autodesktop}\Ferrum Browser"; Filename: "{app}\ferrum-browser.exe"; WorkingDir: "{app}"; Tasks: desktopicon; Comment: "Ferrum Browser — Navegador web minimalista y seguro en Rust"

[Registry]
; Per-user browser registration (HKCU) — cleanly removed on uninstall
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser"; ValueType: string; ValueData: "Ferrum Browser"; Tasks: registerbrowser; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\DefaultIcon"; ValueType: string; ValueData: "{app}\ferrum-browser.exe,0"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\shell\open\command"; ValueType: string; ValueData: """{app}\ferrum-browser.exe"""; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Ferrum Browser"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Navegador web minimalista y seguro en Rust"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\ferrum-browser.exe,0"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities\URLAssociations"; ValueType: string; ValueName: "http"; ValueData: "FerrumHTML"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities\URLAssociations"; ValueType: string; ValueName: "https"; ValueData: "FerrumHTML"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities\FileAssociations"; ValueType: string; ValueName: ".htm"; ValueData: "FerrumHTML"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities\FileAssociations"; ValueType: string; ValueName: ".html"; ValueData: "FerrumHTML"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pdf"; ValueData: "FerrumHTML"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "FerrumBrowser"; ValueData: "Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities"; Tasks: registerbrowser; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\FerrumHTML"; ValueType: string; ValueData: "Ferrum Web Document"; Tasks: registerbrowser; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\FerrumHTML\DefaultIcon"; ValueType: string; ValueData: "{app}\ferrum-browser.exe,0"; Tasks: registerbrowser
Root: HKCU; Subkey: "Software\Classes\FerrumHTML\shell\open\command"; ValueType: string; ValueData: """{app}\ferrum-browser.exe"" ""%1"""; Tasks: registerbrowser

[Run]
Filename: "{app}\ferrum-browser.exe"; Description: "Abrir Ferrum Browser"; Flags: nowait postinstall skipifsilent

[Code]
function HasWebView2Runtime(): Boolean;
var
  Ver: String;
begin
  Result :=
    (RegQueryStringValue(HKLM64, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Ver) and (Ver <> '') and (Ver <> '0.0.0.0')) or
    (RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Ver) and (Ver <> '') and (Ver <> '0.0.0.0'));
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ErrCode: Integer;
begin
  if (CurStep = ssPostInstall) and (not HasWebView2Runtime()) then
  begin
    if MsgBox('Ferrum Browser requiere Microsoft Edge WebView2 Runtime, que no se detectó en este equipo. ¿Quieres abrir la página oficial de Microsoft para instalarlo?', mbConfirmation, MB_YESNO) = IDYES then
      ShellExec('open', 'https://developer.microsoft.com/en-us/microsoft-edge/webview2/', '', '', SW_SHOWNORMAL, ewNoWait, ErrCode);
  end;
end;
