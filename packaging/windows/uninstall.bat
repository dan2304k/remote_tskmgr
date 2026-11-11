@echo off
setlocal EnableExtensions
REM uninstall.bat - remove rtm-sender (installed by install.bat).
REM Keeps %APPDATA%\rtm\sender.toml unless /purge is given.
set "DEST=%~dp0"
set "DEST=%DEST:~0,-1%"
set "PURGE=0"
if /I "%~1"=="/purge" set "PURGE=1"

echo [uninstall] removing rtm-sender from "%DEST%"
taskkill /IM rtm-sender.exe /F >nul 2>&1
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v rtm-sender /f >nul 2>&1
schtasks /delete /tn rtm-sender /f >nul 2>&1
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\rtm-sender" /f >nul 2>&1
reg delete "HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\rtm-sender" /f >nul 2>&1
del /Q "%AppData%\Microsoft\Windows\Start Menu\Programs\rtm-sender.lnk" >nul 2>&1
del /Q "%ProgramData%\Microsoft\Windows\Start Menu\Programs\rtm-sender.lnk" >nul 2>&1
if "%PURGE%"=="1" (
  rmdir /S /Q "%APPDATA%\rtm" >nul 2>&1
  echo [uninstall] removed config %APPDATA%\rtm
)
echo [uninstall] done
REM Delete the install folder after this script has exited (it runs from inside it).
start "" /B cmd /C timeout /t 1 /nobreak ^>nul ^& rmdir /S /Q "%DEST%"
exit /b 0
