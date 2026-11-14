#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif

#ifndef BuildRoot
  #define BuildRoot "..\target\release"
#endif

#ifndef ArtifactsDir
  #define ArtifactsDir "..\artifacts"
#endif

#define AppName "Pajdflix"
#define AppExeName "stremio-native.exe"

[Setup]
AppId={{9B7477C6-8E3D-4EA7-A128-EB249D052C6C}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Pajdflix
DefaultDirName={localappdata}\Programs\stremio-native
DefaultGroupName=Pajdflix
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Older clients launch Setup before their shutdown has completed. Restart Manager
; still requests a graceful close first; force is the fallback for a stuck client.
CloseApplications=force
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
OutputDir={#ArtifactsDir}
OutputBaseFilename=PajdflixSetup-v{#AppVersion}-x64
SetupIconFile=..\app\assets\app.ico
UninstallDisplayIcon={app}\{#AppExeName}
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoProductVersion={#AppVersion}

[Tasks]
Name: "desktopicon"; Description: "Utwórz skrót na &pulpicie"; GroupDescription: "Dodatkowe skróty:"; Flags: unchecked
Name: "startup"; Description: "Uruchamiaj Pajdflix po zalogowaniu"; GroupDescription: "Autostart aplikacji:"; Flags: unchecked
Name: "magnet"; Description: "Otwieraj linki magnet w Pajdflix"; GroupDescription: "Obsługa linków:"; Flags: unchecked

[Files]
Source: "{#BuildRoot}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildRoot}\libmpv-2.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildRoot}\msvc-runtime\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildRoot}\licenses\mpv\LICENSE.GPL"; DestDir: "{app}\licenses\mpv"; Flags: ignoreversion
Source: "{#BuildRoot}\licenses\mpv\LICENSE.LGPL"; DestDir: "{app}\licenses\mpv"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}\licenses\pajdflix"; Flags: ignoreversion
Source: "..\licenses\thumbfast\LICENSE"; DestDir: "{app}\licenses\thumbfast"; Flags: ignoreversion
Source: "..\licenses\thumbfast\NOTICE.md"; DestDir: "{app}\licenses\thumbfast"; Flags: ignoreversion

; AppUserModelID must match SetCurrentProcessExplicitAppUserModelID in the app so
; Windows resolves the friendly name and icon shown in the media overlay / taskbar.
[Icons]
Name: "{group}\Pajdflix"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; AppUserModelID: "com.stremio.native"
Name: "{autodesktop}\Pajdflix"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon; AppUserModelID: "com.stremio.native"
Name: "{userstartup}\Pajdflix"; Filename: "{app}\{#AppExeName}"; Parameters: "--start-hidden"; WorkingDir: "{app}"; Tasks: startup; AppUserModelID: "com.stremio.native"

[Registry]
; Maintenance is always installed independently of the optional GUI startup task.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "PajdflixCacheMaintenance"; ValueData: """{app}\{#AppExeName}"" --cache-maintenance"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\stremio"; ValueType: string; ValueData: "URL:Stremio Protocol"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\stremio"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\stremio\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExeName},0"
Root: HKCU; Subkey: "Software\Classes\stremio\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExeName}"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\magnet"; ValueType: string; ValueData: "URL:Magnet Protocol"; Flags: uninsdeletekey; Tasks: magnet
Root: HKCU; Subkey: "Software\Classes\magnet"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: magnet
Root: HKCU; Subkey: "Software\Classes\magnet\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExeName},0"; Tasks: magnet
Root: HKCU; Subkey: "Software\Classes\magnet\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExeName}"" ""%1"""; Tasks: magnet

[Run]
Filename: "{app}\{#AppExeName}"; Parameters: "--cache-maintenance"; WorkingDir: "{app}"; Flags: nowait runhidden
Filename: "{app}\{#AppExeName}"; Description: "Uruchom Pajdflix"; WorkingDir: "{app}"; Flags: nowait postinstall
