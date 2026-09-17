@echo off
rem one-click installer: registers ar-OCG-Router as an auto-start Windows service
rem (service.ps1 asks for elevation through UAC)
setlocal
set "SCRIPT=%~dp0service.ps1"
if not exist "%SCRIPT%" (
  echo service.ps1 not found next to this file.
  pause
  exit /b 1
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%" install -ConfigFile "%~dp0config.yaml"
set "RC=%ERRORLEVEL%"
echo.
if "%RC%"=="0" (echo Done. Service ar-ocg-router installed and started.) else (echo Failed with exit code %RC%.)
pause
exit /b %RC%
