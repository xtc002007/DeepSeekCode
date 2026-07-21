# DeepSeekCode Open Core 草案

> 状态：**规划草案**（Community 栈已采用 **Apache-2.0**；项目尚未公开）  
> 原则：**现有代码 = Community 全量开源；收费能力 = 未来新增，不从开源仓库撤回功能。**

---

## 1. 许可与仓库策略

| 层级 | 协议 | 仓库 |
|------|------|------|
| **Community**（当前 monorepo 全部 crate） | Apache-2.0 | `deepseekcode`（公开） |
| **Pro / Enterprise**（未来） | 专有 / 商业许可 | `deepseekcode-pro`（私有，或独立 org 下 private repo） |
| **Cloud 控制面**（未来） | 专有 | `deepseekcode-cloud`（私有） |
| **商标** | 独立声明 | 名称 `DeepSeekCode`、`DeepSeekCode Pro` 仅官方构建可用 |

**不做的事**

- 不在 v1 开源后再从 public repo **删除**已发布功能改为收费  
- 不把「会话运行时 / OpenAPI 契约 / specs」闭源（这是生态与信任根基）

**可选后手（仅当 SaaS 克隆成问题时）**

- `deepseekcode-server` 单独 **AGPL-3.0 + 商业例外**；Core/SDK 仍 Apache-2.0  
- 当前阶段 **不建议** 全仓库 AGPL

---

## 2. 当前 Crate → 是否开源

| Crate | 开源？ | Community 定位 | 备注 |
|-------|--------|----------------|------|
| `deepseekcode-types` | ✅ | 共享 ID、错误 | crates.io 候选 |
| `deepseekcode-schema` | ✅ | API / 领域 schema | OpenAPI 真源 |
| `deepseekcode-config` | ✅ | `deepseekcode.jsonc` 加载 | |
| `deepseekcode-telemetry` | ✅ | 日志、诊断导出 | 无账号遥测 |
| `deepseekcode-storage` | ✅ | SQLite、凭据加密、事件 | 本地单用户 |
| `deepseekcode-llm` | ✅ | 多 Provider 协议层 | BYOK（用户自带 Key） |
| `deepseekcode-core` | ✅ | Session、Policy、Tools、MCP、Compaction | **开放核心心脏** |
| `deepseekcode-plugin` | ✅ | WASM / 原生插件宿主 | 运行时开源；商店可 Pro |
| `deepseekcode-server` | ✅ | `/api/v1/*`、SSE、PTY | 见 §4 路由分层 |
| `deepseekcode-sdk` | ✅ | `EmbeddedEngine`、HTTP 客户端 | 嵌入场景主入口 |
| `deepseekcode-cli` | ✅ | `serve`、`run`、session 命令 | Community CLI |
| `deepseekcode-desktop` | ✅ | GPUI Community 桌面 | 见 §3 功能上限 |
| `examples/plugins/hello-hook` | ✅ | 插件示例 | |
| `benches/` | ✅ | 性能冒烟 | |
| `specs/` | ✅ | 行为契约 | 对外承诺的语义 |
| `migrations/` | ✅ | SQL 迁移 | |
| `packaging/`、`scripts/release/` | ✅ | 构建与发布脚本 | 不含私钥 |
| `docs/` | ✅ | 公开文档 | 不含内部规划 |

**未来新增（闭源，不在当前 monorepo）**

| Crate / 服务 | 协议 | 职责 |
|--------------|------|------|
| `deepseekcode-license` | 专有 | 订阅 / 离线 license JWT 校验 |
| `deepseekcode-enterprise` | 专有 | SSO、组织 Policy、审计汇聚、License Server 客户端 |
| `deepseekcode-desktop-pro` | 专有 | 桌面 Pro UI 模块（可选依赖） |
| `deepseekcode-cloud`（服务） | 专有 | 账号、团队、同步、托管 Server、计费 |

---

## 3. Community vs Pro 功能对照（产品）

### 3.1 运行时与 API（`deepseekcode-core` + `server`）

| 能力 | Community | Pro / Enterprise |
|------|-----------|------------------|
| 单项目本地 Session | ✅ | ✅ |
| Prompt inbox / steer / queue | ✅ | ✅ |
| Policy permit/prompt/block | ✅ | ✅ |
| 内置工具 + MCP + Skills + 插件宿主 | ✅ | ✅ |
| Compaction / revert / export(sanitize) | ✅ | ✅ |
| 嵌入式 `127.0.0.1` Server | ✅ | ✅ |
| 远程 `serve` + Basic Auth | ✅ | ✅ + SSO/OIDC |
| 多租户 / 组织 / RBAC | ❌ | ✅ |
| 组织级 Policy 模板下发 | ❌ | ✅ |
| 集中审计日志 / SIEM 导出 | ❌ | ✅ |
| 官方托管 Server（SaaS） | ❌ | ✅ |
| 会话跨设备云同步 | ❌ | ✅ |
| 官方 LLM 网关（代付/token） | ❌ | ✅ 可选 |

### 3.2 桌面（`deepseekcode-desktop`）

| 能力 | Community | Pro |
|------|-----------|-----|
| 单工作区、本地项目 | ✅ | ✅ |
| 时间线、文件树、终端、Provider 连接 | ✅ | ✅ |
| 主题、设置、诊断导出 | ✅ | ✅ |
| 多窗口 / 多工作区数量 | 限制（如 ≤2） | 无限制 |
| 官方自动更新优先通道 | 可选 / 延迟 | ✅ 优先 |
| 高级 Plan/Diff 审查 UI | 基础 | ✅ 增强 |
| 官方签名插件市场 | ❌ | ✅ |
| 崩溃报告一键上传（opt-in） | ❌ | ✅ |

### 3.3 CLI / SDK

| 能力 | Community | Pro |
|------|-----------|-----|
| `serve` / `run` / session API | ✅ | ✅ |
| 嵌入第三方产品（Apache 许可下） | ✅ 允许 | — |
| 企业离线 License Server 对接 | ❌ | ✅ |
| 批量 / CI Agent 编排 | 基础 | ✅ 增强 |

---

## 4. API 路由分层（建议）

**Community（已实现，保持 `/api/v1/*`）**

```
health, openapi, location, projects, worktree
session/*, provider/*, permission/*, policy/*
mcp/*, skills, plugins
diagnostics/export, event (SSE)
fs/*, pty/*
```

**Pro / Enterprise（未来，建议前缀 `/api/v1/org/` 或独立 `deepseekcode-cloud`）**

```
POST   /api/v1/org                          # 创建组织
GET    /api/v1/org/{id}/members
POST   /api/v1/org/{id}/policy/template     # 下发策略模板
GET    /api/v1/org/{id}/audit               # 审计查询
POST   /api/v1/org/{id}/sessions/sync       # 云同步
GET    /api/v1/license/status               # 订阅状态（桌面/CLI 轮询）
POST   /api/v1/auth/oidc/callback           # 企业 SSO
```

无有效 license 时：HTTP **402 Payment Required** 或 **403** + 明确 error code（如 `pro_required`）。

---

## 5. 扩展点（第一批应在 Core 预留）

以下 trait / 结构 **留在开源 crate**，实现分 `community`（noop）与 `pro`（闭源 crate）。

### 5.1 认证与租户 — `deepseekcode-server`

**现状**：`AppState { password: Option<String> }` + `auth.rs` Basic Auth  

**建议扩展**：

```rust
// deepseekcode-server/src/auth.rs（开源）
pub trait RequestAuthenticator: Send + Sync {
    fn authenticate(&self, request: &Request) -> Result<AuthContext, AuthError>;
}

pub struct AuthContext {
    pub user_id: Option<String>,
    pub org_id: Option<String>,
    pub plan: PlanTier,  // Community | Pro | Enterprise
}

// AppState 增加
pub authenticator: Arc<dyn RequestAuthenticator>,
```

- Community：`NoopAuthenticator`（仅保留现有 Basic Auth 可选）  
- Pro：`OidcAuthenticator` + license 校验（闭源 crate 提供）

### 5.2 许可 / 功能门控 — 新 trait（可放 `deepseekcode-types` 或 `deepseekcode-core`）

```rust
pub enum Feature {
    MultiWindow,
    CloudSync,
    OrgPolicyTemplate,
    AuditExport,
    // ...
}

pub trait FeatureGate: Send + Sync {
    fn enabled(&self, feature: Feature, ctx: &AuthContext) -> bool;
}
```

- Community：`CommunityFeatureGate` → 仅 Community 功能为 true  
- Desktop/Server 启动时注入；Pro 构建注入 `LicensedFeatureGate`

### 5.3 Policy 组织层 — `deepseekcode-core/src/policy/`

**现状**：`PolicyService` 按 project + session 规则  

**建议扩展**：

```rust
pub trait OrgPolicyProvider: Send + Sync {
    async fn org_ruleset(&self, org_id: &str) -> Option<PolicyRuleset>;
}
```

- Community：返回 `None`  
- Enterprise：从 Cloud 或本地 License Server 拉取组织规则，合并进 `PolicyService`（文档化优先级：org > project > agent）

### 5.4 审计汇聚 — `deepseekcode-telemetry`

**现状**：本地 `audit.log`、diagnostics zip  

**建议扩展**：

```rust
pub trait AuditSink: Send + Sync {
    fn emit(&self, event: AuditEvent);
}
```

- Community：`FileAuditSink`（现有行为）  
- Enterprise：`RemoteAuditSink`（HTTPS 批量上报，闭源实现）

### 5.5 会话存储同步 — `deepseekcode-storage`

**现状**：每 location  SQLite  

**建议扩展**（Phase 2+）：

```rust
pub trait SessionSyncAdapter: Send + Sync {
    async fn push_snapshot(&self, session_id: SessionId, payload: &[u8]) -> Result<()>;
    async fn pull_updates(&self, since: EventSeq) -> Result<Vec<SyncChunk>>;
}
```

- Community：不编译 / noop  
- Pro Cloud：闭源 adapter

### 5.6 桌面 Pro UI — `deepseekcode-desktop`

**建议结构**（不破坏现有 `WorkspaceView`）：

```
deepseekcode-desktop/          # 开源 Community
  src/pro/                     # 可选模块，Community 构建为空 stub
    mod.rs                     # pub fn register_pro_panels(...) 空实现

deepseekcode-desktop-pro/      # 闭源（未来 repo）
  依赖 deepseekcode-desktop
  实现 register_pro_panels、多窗口策略、Pro 设置页
```

Community `Cargo.toml` 不依赖 pro crate；Pro 安装包链接 `deepseekcode-desktop-pro` 静态注册。

### 5.7 插件目录 — `deepseekcode-plugin` + server `/api/v1/plugins`

- Community：本地路径加载 + 示例 `hello-hook`  
- Pro：**官方 Catalog API**（签名校验、版本兼容）→ 闭源 catalog 服务 + server 可选路由 `/api/v1/plugins/catalog`

---

## 6. 实施阶段（建议顺序）

| 阶段 | 目标 | 开源仓库工作 | 闭源仓库工作 |
|------|------|--------------|--------------|
| **P0 公开** | Community 首次发布 | Apache-2.0、README Community/Pro 说明、本草案 | 无 |
| **P1 门控骨架** | 可插 Pro 但不发 Pro | `FeatureGate`、`RequestAuthenticator` trait + noop 实现 | — |
| **P2 桌面 Pro v1** | 多窗口 + 官方更新通道 | desktop stub `pro/` 模块 | `desktop-pro`、license 校验 |
| **P3 Enterprise v1** | SSO + 组织 Policy | `OrgPolicyProvider`、audit sink trait | `enterprise` crate、License Server |
| **P4 Cloud v1** | 账号 + 会话同步 | Session sync trait、OpenAPI 扩展草案 | `deepseekcode-cloud` |
| **P5 商业化** | 计费、插件市场 | 文档 + 402 错误码规范 | 支付、Catalog 服务 |

---

## 7. README 片段（可直接粘贴）

### Community Edition

DeepSeekCode Community is **Apache-2.0** software: full session runtime, CLI, SDK, and desktop for **local, single-user** AI-assisted development. You may use, modify, and distribute it under the Apache-2.0 license.

### Pro & Enterprise

**DeepSeekCode Pro** and **Enterprise** add team collaboration, cloud sync, advanced desktop UX, SSO, centralized policy, and official hosted services. These components are **separate proprietary offerings** and are not included in this repository.

### Trademark

This license does not grant permission to use the DeepSeekCode name or logos for derivative products. See [TRADEMARK.md](TRADEMARK.md) (TODO).

---

## 8. 贡献与 CLA（Pro 上线前必须）

公开 Pro 前引入 **Contributor License Agreement**：

- 贡献者保留版权，但授予维护者 **再许可** 为 Apache-2.0（Community）及 **专有**（Pro）的双重权利  
- 否则第三方贡献的代码无法合法放入闭源 Pro crate

 interim：可先用 **DCO**（Developer Certificate of Origin）+ 贡献指南声明；Pro 营收前完成 CLA。

---

## 9. 决策检查清单（公开前）

- [x] `LICENSE` 改为 Apache-2.0（含 `NOTICE`、`THIRD_PARTY_NOTICES.md` 首行）  
- [x] README 增加 §Community vs Pro  
- [x] 本文件提交至 `docs/open-core-plan.md`  
- [x] 不在 public repo 出现 Pro 闭源代码  
- [x] `Cargo.toml` `repository` 占位 URL（公开前替换为真实 GitHub 地址）  
- [ ] 商标与域名策略（可选 `TRADEMARK.md`）  
- [ ] P1 trait 骨架排进 roadmap issue（不阻塞首次 Community 发布）

---

## 10. 相关文档

- [architecture.md](architecture.md) — 技术分层  
- [packaging.md](packaging.md) — Community 安装包与签名  
- [SECURITY.md](../SECURITY.md) — 密钥与暴露面  
- [CONTRIBUTING.md](../CONTRIBUTING.md) — 贡献流程（待补 CLA）
