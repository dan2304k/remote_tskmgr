; rtm-sender.iss - Inno Setup 6 script producing rtm-sender-setup.exe
; Compile on Windows with Inno Setup (https://jrsoftware.org/isinfo.php):
;   iscc packaging\windows\rtm-sender.iss
; after sender\build.bat has produced dist\windows-x86_64\rtm-sender.exe.

#define AppName "rtm-sender"
#define AppVersion GetVersionNumbersString("..\..\dist\windows-x86_64\rtm-sender.exe")
#if AppVersion == ""
  #define AppVersion "1.0.0"
#endif

[Setup]
AppId={{7D2E0E7E-5B1C-4C4B-9A1B-RTMSENDER0001}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Remote Task Manager
DefaultDirName={autopf}\rtm-sender
DefaultGroupName=rtm-sender
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\..\dist\windows-x86_64
OutputBaseFilename=rtm-sender-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\rtm-sender.exe
CloseApplications=yes
RestartApplications=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "..\..\dist\windows-x86_64\rtm-sender.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; DestName: "INSTALL.md"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\rtm-sender"; Filename: "{app}\rtm-sender.exe"
Name: "{group}\Uninstall rtm-sender"; Filename: "{uninstallexe}"

[Tasks]
Name: "autostart"; Description: "Start rtm-sender when I log in"; GroupDescription: "Startup:"; Flags: checkedonce

[Registry]
; Same HKCU Run value the tray's "Start with login" setting manages.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "rtm-sender"; ValueData: """{app}\rtm-sender.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\rtm-sender.exe"; Description: "Start rtm-sender now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "taskkill"; Parameters: "/IM rtm-sender.exe /F"; Flags: runhidden; RunOnceId: "killsender"
