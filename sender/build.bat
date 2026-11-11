@echo off
setlocal EnableExtensions
REM build.bat - build rtm-sender on Windows and copy the result to ..\dist\windows-x86_64\
REM
REM   build.bat              release build with the tray icon + settings window (gui feature)
REM   build.bat headless     release build without the GUI (console daemon, smaller)
REM   build.bat debug        debug build with gui
REM   build.bat test         run the test suites (headless workspace + rtm-sender with gui), no copy to dist
REM   build.bat clean        remove target\ and ..\dist\windows-x86_64\
REM
REM Needs Rust (https://rustup.rs). The gui feature needs rustc 1.95 or newer; run "rustup update"
REM if cargo complains about rust-version. No other tools or libraries are required on Windows.

set "SENDER_DIR=%~dp0"
set "SENDER_DIR=%SENDER_DIR:~0,-1%"
set "REPO_DIR=%SENDER_DIR%\.."
set "DIST_DIR=%REPO_DIR%\dist\windows-x86_64"

set "PROFILE=release"
set "PROFILE_FLAG=--release"
set "FEATURES=--features gui"
set "VARIANT=gui"

:parse
if "%~1"=="" goto :checks
if /I "%~1"=="headless" ( set "FEATURES=" & set "VARIANT=headless" & shift & goto :parse )
if /I "%~1"=="debug"    ( set "PROFILE=debug" & set "PROFILE_FLAG=" & shift & goto :parse )
if /I "%~1"=="clean"    goto :test
where cargo >nul 2>nul || ( echo [build] cargo not found. Install Rust from https://rustup.rs & exit /b 1 )
pushd "%SENDER_DIR%" || exit /b 1
echo [test] cargo test --workspace
cargo test --workspace
if errorlevel 1 ( echo [test] workspace tests FAILED & popd & exit /b 1 )
echo [test] cargo test -p rtm-sender --features gui
cargo test -p rtm-sender --features gui
if errorlevel 1 ( echo [test] gui tests FAILED & popd & exit /b 1 )
popd
echo [test] all tests passed
exit /b 0

:clean
if /I "%~1"=="test"     goto :test
if /I "%~1"=="help"     goto :usage
if /I "%~1"=="-h"       goto :usage
if /I "%~1"=="/?"       goto :usage
echo Unknown option: %~1
goto :usage

:checks
where cargo >nul 2>nul
if errorlevel 1 (
  echo [build] cargo not found. Install Rust from https://rustup.rs and reopen this terminal.
  exit /b 1
)
for /f "tokens=2" %%v in ('rustc --version') do set "RUSTC_VER=%%v"
echo [build] rustc %RUSTC_VER%, profile=%PROFILE%, variant=%VARIANT%

pushd "%SENDER_DIR%" || exit /b 1
cargo build %PROFILE_FLAG% -p rtm-sender %FEATURES%
if errorlevel 1 (
  echo [build] cargo build failed.
  popd
  exit /b 1
)
popd

set "EXE=%SENDER_DIR%\target\%PROFILE%\rtm-sender.exe"
if not exist "%EXE%" (
  echo [build] expected output not found: %EXE%
  exit /b 1
)

if not exist "%DIST_DIR%" mkdir "%DIST_DIR%"
copy /Y "%EXE%" "%DIST_DIR%\rtm-sender.exe" >nul || exit /b 1
copy /Y "%REPO_DIR%\packaging\README.md" "%DIST_DIR%\INSTALL.md" >nul
copy /Y "%REPO_DIR%\LICENSE" "%DIST_DIR%\LICENSE" >nul

copy /Y "%REPO_DIR%\packaging\windows\install.bat" "%DIST_DIR%\install.bat" >nul
copy /Y "%REPO_DIR%\packaging\windows\uninstall.bat" "%DIST_DIR%\uninstall.bat" >nul

for %%F in ("%DIST_DIR%\rtm-sender.exe") do set "SIZE=%%~zF"
echo [build] done: %DIST_DIR%\rtm-sender.exe (%SIZE% bytes, %VARIANT%)
echo [build] install: "%DIST_DIR%\install.bat" [/system] [/noautostart]   ^(or run the exe in place^)
echo [build] test it: "%DIST_DIR%\rtm-sender.exe" --once     ^(prints one JSON packet^)
exit /b 0

:test
where cargo >nul 2>nul || ( echo [build] cargo not found. Install Rust from https://rustup.rs & exit /b 1 )
pushd "%SENDER_DIR%" || exit /b 1
echo [test] cargo test --workspace
cargo test --workspace
if errorlevel 1 ( echo [test] workspace tests FAILED & popd & exit /b 1 )
echo [test] cargo test -p rtm-sender --features gui
cargo test -p rtm-sender --features gui
if errorlevel 1 ( echo [test] gui tests FAILED & popd & exit /b 1 )
popd
echo [test] all tests passed
exit /b 0

:clean
echo [build] removing %SENDER_DIR%\target and %DIST_DIR%
if exist "%SENDER_DIR%\target" rmdir /S /Q "%SENDER_DIR%\target"
if exist "%DIST_DIR%" rmdir /S /Q "%DIST_DIR%"
exit /b 0

:usage
echo usage: build.bat [headless] [debug] ^| test ^| clean ^| help
exit /b 2
