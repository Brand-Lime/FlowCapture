@echo off
rem Pubblica FlowCapture con Velopack. Uso: publish.bat [versione] [altre opzioni]
rem   es. publish.bat 1.0.1
rem       publish.bat 1.0.1 -Mandatory
rem Opzioni e valori predefiniti (cartella di rete, feed aggiornamenti, firma): scripts\publish-velopack.ps1
setlocal
set VERSION=%~1
if not "%VERSION%"=="" shift
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\publish-velopack.ps1" -Version "%VERSION%" %1 %2 %3 %4 %5 %6 %7 %8
exit /b %ERRORLEVEL%
