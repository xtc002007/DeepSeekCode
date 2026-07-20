# DeepSeekCode release signing keys

本目录存放 **minisign ed25519** 签名密钥，用于 Release 安装包与更新校验。

| 文件 | 说明 | Git |
|------|------|-----|
| `deepseekcode.key` | 私钥（仅本地 / CI Secret） | **禁止提交** |
| `deepseekcode.key.pub` | 公钥（客户端 / 文档校验） | **可提交** |

## 首次生成

```powershell
.\scripts\release\generate-signing-key.ps1
```

或在已安装 [minisign](https://github.com/jedisct1/minisign) 时：

```powershell
minisign -G -p signing/deepseekcode.key.pub -s signing/deepseekcode.key -W
```

## `.env` 配置

复制 `.env.example` 为 `.env` 并填写：

```env
DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PATH=signing/deepseekcode.key
DEEPSEEKCODE_SIGNING_PUBLIC_KEY_PATH=signing/deepseekcode.key.pub
DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PASSWORD=<your-password-or-empty>
```

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
| `DEEPSEEKCODE_SIGNING_PRIVATE_KEY` | 私钥文件全文 |
| `DEEPSEEKCODE_SIGNING_PRIVATE_KEY_PASSWORD` | 私钥密码（无密码则空） |
| `DEEPSEEKCODE_SIGNING_PUBLIC_KEY` | 公钥文件全文 |

可选 Authenticode（Windows 安装器受信任签名）：

| Secret | 内容 |
|--------|------|
| `DEEPSEEKCODE_WINDOWS_PFX_BASE64` | PFX 证书 Base64 |
| `DEEPSEEKCODE_WINDOWS_PFX_PASSWORD` | PFX 密码 |

See [SECURITY.md](SECURITY.md) and [docs/packaging.md](packaging.md) §5 before first release. Run `.\scripts\release\scan-secrets.ps1` and configure GitHub Secrets via `.\scripts\release\set-github-secrets.ps1`.
