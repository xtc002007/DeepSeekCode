# Release

完整打包说明见 **[docs/packaging.md](packaging.md)**（架构、产物路径、密钥、本地/CI 用法、通道启用等）。

快速入口：

```powershell
# 裸二进制
cargo build --release -p deepseekcode-desktop --target x86_64-pc-windows-msvc
# → target/x86_64-pc-windows-msvc/release/deepseekcode.exe

# NSIS 安装器
.\packaging\desktop\windows\build-installer.ps1 -Version 0.1.0
# → packaging/desktop/windows/dist/DeepSeekCode-0.1.0-x64-setup.exe
```
