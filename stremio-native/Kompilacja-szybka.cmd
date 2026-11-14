@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\Build-Pajdflix.ps1" -Mode Fast %*
pause
