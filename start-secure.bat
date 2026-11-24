@echo off
setlocal DisableDelayedExpansion
chcp 65001 >nul
title Snapgent Secure Bridge
cd /d "%~dp0"
set "SECURITY=%LOCALAPPDATA%\SnapgentSecurity"
set "PYTHON=%SECURITY%\venv\Scripts\python.exe"
if not exist "%PYTHON%" (
    py.exe -3 -m venv "%SECURITY%\venv"
    if errorlevel 1 goto failed
)
"%PYTHON%" -c "import aiohttp, websockets" >nul 2>nul
if errorlevel 1 (
    "%PYTHON%" -m pip --isolated install --index-url https://pypi.org/simple --only-binary=:all: -r requirements-secure.lock
    if errorlevel 1 goto failed
)
"%PYTHON%" -m pip check
if errorlevel 1 goto failed
"%PYTHON%" secure_start.py
if errorlevel 1 goto failed
exit /b 0
:failed
echo Secure bridge failed to start. Existing processes were not terminated.
pause
exit /b 1
