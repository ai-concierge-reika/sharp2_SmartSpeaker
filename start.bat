@echo off
setlocal

echo ============================================
echo   Smart Speaker Launcher
echo ============================================
echo.

:: Path settings
set OLLAMA_PATH=C:\Users\xxxxx\AppData\Local\Programs\Ollama\ollama.exe
set VOICEVOX_PATH=C:\Program Files\VOICEVOX\VOICEVOX.exe
set PROJECT_DIR=%~dp0

:: Check Ollama
echo [1/3] Checking Ollama...
curl -s http://localhost:11434/api/tags >nul 2>&1
if %errorlevel% equ 0 (
    echo       Ollama is already running
) else (
    echo       Starting Ollama...
    start "" "%OLLAMA_PATH%" serve
    timeout /t 3 /nobreak >nul
)

:: Check VOICEVOX
echo [2/3] Checking VOICEVOX...
curl -s http://localhost:50021/version >nul 2>&1
if %errorlevel% equ 0 (
    echo       VOICEVOX is already running
) else (
    echo       Starting VOICEVOX...
    start "" "%VOICEVOX_PATH%"
    echo       Waiting for VOICEVOX to start...
    :wait_voicevox
    timeout /t 2 /nobreak >nul
    curl -s http://localhost:50021/version >nul 2>&1
    if %errorlevel% neq 0 goto wait_voicevox
    echo       VOICEVOX is ready
)

:: Wait for services
echo.
echo Verifying external services...
timeout /t 2 /nobreak >nul

:: Final check
echo.
echo ============================================
echo   Service Status
echo ============================================
curl -s http://localhost:11434/api/tags >nul 2>&1
if %errorlevel% equ 0 (
    echo   Ollama:   OK
) else (
    echo   Ollama:   FAILED
    pause
    exit /b 1
)

curl -s http://localhost:50021/version >nul 2>&1
if %errorlevel% equ 0 (
    echo   VOICEVOX: OK
) else (
    echo   VOICEVOX: FAILED
    pause
    exit /b 1
)

echo ============================================
echo.

:: Start Smart Speaker
echo [3/3] Starting Smart Speaker...
echo.
cd /d "%PROJECT_DIR%"
cargo run --release

pause
