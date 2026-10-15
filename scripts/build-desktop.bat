@echo off
REM Build CodePlusPlus desktop for Windows (requires Rust + MSVC)
REM Usage: build-desktop.bat [x86_64-pc-windows-msvc ^| aarch64-pc-windows-msvc]
cd /d "%~dp0.."
if "%~1"=="" ( set RUST_TARGET=x86_64-pc-windows-msvc ) else ( set RUST_TARGET=%~1 )
cargo build --release -p code-plus-plus-desktop --target %RUST_TARGET%
echo Binary: target\%RUST_TARGET%\release\code-plus-plus-desktop.exe
echo Installer: powershell -File packaging\desktop\windows\build-installer.ps1 -Version ^<workspace version in Cargo.toml^>
