; ==============================================================================
; Ruston Mail - Windows installer (Inno Setup 6)
; Installs for the current user without administrator rights. package.ps1
; passes the version, the architecture, and the folders:
;   ISCC /DVersion=0.7.1 /DArch=x86_64 /DSourceDir=<binaries> /DOutputDir=<dist> ruston-mail.iss
; ==============================================================================

#ifndef Version
  #error Pass /DVersion=<x.y.z>
#endif
#ifndef Arch
  #error Pass /DArch=x86_64 or /DArch=arm64
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<folder with ruston.exe and ruston-cli.exe>
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

#if Arch == "arm64"
  #define InnoArchitectures "arm64"
#elif Arch == "x86_64"
  #define InnoArchitectures "x64compatible"
#else
  #error Unknown architecture
#endif

; Matches the app's AppUserModelID, which Windows toasts need on the shortcut.
#define AppId "com.luiscuellar.ruston-mail"
#define MailtoProgId "com.luiscuellar.ruston-mail.mailto"
#define Repository "https://github.com/luiscuellar31/ruston-mail"

[Setup]
; Fixed forever: it is how a new version finds and upgrades the installed one.
AppId={{F5540D77-9DAD-461F-9168-030CF7B10662}
AppName=Ruston Mail
AppVersion={#Version}
AppVerName=Ruston Mail {#Version}
AppPublisher=Luis Cuellar
AppPublisherURL={#Repository}
AppSupportURL={#Repository}/issues
AppUpdatesURL={#Repository}/releases
AppCopyright=Copyright (c) 2026 Luis Cuellar
VersionInfoVersion={#Version}.0
VersionInfoDescription=Ruston Mail Setup
; Per-user only: {autopf} is then %LOCALAPPDATA%\Programs, and the app looks
; for its Start menu shortcut in the user's own Programs folder.
PrivilegesRequired=lowest
DefaultDirName={autopf}\Ruston Mail
DisableProgramGroupPage=yes
ArchitecturesAllowed={#InnoArchitectures}
ArchitecturesInstallIn64BitMode={#InnoArchitectures}
MinVersion=10.0
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no
SetupIconFile=..\..\assets\windows\ruston-mail.ico
UninstallDisplayIcon={app}\ruston.exe
UninstallDisplayName=Ruston Mail
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=ruston-mail-{#Version}-{#Arch}-setup-UNSIGNED

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "mailto"; Description: "Offer Ruston Mail as an app for email links (mailto)"; GroupDescription: "Integration:"
Name: "path"; Description: "Add ruston-cli to the PATH"; GroupDescription: "Integration:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\ruston.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\ruston-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Ruston Mail"; Filename: "{app}\ruston.exe"; AppUserModelID: "{#AppId}"; Comment: "Unofficial desktop client for Proton Mail"

[Registry]
; The same per-user registration as register-mailto.ps1. It makes Ruston Mail
; selectable in Settings > Apps > Default apps without changing the default.
Root: HKCU; Subkey: "Software\Classes\{#MailtoProgId}"; ValueType: string; ValueName: ""; ValueData: "Ruston Mail email link"; Flags: uninsdeletekey; Tasks: mailto
Root: HKCU; Subkey: "Software\Classes\{#MailtoProgId}"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: mailto
Root: HKCU; Subkey: "Software\Classes\{#MailtoProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\ruston.exe"",0"; Tasks: mailto
Root: HKCU; Subkey: "Software\Classes\{#MailtoProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ruston.exe"" ""%1"""; Tasks: mailto
Root: HKCU; Subkey: "Software\Ruston Mail"; Flags: uninsdeletekeyifempty; Tasks: mailto
Root: HKCU; Subkey: "Software\Ruston Mail\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Ruston Mail"; Flags: uninsdeletekey; Tasks: mailto
Root: HKCU; Subkey: "Software\Ruston Mail\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Unofficial native desktop client for Proton Mail"; Tasks: mailto
Root: HKCU; Subkey: "Software\Ruston Mail\Capabilities\UrlAssociations"; ValueType: string; ValueName: "mailto"; ValueData: "{#MailtoProgId}"; Tasks: mailto
; Only Ruston Mail's value: the key lists every other registered app too.
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "Ruston Mail"; ValueData: "Software\Ruston Mail\Capabilities"; Flags: uninsdeletevalue; Tasks: mailto

[Run]
Filename: "{app}\ruston.exe"; Description: "Open Ruston Mail"; Flags: nowait postinstall skipifsilent

; Uninstalling leaves settings, the signed-in session, the CLI's cache, and the
; single-instance lock file, which must never be removed while a copy of the
; app, such as a portable one, may hold it.

[Code]
const
  EnvironmentKey = 'Environment';

{ Where Dir sits in the user's PATH, 0 when it is absent. }
function PathPosition(Paths, Dir: String): Integer;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';');
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(HKCU, EnvironmentKey, 'Path', Paths) then
    Paths := '';
  if PathPosition(Paths, Dir) > 0 then
    exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(HKCU, EnvironmentKey, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths: String;
  Position: Integer;
begin
  if not RegQueryStringValue(HKCU, EnvironmentKey, 'Path', Paths) then
    exit;
  Position := PathPosition(Paths, Dir);
  if Position = 0 then
    exit;
  { In the list wrapped in ';', Position is the separator before Dir. }
  Paths := ';' + Paths + ';';
  Delete(Paths, Position, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2);
  RegWriteExpandStringValue(HKCU, EnvironmentKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('path') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
