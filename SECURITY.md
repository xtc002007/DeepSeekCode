# Security Policy

## Reporting a Vulnerability

If you discover a security issue, **do not** open a public GitHub issue with exploit details.

1. Email the maintainers (see repository contact) with a description, reproduction steps, and impact.
2. Allow reasonable time for a fix before public disclosure.
3. We will acknowledge receipt and coordinate a release if needed.

## Secrets and Credentials

### Never commit

| Item | Location |
|------|----------|
| Environment files | `.env` (use `.env.example` as template) |
| Release signing private keys | `signing/*.key`, `signing/*.pfx` |
| SQLite databases | `**/*.db` (sessions, encrypted provider keys) |
| Local reference trees | `third-party-code/` (dev-only upstream checkout) |
| Runtime logs / diagnostics | `%LOCALAPPDATA%/code-plus-plus/logs/` or `~/.local/share/code-plus-plus/logs/` |

**Automatic (local):** run `cargo build` or `cargo test` once after clone — commits then scan staged files via `gitleaks protect`. First commit may download gitleaks to `.tools/` (one-time, needs network).

Run a full-repository scan before every release:

```powershell
.\scripts\release\scan-secrets.ps1
```

Emergency bypass only: `CODE_PLUS_PLUS_SKIP_SECRET_SCAN=1 git commit ...`.

### Provider API keys

Prefer **environment variables** over inline config:

- `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `DEEPSEEK_API_KEY`, `OPENROUTER_API_KEY`
- `BRAVE_SEARCH_API_KEY` (when using Brave web search)

If you must use `code-plus-plus.jsonc`, **do not commit** `providers.*.api_key`. Use `code-plus-plus.jsonc.example` as a template.

Provider keys entered in the desktop UI are stored in the OS credential store (with an encrypted SQLite fallback). See `crates/code-plus-plus-storage/src/credentials_store.rs`.

### Remote server password

When connecting the desktop client to a remote Code++ server:

- The password is stored in the OS credential store (`code-plus-plus` / `server_password`), not in `preferences.json`.
- Set the server-side password via `CODE_PLUS_PLUS_SERVER_PASSWORD` when running `code-plus-plus serve`.

### Credential encryption (SQLite fallback)

Encrypted provider keys in SQLite use a key derived from a fixed salt plus the local username/hostname. This protects against casual file copy but is **not** strong isolation against a local attacker. Prefer the OS keyring when available.

## Network Exposure

The embedded engine binds to `127.0.0.1` by default. If you expose the HTTP API on a network interface:

- **Always** set `CODE_PLUS_PLUS_SERVER_PASSWORD` (Basic Auth user: `code-plus-plus`).
- Treat the server as full agent access to the project directory (file write, shell, MCP tools per policy).

## Exports and Diagnostics

| Endpoint / feature | Default | Notes |
|--------------------|---------|-------|
| `GET /api/v1/session/{id}/export` | `sanitize=true` | Redacts long text and tool I/O |
| `GET /api/v1/diagnostics/export` | log redaction enabled | Applies pattern redaction to log files in the bundle |

Diagnostic bundles may still contain file paths, session metadata, and partial prompts. Share exports only with trusted parties.

## Release Signing

- **Private** minisign / Authenticode keys live in GitHub Actions secrets or local `signing/` (gitignored).
- Only the **public** minisign key (`signing/code-plus-plus.key.pub`) belongs in the repository.
- If a private key was ever committed, **rotate it immediately** and re-sign all release artifacts.


## Supported Versions

Security fixes are applied to the latest release on the default branch. Older tags may not receive backports unless noted in release notes.
