@echo off
REM Build DeepSeekCode desktop for Windows (requires Rust + MSVC)
cd /d "%~dp0.."
cargo build --release -p deepseekcode-desktop --target x86_64-pc-windows-msvc
echo Binary: target\x86_64-pc-windows-msvc\release\deepseekcode.exe
echo Installer: powershell -File packaging\desktop\windows\build-installer.ps1 -Version 0.1.0
