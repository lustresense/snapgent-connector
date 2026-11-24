@echo off
setlocal
cd /d "%~dp0"
if not exist "%~dp0dist\native\Snapgent.exe" (
    echo Native Snapgent executable not found.
    echo Build it using development\build-native.ps1 first.
    pause
    exit /b 1
)
start "Snapgent" "%~dp0dist\native\Snapgent.exe"
