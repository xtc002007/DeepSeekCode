# Release

完整打包说明见 **[docs/packaging.md](packaging.md)**（架构、产物路径、密钥、本地/CI 用法、通道启用等）。
当前 workspace 版本见根 `Cargo.toml`（`[workspace.package] version`）。

快速入口：

```powershell
# 裸二进制（P1-12: desktop 二进制已改名，避免与 CLI 覆盖）
cargo build --release -p code-plus-plus-desktop --target x86_64-pc-windows-msvc
# → target/x86_64-pc-windows-msvc/release/code-plus-plus-desktop.exe

# NSIS 安装器（版本号与 Cargo.toml 保持一致，例如 0.2.0）
.\packaging\desktop\windows\build-installer.ps1 -Version 0.2.0
# → packaging/desktop/windows/dist/CodePlusPlus-0.2.0-x64-setup.exe
```
