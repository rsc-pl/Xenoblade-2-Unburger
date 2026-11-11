@echo off
setlocal EnableExtensions
set "ROOT=%~dp0"
if not exist "%ROOT%toolset\launcher.ps1" (
  echo ERROR: Missing Windows build launcher in toolset. 1>&2
  exit /b 1
)
where powershell.exe >nul 2>nul
if errorlevel 1 (
  echo ERROR: Windows PowerShell not found. 1>&2
  exit /b 127
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%toolset\launcher.ps1" %*
exit /b %errorlevel%
