@echo off
echo ========================================
echo   Smart Speaker Launcher
echo ========================================
echo.

:: Check if Ollama is running
echo [1/3] Checking Ollama...
curl -s http://localhost:11434/api/tags >nul 2>&1
if %errorlevel% neq 0 (
    echo       Starting Ollama...
    start "" ollama serve
    timeout /t 3 /nobreak >nul
) else (
    echo       Ollama is already running
)

:: Check if VOICEVOX is running
echo [2/3] Checking VOICEVOX...
curl -s http://localhost:50021/version >nul 2>&1
if %errorlevel% neq 0 (
    echo       Starting VOICEVOX...

    if exist "%LOCALAPPDATA%\Programs\VOICEVOX\VOICEVOX.exe" (
        start "" "%LOCALAPPDATA%\Programs\VOICEVOX\VOICEVOX.exe"
    ) else if exist "C:\Program Files\VOICEVOX\VOICEVOX.exe" (
        start "" "C:\Program Files\VOICEVOX\VOICEVOX.exe"
    ) else (
        echo       [WARNING] VOICEVOX not found. Please start it manually.
    )

    echo       Waiting for VOICEVOX...
    :wait_voicevox
    timeout /t 2 /nobreak >nul
    curl -s http://localhost:50021/version >nul 2>&1
    if %errorlevel% neq 0 goto wait_voicevox
    echo       VOICEVOX ready
) else (
    echo       VOICEVOX is already running
)

echo [3/3] Starting Smart Speaker...
echo.
echo ----------------------------------------
echo Speak into your microphone
echo ----------------------------------------
echo.

cargo run --release

echo.
echo Done.
pause
