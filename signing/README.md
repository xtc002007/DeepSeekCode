# Code++ release signing keys

本目录存放 **minisign ed25519** 签名密钥，用于 Release 安装包与更新校验。

| 文件 | 说明 | Git |
|------|------|-----|
| `code-plus-plus.key` | 私钥（仅本地 / CI Secret） | **禁止提交** |
| `code-plus-plus.key.pub` | 公钥（客户端 / 文档校验） | **可提交** |

## 首次生成

```powershell
.\scripts\release\generate-signing-key.ps1
```

或在已安装 [minisign](https://github.com/jedisct1/minisign) 时：

```powershell
minisign -G -p signing/code-plus-plus.key.pub -s signing/code-plus-plus.key -W
```

## `.env` 配置

复制 `.env.example` 为 `.env` 并填写：

```env
CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PATH=signing/code-plus-plus.key
CODE_PLUS_PLUS_SIGNING_PUBLIC_KEY_PATH=signing/code-plus-plus.key.pub
CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PASSWORD=<your-password-or-empty>
```

## 口令轮换与 CI 说明

- 本地 `.env` 中的 `CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PASSWORD` 仅用于本地签名，
  **绝不提交**（`.gitignore` 已覆盖 `.env`）。若疑似泄露，立即生成新的强口令
  （≥32 位随机字符）并同步更新 GitHub Secret。
- CI 通过 Repository Secrets
  (`CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY[_PASSWORD]` 等) 注入，签名脚本经临时文件
  管道传递口令（`release-env.psm1::Invoke-MinisignSign`），**不在命令行参数与
  日志中回显**。

## 本地私钥文件权限（S2）

- `signing/*.key` / `*.pfx` 已被 `.gitignore` 覆盖（本机 `git check-ignore`
  确认 `signing/code-plus-plus.key` 被忽略），禁止提交；仓库只保留
  `code-plus-plus.key.pub`。
- 私钥文件应仅本机可读。Windows 下限制 ACL 示例：

```powershell
icacls signing\code-plus-plus.key /inheritance:r /grant:r "$env:USERNAME:(R,W)"
```

- 备选：将私钥移出仓库目录（如 `%USERPROFILE%\.code-plus-plus\signing\`），
  并把 `.env` 中的 `CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PATH` 指向绝对路径。
- 发布前必跑 `.\scripts\release\scan-secrets.ps1`（gitleaks 全仓扫描）；
  若私钥曾被提交，立即轮换密钥并重新签名全部产物（见 SECURITY.md）。

## 验证与上传 CI Secrets

```powershell
# 验证密钥可用
.\scripts\release\verify-signing-key.ps1

# 上传到 GitHub Actions（需 gh auth login）
.\scripts\release\set-github-secrets.ps1
```

上传的 Secrets：

| Secret | 内容 |
|--------|------|
| `CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY` | 私钥文件全文 |
| `CODE_PLUS_PLUS_SIGNING_PRIVATE_KEY_PASSWORD` | 私钥密码（无密码则空） |
| `CODE_PLUS_PLUS_SIGNING_PUBLIC_KEY` | 公钥文件全文 |

可选 Authenticode（Windows 安装器受信任签名）：

| Secret | 内容 |
|--------|------|
| `CODE_PLUS_PLUS_WINDOWS_PFX_BASE64` | PFX 证书 Base64 |
| `CODE_PLUS_PLUS_WINDOWS_PFX_PASSWORD` | PFX 密码 |

See [SECURITY.md](SECURITY.md) and [docs/packaging.md](packaging.md) §5 before first release. Run `.\scripts\release\scan-secrets.ps1` and configure GitHub Secrets via `.\scripts\release\set-github-secrets.ps1`.
