
## Goals

DeepSeekCode is a Rust-native AI programming environment:

- **Session-driven agent** with tools, policy, file/terminal/diff review, multi-provider LLM
- **Embeddable SDK** (`deepseekcode-sdk`) shared by desktop, CLI, and future clients
- **GPUI desktop shell** with in-process Axum server + SSE (low latency vs sidecar)

## Layered architecture

```mermaid
flowchart TB
  subgraph clients [Clients]
    Desktop[deepseekcode-desktop]
    CLI[deepseekcode-cli]
  end

  subgraph sdk [SDK]
    Sdk[deepseekcode-sdk]
  end

  subgraph server [Server]
    Axum[deepseekcode-server]
  end

  subgraph core [Core]
    Session[Session engine]
    Tools[Tools + policy]
    Ctx[System context]
  end

  subgraph infra [Infrastructure]
    LLM[deepseekcode-llm]
    DB[deepseekcode-storage]
    Plugin[deepseekcode-plugin]
  end

  Desktop --> Sdk
  CLI --> Sdk
  Sdk --> Axum
  Axum --> core
  core --> infra
```

| Layer | Crate | Role |
|-------|-------|------|
| Types / schema | `deepseekcode-types`, `deepseekcode-schema` | IDs, errors, API shapes |
| Config | `deepseekcode-config` | `deepseekcode.jsonc`,  |
| Core | `deepseekcode-core` | Session runner, tools, policy, MCP |
| LLM | `deepseekcode-llm` | OpenAI / Anthropic / OpenRouter / DeepSeek |
| Storage | `deepseekcode-storage` | SQLite, events, encrypted credentials |
| Server | `deepseekcode-server` | HTTP `/api/v1/*`, SSE, OpenAPI |
| SDK | `deepseekcode-sdk` | `EmbeddedEngine`, HTTP client |
| Desktop | `deepseekcode-desktop` | GPUI UI |
| CLI | `deepseekcode-cli` | `serve`, `run`, session commands |

## Session runtime (summary)

Authoritative detail: [`specs/session-runtime.md`](../specs/session-runtime.md).

- Prompt admission inbox → promotion at safe provider-turn boundary
- Context epochs with `AGENTS.md` as context source
- Location-scoped services per project directory
- Policy engine: `permit` / `prompt` / `block` ([`specs/policy-system.md`](../specs/policy-system.md))

## Embedded vs standalone server

- **Desktop / SDK default**: bind `127.0.0.1`, engine in-process
- **CLI `serve`**: standalone HTTP server; set `DEEPSEEKCODE_SERVER_PASSWORD` when exposed beyond localhost ([`SECURITY.md`](../SECURITY.md))

## Performance smoke targets (local dev)

Benchmarks under `benches/` track rough targets:

| Benchmark | Target (indicative) |
|-----------|---------------------|
| `startup` | Embedded engine cold start < 2s |
| `sse_latency` | First SSE event < 50ms locally |
| `file_tree` | 10k-node listing < 200ms |
| `memory` | RSS after engine start (regression guard) |

Run: `cargo bench -p deepseekcode-sdk --bench startup`

## Related docs

- [README.md](../README.md) — quick start
- [packaging.md](packaging.md) — release channels & signing
- [logging.md](logging.md) — observability
- [CONTRIBUTING.md](../CONTRIBUTING.md) — dev workflow
