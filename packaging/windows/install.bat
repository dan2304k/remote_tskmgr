@echo off
setlocal EnableExtensions
REM install.bat - install rtm-sender on Windows.
REM
REM   install.bat                 per-user install to %LOCALAPPDATA%\Programs\rtm-sender (no admin prompt)
REM   install.bat /system         all-users install to %ProgramFiles%\rtm-sender (asks for admin)
REM   install.bat /noautostart    do not register start-at-logon (autostart is the default)
REM   install.bat /nostart        do not launch rtm-sender after installing
REM
REM Run it from the folder that contains rtm-sender.exe (dist\windows-x86_64 after build.bat).
REM The matching uninstall.bat is copied next to the installed exe and listed in Apps & features.

set "SRC=%~dp0rtm-sender.exe"
if not exist "%SRC%" (
  echo [install] rtm-sender.exe not found next to this script. Run sender\build.bat first.
  exit /b 1
)

set "SYSTEM=0"
set "AUTOSTART=1"
set "START=1"
set "ELEVATED=0"
:parse
if "%~1"=="" goto :go
if /I "%~1"=="/system"    set "SYSTEM=1"
if /I "%~1"=="/autostart"   set "AUTOSTART=1"
if /I "%~1"=="/noautostart" set "AUTOSTART=0"
if /I "%~1"=="/nostart"   set "START=0"
if /I "%~1"=="/elevated"  set "ELEVATED=1"
if /I "%~1"=="/?"         goto :usage
if /I "%~1"=="/help"      goto :usage
shift
goto :parse

:go
net session >nul 2>&1 && set "ELEVATED=1"
if "%SYSTEM%"=="0" set "DEST=%LOCALAPPDATA%\Programs\rtm-sender"
if "%SYSTEM%"=="1" set "DEST=%ProgramFiles%\rtm-sender"
if "%SYSTEM%"=="1" if "%ELEVATED%"=="0" goto :elevate

echo [install] installing to "%DEST%"
if not exist "%DEST%" mkdir "%DEST%"
if not exist "%DEST%" ( echo [install] cannot create "%DEST%" & exit /b 1 )

REM Stop a running instance so the exe can be replaced.
tasklist /FI "IMAGENAME eq rtm-sender.exe" 2>nul | find /I "rtm-sender.exe" >nul
if not errorlevel 1 (
  echo [install] stopping running rtm-sender
  taskkill /IM rtm-sender.exe /F >nul 2>&1
  timeout /t 1 /nobreak >nul
)

copy /Y "%SRC%" "%DEST%\rtm-sender.exe" >nul
if errorlevel 1 ( echo [install] copy failed & exit /b 1 )
if exist "%~dp0uninstall.bat" copy /Y "%~dp0uninstall.bat" "%DEST%\uninstall.bat" >nul
if exist "%~dp0INSTALL.md"    copy /Y "%~dp0INSTALL.md"    "%DEST%\INSTALL.md" >nul
if exist "%~dp0LICENSE"       copy /Y "%~dp0LICENSE"       "%DEST%\LICENSE" >nul

REM Start Menu shortcut.
set "SM=%AppData%\Microsoft\Windows\Start Menu\Programs"
if "%SYSTEM%"=="1" set "SM=%ProgramData%\Microsoft\Windows\Start Menu\Programs"
powershell -NoProfile -Command "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('%SM%\rtm-sender.lnk'); $s.TargetPath='%DEST%\rtm-sender.exe'; $s.WorkingDirectory='%DEST%'; $s.Description='Remote Task Manager sender'; $s.Save()" >nul 2>&1

REM Autostart: the same HKCU Run value the tray's "Start with login" setting uses.
if "%AUTOSTART%"=="1" (
  reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v rtm-sender /t REG_SZ /d "\"%DEST%\rtm-sender.exe\"" /f >nul
  echo [install] registered to start at logon
) else (
  reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v rtm-sender /f >nul 2>&1
)

REM Apps & features entry.
set "UK=HKCU"
if "%SYSTEM%"=="1" set "UK=HKLM"
set "VER="
for /f "tokens=2" %%v in ('""%DEST%\rtm-sender.exe" --version" 2^>nul') do set "VER=%%v"
if not defined VER set "VER=unknown"
set "UKEY=%UK%\Software\Microsoft\Windows\CurrentVersion\Uninstall\rtm-sender"
reg add "%UKEY%" /v DisplayName     /t REG_SZ    /d "rtm-sender" /f >nul
reg add "%UKEY%" /v DisplayVersion  /t REG_SZ    /d "%VER%" /f >nul
reg add "%UKEY%" /v InstallLocation /t REG_SZ    /d "%DEST%" /f >nul
reg add "%UKEY%" /v DisplayIcon     /t REG_SZ    /d "%DEST%\rtm-sender.exe" /f >nul
reg add "%UKEY%" /v UninstallString /t REG_SZ    /d "\"%DEST%\uninstall.bat\"" /f >nul
reg add "%UKEY%" /v NoModify        /t REG_DWORD /d 1 /f >nul
reg add "%UKEY%" /v NoRepair        /t REG_DWORD /d 1 /f >nul

echo [install] done. rtm-sender %VER% is in "%DEST%"
echo [install] config: %APPDATA%\rtm\sender.toml ^(created on first run^)
if "%START%"=="1" (
  echo [install] starting rtm-sender ^(look for the tray icon^)
  start "" "%DEST%\rtm-sender.exe"
)
exit /b 0

:elevate
echo [install] requesting administrator rights for "%ProgramFiles%"...
set "ARGS=/system /elevated"
if "%AUTOSTART%"=="0" set "ARGS=%ARGS% /noautostart"
if "%START%"=="0" set "ARGS=%ARGS% /nostart"
powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList '%ARGS%' -Verb RunAs -Wait"
exit /b

:usage
echo usage: install.bat [/system] [/noautostart] [/nostart]
exit /b 2
