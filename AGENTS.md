# DeepSeekCode

Rust-native AI programming environment. Core SDK with embedded Axum server; GPUI desktop and CLI clients.

## Architecture

- `deepseekcode-core` — Session engine, system context, tools, policy engine
- `deepseekcode-server` — HTTP API + SSE (`/api/v1/*`)
- `deepseekcode-sdk` — `EmbeddedEngine` + `DeepSeekCodeClient`
- `deepseekcode-desktop` — GPUI UI (standalone crate, depends on git `gpui`)
- `deepseekcode-cli` — `run`, `serve`, `session-list`
- `deepseekcode-i18n` — locales, catalogs, locale-aware formatting

## Internationalization

Ten UI locales (`en`, `zh-Hans`, `zh-Hant`, `ja`, `ko`, `es`, `fr`, `de`,
`pt-BR`, `ru`). See [docs/i18n.md](docs/i18n.md). The short version: every
user-visible string lives in all ten catalogs (English defines the key set),
values render through `deepseekcode_i18n` helpers (never `format!`), system
prompts and logs stay English, and `cargo test -p deepseekcode-i18n` enforces
key/placeholder parity. `ui.language` (`"auto"` or a BCP-47 tag) pins the
language in `deepseekcode.jsonc`.

## Session Runtime

Follow `specs/session-runtime.md`. Key semantics:
- Prompt admission inbox → promotion at safe provider-turn boundary
- Context epochs with AGENTS.md as context source
- Location-scoped services per project directory

## Development

```bash
cargo test -p deepseekcode-core
cargo run -p deepseekcode-cli -- serve --directory . --port 4096
cargo run -p deepseekcode-desktop   # requires GPUI/Zed deps
```

## Config

DeepSeekCode reads `deepseekcode.jsonc` or `.deepseekcode/deepseekcode.jsonc`.

### Policy

Tool permissions use the `policy` ruleset (`permit` / `prompt` / `block`). See `specs/policy-system.md`. Legacy `permissions.allow/deny` is auto-converted. User **Always (project)** grants persist in SQLite per project directory.

### Open Source / Security

Licensed under **Apache-2.0** (`LICENSE`, `NOTICE`). Git secret scan is automatic after `cargo build`/`cargo test` (sets `core.hooksPath`). Before publishing or cutting a release, read [SECURITY.md](SECURITY.md) and run `scripts/release/scan-secrets.ps1`. Never commit `.env`, signing private keys, `third-party-code/`, or API keys in config files.
