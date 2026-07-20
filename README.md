# DeepSeekCode

Rust + GPUI AI programming environment. Core SDK with embedded server; desktop, CLI, and future web clients share the same runtime.

## Architecture

```
deepseekcode-desktop (GPUI) ──┐
deepseekcode-cli              ├── deepseekcode-sdk ── deepseekcode-server ── deepseekcode-core
future web client         ──┘                              └── deepseekcode-llm
```

## Quick Start

```bash
cargo run -p deepseekcode-desktop    # GPUI desktop app
cargo run -p deepseekcode-cli -- serve --directory . --port 4096
cargo test -p deepseekcode-core
```

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `deepseekcode-types` | Shared IDs and errors |
| `deepseekcode-schema` | API and domain schemas |
| `deepseekcode-config` | DeepSeekCode config loading (`deepseekcode.jsonc`) |
| `deepseekcode-storage` | SQLite + event store |
| `deepseekcode-llm` | LLM protocol layer |
| `deepseekcode-core` | Session engine, tools, system context |
| `deepseekcode-server` | Axum HTTP + SSE |
| `deepseekcode-sdk` | Rust client + EmbeddedEngine |
| `deepseekcode-desktop` | GPUI desktop application |
| `deepseekcode-cli` | Command-line interface |
| `deepseekcode-plugin` | Rust + WASM plugin host |

## Configuration

Architecture overview: [docs/architecture.md](docs/architecture.md).

**Do not commit real API keys.** Prefer environment variables:

```bash
export DEEPSEEK_API_KEY=...
export OPENAI_API_KEY=...
export DEEPSEEKCODE_SERVER_PASSWORD=...   # when exposing serve remotely
```

## Security

See **[SECURITY.md](SECURITY.md)** for the full policy. Summary:

| Secret | Where it belongs |
|--------|------------------|
| Provider API keys | Environment variables or OS keyring (via UI) — not in git |
| Server password | `DEEPSEEKCODE_SERVER_PASSWORD` / desktop keyring |
| Release signing keys | `signing/*.key` (gitignored) or GitHub Actions secrets |
| Local reference clones | `third-party-code/` (gitignored, never publish) |

Before release, run:

```powershell
.\scripts\release\scan-secrets.ps1
```

Session and diagnostic exports **redact secrets by default** (`sanitize=true`, diagnostic log scrubbing). Use `--no-sanitize` on CLI export only when you need full content locally.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Third-party upstream reference notes: [docs/third-party-reference.md](docs/third-party-reference.md).

## License

MIT — see [LICENSE](LICENSE). Third-party components: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Release

Windows desktop installer, signing keys, and CI release workflow: [docs/packaging.md](docs/packaging.md).
