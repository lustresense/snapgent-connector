@echo off
setlocal
cd /d "%~dp0"
call "%~dp0start-secure.bat"
exit /b %errorlevel%
