@echo off
setlocal EnableExtensions DisableDelayedExpansion
REM install.bat - install rtm-sender on Windows.
REM
REM   install.bat                 per-user install to %LOCALAPPDATA%\Programs\rtm-sender (no admin prompt)
REM   install.bat /system         all-users install to %ProgramFiles%\rtm-sender (asks for admin)
REM   install.bat /noautostart    do not register start-at-logon (autostart is the default)
REM   install.bat /nostart        do not launch rtm-sender after installing
REM
REM Run it from the folder that contains rtm-sender.exe (dist\windows-x86_64 after build.bat,
REM or the unpacked rtm-sender-windows-x86_64.zip release).
REM The matching uninstall.bat is copied next to the installed exe and listed in Apps & features.
REM
REM Paths may contain spaces and ( ) & ' characters: every path is quoted, nothing that expands
REM a path sits inside a ( ) block (goto labels instead), values with embedded quotes use
REM !DEST! (expanded after cmd has parsed the line), and PowerShell reads paths from
REM environment variables ($env:DEST), never from inline '...' strings.

set "SRC=%~dp0rtm-sender.exe"
if exist "%SRC%" goto :args
echo [install] rtm-sender.exe not found next to this script. Run sender\build.bat first.
exit /b 1

:args
set "SYSTEM=0"
set "AUTOSTART=1"
set "START=1"
set "ELEVATED=0"
set "CHILD=0"
:parse
if "%~1"=="" goto :go
if /I "%~1"=="/system"      set "SYSTEM=1"
if /I "%~1"=="/autostart"   set "AUTOSTART=1"
if /I "%~1"=="/noautostart" set "AUTOSTART=0"
if /I "%~1"=="/nostart"     set "START=0"
if /I "%~1"=="/elevated"    set "CHILD=1"
if /I "%~1"=="/?"           goto :usage
if /I "%~1"=="/help"        goto :usage
shift
goto :parse

:go
net session >nul 2>&1 && set "ELEVATED=1"
REM The elevated child never asks again (net session can fail when the Server service is off).
if "%CHILD%"=="1" set "ELEVATED=1"
if "%SYSTEM%"=="0" set "DEST=%LOCALAPPDATA%\Programs\rtm-sender"
if "%SYSTEM%"=="1" set "DEST=%ProgramFiles%\rtm-sender"
if "%SYSTEM%"=="1" if "%ELEVATED%"=="0" goto :elevate

echo [install] installing to "%DEST%"
if not exist "%DEST%" mkdir "%DEST%"
if exist "%DEST%\" goto :stop
echo [install] cannot create "%DEST%"
goto :fail

:stop
REM Stop a running instance so the exe can be replaced.
tasklist /FI "IMAGENAME eq rtm-sender.exe" 2>nul | find /I "rtm-sender.exe" >nul
if errorlevel 1 goto :copy
echo [install] stopping running rtm-sender
taskkill /IM rtm-sender.exe /F >nul 2>&1
timeout /t 1 /nobreak >nul

:copy
copy /Y "%SRC%" "%DEST%\rtm-sender.exe" >nul
if not errorlevel 1 goto :copied
echo [install] copy failed
goto :fail
:copied
if exist "%~dp0uninstall.bat" copy /Y "%~dp0uninstall.bat" "%DEST%\uninstall.bat" >nul
if exist "%~dp0INSTALL.md"    copy /Y "%~dp0INSTALL.md"    "%DEST%\INSTALL.md" >nul
if exist "%~dp0LICENSE"       copy /Y "%~dp0LICENSE"       "%DEST%\LICENSE" >nul

REM Start Menu shortcut (paths passed to PowerShell through the environment, never inline).
set "SM=%AppData%\Microsoft\Windows\Start Menu\Programs"
if "%SYSTEM%"=="1" set "SM=%ProgramData%\Microsoft\Windows\Start Menu\Programs"
powershell -NoProfile -Command "$s=(New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $env:SM 'rtm-sender.lnk')); $s.TargetPath=(Join-Path $env:DEST 'rtm-sender.exe'); $s.WorkingDirectory=$env:DEST; $s.Description='Remote Task Manager sender'; $s.Save()" >nul 2>&1

set "RUNKEY=HKCU\Software\Microsoft\Windows\CurrentVersion\Run"

REM Autostart: the same HKCU Run value the tray's "Start at login" checkbox uses.
if "%AUTOSTART%"=="0" goto :noautostart
REM The value needs embedded quotes, which leave the path outside a quoted region as far as
REM cmd is concerned. Delayed expansion inserts it after cmd has parsed the line, so special
REM characters in it stay plain text.
setlocal EnableDelayedExpansion
reg add "!RUNKEY!" /v rtm-sender /t REG_SZ /d "\"!DEST!\rtm-sender.exe\"" /f >nul
endlocal
echo [install] registered to start at logon
goto :appsentry
:noautostart
reg delete "%RUNKEY%" /v rtm-sender /f >nul 2>&1

:appsentry
REM Apps & features entry.
set "UK=HKCU"
if "%SYSTEM%"=="1" set "UK=HKLM"
set "VER="
set "VERFILE=%TEMP%\rtm-sender-version-%RANDOM%.txt"
"%DEST%\rtm-sender.exe" --version >"%VERFILE%" 2>nul
for /f "usebackq tokens=2" %%v in ("%VERFILE%") do set "VER=%%v"
del /Q "%VERFILE%" >nul 2>&1
if not defined VER set "VER=unknown"
set "UKEY=%UK%\Software\Microsoft\Windows\CurrentVersion\Uninstall\rtm-sender"
reg add "%UKEY%" /v DisplayName     /t REG_SZ    /d "rtm-sender" /f >nul
reg add "%UKEY%" /v DisplayVersion  /t REG_SZ    /d "%VER%" /f >nul
reg add "%UKEY%" /v InstallLocation /t REG_SZ    /d "%DEST%" /f >nul
reg add "%UKEY%" /v DisplayIcon     /t REG_SZ    /d "%DEST%\rtm-sender.exe" /f >nul
setlocal EnableDelayedExpansion
reg add "!UKEY!" /v UninstallString /t REG_SZ    /d "\"!DEST!\uninstall.bat\"" /f >nul
endlocal
reg add "%UKEY%" /v NoModify        /t REG_DWORD /d 1 /f >nul
reg add "%UKEY%" /v NoRepair        /t REG_DWORD /d 1 /f >nul

echo [install] done. rtm-sender %VER% is in "%DEST%"
echo [install] config: "%APPDATA%\rtm\sender.toml" (created on first run)
if "%START%"=="0" goto :done
echo [install] starting rtm-sender (look for the tray icon)
start "" "%DEST%\rtm-sender.exe"
:done
if "%CHILD%"=="1" timeout /t 5 >nul
exit /b 0

:fail
if "%CHILD%"=="1" pause
exit /b 1

:elevate
echo [install] requesting administrator rights for "%ProgramFiles%"...
set "RTM_SELF=%~f0"
set "RTM_ARGS=/system /elevated"
if "%AUTOSTART%"=="0" set "RTM_ARGS=%RTM_ARGS% /noautostart"
if "%START%"=="0" set "RTM_ARGS=%RTM_ARGS% /nostart"
REM cmd /c ""script" args": the outer quotes are stripped, the script path stays quoted.
REM The child's exit code becomes ours; a declined UAC prompt throws, so PowerShell exits 1.
powershell -NoProfile -Command "$a='/d /c \"\"' + $env:RTM_SELF + '\" ' + $env:RTM_ARGS + '\"'; $p=Start-Process -FilePath $env:ComSpec -ArgumentList $a -Verb RunAs -Wait -PassThru -ErrorAction Stop; exit $p.ExitCode"
exit /b %ERRORLEVEL%

:usage
echo usage: install.bat [/system] [/noautostart] [/nostart]
exit /b 2
