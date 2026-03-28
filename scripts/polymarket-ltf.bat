@echo off
setlocal

set "SCRIPT_DIR=%~dp0"
for %%I in ("%SCRIPT_DIR%..") do set "APP_DIR=%%~fI"
set "PID_FILE=%APP_DIR%\.run\polymarket-ltf.pid"
set "BIN_PATH=%APP_DIR%\polymarket-ltf.exe"

if "%~1"=="" goto usage
if /I "%~1"=="start" goto start
if /I "%~1"=="stop" goto stop
if /I "%~1"=="status" goto status
goto usage

:start
if exist "%PID_FILE%" (
  for /f %%p in (%PID_FILE%) do tasklist /FI "PID eq %%p" | find "%%p" >nul && (
    echo polymarket-ltf is already running: pid=%%p
    exit /b 1
  )
)

if not exist "%BIN_PATH%" (
  echo binary not found: %BIN_PATH%
  exit /b 1
)

if not exist "%APP_DIR%\.run" mkdir "%APP_DIR%\.run"
start "" /B cmd /c ""%BIN_PATH%" %* >NUL 2>&1"
for /f "tokens=2" %%p in ('tasklist /FI "IMAGENAME eq polymarket-ltf.exe" /NH ^| find "polymarket-ltf.exe"') do (
  > "%PID_FILE%" echo %%p
  echo started: pid=%%p
  exit /b 0
)

echo failed to detect started process
exit /b 1

:stop
if not exist "%PID_FILE%" (
  echo polymarket-ltf is not running
  exit /b 0
)

for /f %%p in (%PID_FILE%) do (
  taskkill /PID %%p >nul 2>&1
  echo stopping polymarket-ltf: pid=%%p
)
del /Q "%PID_FILE%" >nul 2>&1
exit /b 0

:status
if not exist "%PID_FILE%" (
  echo polymarket-ltf is not running
  exit /b 0
)

for /f %%p in (%PID_FILE%) do (
  tasklist /FI "PID eq %%p" | find "%%p" >nul && (
    echo polymarket-ltf is running: pid=%%p
    exit /b 0
  )
)

echo polymarket-ltf is not running
del /Q "%PID_FILE%" >nul 2>&1
exit /b 0

:usage
echo usage:
echo   scripts\polymarket-ltf.bat start [args...]
echo   scripts\polymarket-ltf.bat stop
echo   scripts\polymarket-ltf.bat status
exit /b 1
