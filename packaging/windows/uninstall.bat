@echo off
setlocal EnableExtensions DisableDelayedExpansion
REM uninstall.bat - remove rtm-sender (installed by install.bat).
REM
REM   uninstall.bat           remove the program, keep %APPDATA%\rtm\sender.toml
REM   uninstall.bat /purge    also remove the config folder %APPDATA%\rtm
REM
REM Asks for administrator rights when the install folder is under Program Files.
REM Like install.bat: paths are always quoted and never expanded inside ( ) blocks.
set "DEST=%~dp0"
set "DEST=%DEST:~0,-1%"
set "PURGE=0"
set "CHILD=0"
:parse
if "%~1"=="" goto :go
if /I "%~1"=="/purge"    set "PURGE=1"
if /I "%~1"=="/elevated" set "CHILD=1"
shift
goto :parse

:go
REM An all-users install (install.bat /system) lives under Program Files and is registered
REM in HKLM: both need admin. DEST is a prefix of P exactly when P plus what follows the
REM first occurrence of P in DEST gives DEST back.
set "UNDERPF=0"
setlocal EnableDelayedExpansion
for %%P in ("%ProgramFiles%" "%ProgramW6432%") do if not "%%~P"=="" (
  set "REST=!DEST:*%%~P\=!"
  if /I "%%~P\!REST!"=="!DEST!" set "UNDERPF=1"
)
endlocal & set "UNDERPF=%UNDERPF%"
if "%UNDERPF%"=="0" goto :remove
net session >nul 2>&1
if not errorlevel 1 goto :remove
if "%CHILD%"=="1" goto :remove

echo [uninstall] requesting administrator rights for "%DEST%"...
set "RTM_SELF=%~f0"
set "RTM_ARGS=/elevated"
if "%PURGE%"=="1" set "RTM_ARGS=/elevated /purge"
REM Same pattern as install.bat: cmd /c ""script" args", child exit code propagated.
powershell -NoProfile -Command "$a='/d /c \"\"' + $env:RTM_SELF + '\" ' + $env:RTM_ARGS + '\"'; $p=Start-Process -FilePath $env:ComSpec -ArgumentList $a -Verb RunAs -Wait -PassThru -ErrorAction Stop; exit $p.ExitCode"
exit /b %ERRORLEVEL%

:remove
echo [uninstall] removing rtm-sender from "%DEST%"
taskkill /IM rtm-sender.exe /F >nul 2>&1
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v rtm-sender /f >nul 2>&1
REM Logon tasks: "rtm-sender" is the documented name; "RTM Sender" was used by older docs.
schtasks /delete /tn "rtm-sender" /f >nul 2>&1
schtasks /delete /tn "RTM Sender" /f >nul 2>&1
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\rtm-sender" /f >nul 2>&1
reg delete "HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\rtm-sender" /f >nul 2>&1
del /Q "%AppData%\Microsoft\Windows\Start Menu\Programs\rtm-sender.lnk" >nul 2>&1
del /Q "%ProgramData%\Microsoft\Windows\Start Menu\Programs\rtm-sender.lnk" >nul 2>&1
if "%PURGE%"=="0" goto :rmdir
rmdir /S /Q "%APPDATA%\rtm" >nul 2>&1
echo [uninstall] removed config "%APPDATA%\rtm"

:rmdir
echo [uninstall] done
if "%CHILD%"=="1" timeout /t 3 >nul
REM Delete the install folder after this script has exited (it runs from inside it). Neither
REM this cmd nor the helper may have the folder as its current directory, or rmdir fails.
cd /d "%TEMP%"
start "" /B /D "%TEMP%" cmd /D /C ping -n 3 127.0.0.1 ^>nul ^& rmdir /S /Q "%DEST%"
exit /b 0
