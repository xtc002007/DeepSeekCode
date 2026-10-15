# Code++ 打包说明

> 三通道分离 + 统一版本。本文档是发布/打包系统的唯一参考手册。

## 1. 架构概览

Code++ 采用 **三通道分离**：CLI、Desktop、Crates 各自独立配置与产物，但 **共享同一 workspace 版本号**。

```
                         git tag v0.2.0
                               │
              ┌────────────────┼────────────────┐
              ▼                ▼                ▼
        通道 A: CLI      通道 B: Desktop   通道 C: Crates
        cargo-dist       NSIS + minisign   release-plz
        (默认关闭)       (Win x64 已启用)  (默认关闭)
              │                │                │
              ▼                ▼                ▼
        GitHub Release   GitHub Release    crates.io
        (tar/zip+脚本)   (NSIS 安装器)     (库 crate)
```

| 通道 | 工具 | 配置文件 | 当前状态 |
|------|------|----------|----------|
| **A — CLI** | [cargo-dist](https://github.com/axodotdev/cargo-dist) | `dist-workspace.toml` | 已配置，`enabled = false` |
| **B — Desktop** | NSIS + minisign | `packaging/desktop/windows/` | **Windows x64 已启用** |
| **C — Crates** | [release-plz](https://github.com/MarcoIeni/release-plz) | `release-plz.toml` | 已配置，`enabled = false` |

**通道开关的唯一真相源**：`packaging/channels.toml`（CI 通过 `scripts/release/read-channels.ps1` 读取）。

**版本唯一真相源**：根目录 `Cargo.toml` → `[workspace.package].version`（当前如 `0.2.0`），对应 git tag `v0.2.0`。

---

## 2. 产物路径（重要）

### 2.1 仅 `cargo build`（裸二进制）

```powershell
cargo build --release -p code-plus-plus-desktop --target x86_64-pc-windows-msvc
```

| 产物 | 路径 |
|------|------|
| **主程序** | `target/x86_64-pc-windows-msvc/release/code-plus-plus-desktop.exe` |
| **依赖 DLL**（若有） | `target/x86_64-pc-windows-msvc/release/*.dll` |
| **调试符号**（若有） | `target/x86_64-pc-windows-msvc/release/code-plus-plus-desktop.pdb` |

> 这是 **未打包的 exe**，不能直接发给用户安装。需要继续执行 NSIS 安装器构建（见 §4）。

若 **未指定 `--target`**（默认本机 triple），则路径为：

```
target/release/code-plus-plus-desktop.exe
```

### 2.2 NSIS 安装器（最终发布物）

```powershell
.\packaging\desktop\windows\build-installer.ps1 -Version 0.2.0
```

| 产物 | 路径 |
|------|------|
| **NSIS 安装器** | `packaging/desktop/windows/dist/CodePlusPlus-0.2.0-x64-setup.exe` |
| **minisign 签名** | `packaging/desktop/windows/dist/CodePlusPlus-0.2.0-x64-setup.exe.minisig` |
| **SHA256 校验和** | `packaging/desktop/windows/dist/CodePlusPlus-0.2.0-x64-setup.sha256` |
| **更新清单** | `packaging/desktop/windows/dist/latest.json` |

安装器默认安装到：`C:\Program Files\Code++\code-plus-plus-desktop.exe`

### 2.3 CI 发布产物

推送 tag 后，产物出现在 **GitHub Releases** 页面（workflow：`.github/workflows/release.yml`）。

---

## 3. 目录与文件索引

```
packaging/
  channels.toml              # 三通道开关（enabled true/false）
  desktop/windows/
    installer.nsi            # NSIS 安装脚本
    build-installer.ps1      # 本地/CI 构建入口
    staging/                 # 临时 staging（gitignore）
    dist/                    # 最终安装器输出（gitignore）

signing/
  README.md                  # 密钥说明
  code-plus-plus.key           # 私钥（gitignore，禁止提交）
  code-plus-plus.key.pub       # 公钥（可提交）

scripts/release/
  generate-signing-key.ps1   # 生成 minisign 密钥对
  verify-signing-key.ps1     # 验证密钥可用
  set-github-secrets.ps1     # 上传 GitHub Secrets
  sign-artifact.ps1          # 签名安装包
  sign-authenticode.ps1      # 可选 Authenticode 签名
  validate-version.ps1       # tag 与 Cargo.toml 版本校验
  read-channels.ps1          # CI 读取通道配置
  create-updater-manifest.ps1

dist-workspace.toml          # cargo-dist 配置（通道 A）
release-plz.toml             # release-plz 配置（通道 C）
.env.example                 # 环境变量模板
.github/workflows/release.yml
```

---

## 4. 本地打包流程（Windows Desktop）

### 前置条件

- Rust stable + MSVC 工具链
- [NSIS](https://nsis.sourceforge.io/)（`makensis`；脚本会自动检测默认安装路径 `C:\Program Files (x86)\NSIS\`）
- [minisign](https://github.com/jedisct1/minisign)（签名用，可选但推荐）

若未安装 NSIS：

```powershell
.\scripts\release\install-nsis.ps1
# 或: winget install NSIS.NSIS
```

### 步骤

```powershell
# ① 构建 release 二进制
cargo build --release -p code-plus-plus-desktop --target x86_64-pc-windows-msvc

# 确认 exe 存在
dir target\x86_64-pc-windows-msvc\release\code-plus-plus-desktop.exe

# ② 打 NSIS 安装器（版本号与 Cargo.toml 一致）
.\packaging\desktop\windows\build-installer.ps1 -Version 0.2.0

# ③ 查看最终产物
dir packaging\desktop\windows\dist\
```

或使用快捷脚本：

```bat
scripts\build-desktop.bat
```

---

## 5. 签名与密钥

采用 **minisign ed25519** 对安装包签名，产出 `.minisig`  sidecar 文件，供后续自动更新校验。

### 5.0 Secret scan (before every release)

```powershell
.\scripts\release\scan-secrets.ps1
```

CI runs [gitleaks](https://github.com/gitleaks/gitleaks) on every push/PR (`.github/workflows/secret-scan.yml`). If a signing key was ever committed, rotate it before publishing — see [SECURITY.md](../SECURITY.md).

### 5.1 密钥文件

| 文件 | 用途 | Git |
|------|------|-----|
| `signing/code-plus-plus.key` | 私钥（签名） | **禁止提交** |
| `signing/code-plus-plus.key.pub` | 公钥（校验） | **可提交** |

### 5.2 首次生成

```powershell
.\scripts\release\generate-signing-key.ps1
copy .env.example .env
# 编辑 .env 填写 CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PASSWORD
.\scripts\release\verify-signing-key.ps1
```

### 5.3 环境变量（`.env`）

```env
CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PATH=signing/code-plus-plus.key
CODE_PLUS_PLUS_SIGNING_PUBLIC_KEY_PATH=signing/code-plus-plus.key.pub
CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PASSWORD=<密码，无密码则留空>
```

### 5.4 GitHub Secrets（CI 必需）

```powershell
.\scripts\release\set-github-secrets.ps1
```

| Secret | 必需 | 说明 |
|--------|------|------|
| `CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY` | ✅ | 私钥文件全文 |
| `CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PASSWORD` | ✅ | 私钥密码（无密码则设空 secret） |
| `CODE_PLUS_PLUS_SIGNING_PUBLIC_KEY` | ✅ | 公钥文件全文 |
| `CODE_PLUS_PLUS_WINDOWS_PFX_BASE64` | 可选 | Authenticode 证书（Base64 PFX） |
| `CODE_PLUS_PLUS_WINDOWS_PFX_PASSWORD` | 可选 | PFX 密码 |

> **CI 无签名密钥会直接失败**，不会发布未签名安装包。

---

## 6. CI 自动发布

### 触发方式

```bash
# 方式一：推送 tag（版本须与 Cargo.toml 一致）
git tag v0.2.0
git push origin v0.2.0

# 方式二：手动触发
gh workflow run Release -f tag_name=v0.2.0
```

### CI 流程

```
validate（版本校验）
    → desktop-windows（构建 exe → NSIS → minisign → latest.json）
    → publish-release（上传 GitHub Release）
```

CLI / Crates job 已预留，当前 `enabled = false` 时自动跳过。

### 版本校验

```powershell
.\scripts\release\validate-version.ps1 -TagName v0.2.0
# tag v0.2.0 必须等于 Cargo.toml 中的 0.2.0
```

---

## 7. 启用其他通道

### 7.1 CLI（cargo-dist）

1. `packaging/channels.toml` → `[channels.cli] enabled = true`
2. `dist-workspace.toml` → 取消注释 `targets`
3. `cargo install cargo-dist --locked && dist init --yes`
4. 完善 `.github/workflows/release.yml` 中 `cli-dist` job

### 7.2 Desktop 其他平台

在 `packaging/channels.toml` 对应 platform 设 `enabled = true`，并在 `release.yml` 添加 job（macOS dmg / Linux deb 脚本待实现）。

### 7.3 Crates.io

1. `packaging/channels.toml` → `[channels.crates] enabled = true`
2. `release-plz.toml` → 目标 crate 设 `publish = true`
3. 添加 `CARGO_REGISTRY_TOKEN` secret

---

## 8. 版本约定

| 项 | 规则 |
|----|------|
| workspace 版本 | `Cargo.toml` → `[workspace.package].version` |
| git tag | `v` + 版本号，如 `v0.2.0` |
| 安装器文件名 | `CodePlusPlus-{version}-x64-setup.exe` |
| CLI 二进制名 | `code-plus-plus`（crate 名仍为 `code-plus-plus-cli`） |
| Desktop 二进制名 | `code-plus-plus-desktop` |

---

## 9. 常见问题

**Q: `cargo build` 后找不到 exe？**

确认使用了 `--target x86_64-pc-windows-msvc`，路径在 `target/x86_64-pc-windows-msvc/release/`，而非 `target/release/`。

**Q: `makensis not found`？**

NSIS 可能已安装但未加入 PATH。`build-installer.ps1` 会自动查找 `C:\Program Files (x86)\NSIS\makensis.exe`。若仍未找到，运行 `.\scripts\release\install-nsis.ps1` 或 `winget install NSIS.NSIS`。

**Q: CI Release 失败提示缺少 signing secret？**

运行 `.\scripts\release\set-github-secrets.ps1` 上传密钥。

**Q: 裸 exe 和安装器的区别？**

- `target/.../code-plus-plus-desktop.exe` — 开发/调试用的裸二进制
- `packaging/.../dist/*-setup.exe` — 带安装/卸载/开始菜单的 NSIS 安装器，**这才是发给用户的发布物**

**Q: 如何跳过签名本地测试？**

```powershell
.\packaging\desktop\windows\build-installer.ps1 -Version 0.2.0 -SkipSign
```
