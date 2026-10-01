@echo off
rem Play the current build. Copies the exe first so rebuilding never fights a running game.
cd /d "%~dp0"
if not exist target\debug\gb_game.exe (
  echo Building...
  cargo build -p gb_game || (pause & exit /b 1)
)
if not exist target\play mkdir target\play
rem Remove copies from earlier launches (a still-running one is locked and just stays).
del /q target\play\gb_game-*.exe >nul 2>&1
rem Use a unique copy so an older running session can never lock the path and leave a stale build.
set "PLAY_EXE=target\play\gb_game-%RANDOM%-%RANDOM%.exe"
copy /y target\debug\gb_game.exe "%PLAY_EXE%" >nul || (echo Failed to copy the current build. & pause & exit /b 1)
rem No arguments: start at the main menu (Menu Alley). Otherwise pass them through (e.g. rooftop).
if "%~1"=="" ("%PLAY_EXE%" menu) else ("%PLAY_EXE%" %*)
if errorlevel 1 pause
