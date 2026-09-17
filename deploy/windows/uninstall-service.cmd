@echo off
rem stops and removes the ar-OCG-Router Windows service
setlocal
set "SCRIPT=%~dp0service.ps1"
powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%" uninstall
set "RC=%ERRORLEVEL%"
echo.
if "%RC%"=="0" (echo Done. Service removed.) else (echo Failed with exit code %RC%.)
pause
exit /b %RC%
