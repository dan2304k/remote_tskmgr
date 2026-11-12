; rtm-sender.iss - Inno Setup 6 script producing rtm-sender-setup.exe
; Compile on Windows with Inno Setup (https://jrsoftware.org/isinfo.php):
;   iscc /DAppVersion=1.2.3 packaging\windows\rtm-sender.iss
; after sender\build.bat has produced dist\windows-x86_64\rtm-sender.exe. Without
; /DAppVersion the version is read from [workspace.package] in sender\Cargo.toml. (The exe
; also carries it in its version resource since 1.0.1; the release workflow passes
; /DAppVersion from the git tag.)

#define AppName "rtm-sender"

#ifndef AppVersion
  #define CargoToml SourcePath + "..\..\sender\Cargo.toml"
  #define AppVersion ""
  #define FileHandle
  #define FileLine
  #define AfterQuote
  ; Takes the first line of the form version = "x.y.z" (rust-version etc. do not match).
  #sub ReadVersionLine
    #define FileLine = Trim(FileRead(FileHandle))
    #if Copy(FileLine, 1, 7) == "version" && Copy(StringChange(Copy(FileLine, 8), " ", ""), 1, 1) == "=" && Pos('"', FileLine) > 0
      #define AfterQuote = Copy(FileLine, Pos('"', FileLine) + 1)
      #define AppVersion = Copy(AfterQuote, 1, Pos('"', AfterQuote) - 1)
    #endif
  #endsub
  #for {FileHandle = FileOpen(CargoToml); FileHandle && !FileEof(FileHandle) && AppVersion == ""; ""} ReadVersionLine
  #if FileHandle
    #expr FileClose(FileHandle)
  #endif
  #if AppVersion == ""
    #error Could not read the version from sender\Cargo.toml; pass /DAppVersion=x.y.z
  #endif
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
; HKA = HKCU for a per-user install (the value the tray's "Start at login" checkbox manages),
; HKLM for an all-users install (every user's login starts the sender).
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "rtm-sender"; ValueData: """{app}\rtm-sender.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\rtm-sender.exe"; Description: "Start rtm-sender now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "taskkill"; Parameters: "/IM rtm-sender.exe /F"; Flags: runhidden; RunOnceId: "killsender"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

{ The tray's "Start at login" checkbox writes HKCU\...\Run\rtm-sender on its own, so the
  uninsdeletevalue flag in the Registry section (only present when the task was ticked) is
  not enough: remove the value unconditionally. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER, RunKey, 'rtm-sender');
    if IsAdminInstallMode then
      RegDeleteValue(HKEY_LOCAL_MACHINE, RunKey, 'rtm-sender');
  end;
end;
