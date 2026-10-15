# Contributing to Code++

Thank you for your interest in contributing. This project is a Rust workspace with a GPUI desktop client, CLI, and embeddable SDK.

## Development Setup

Secret scanning is **automatic** — no manual hook install required for normal development:

1. Run any build/test once (`cargo build`, `cargo test`, …) — this configures `git config core.hooksPath scripts/git-hooks` for this clone.
2. Use the usual git flow: `git add` → `git commit` → `git push`.
3. On the first commit, gitleaks is auto-downloaded to `.tools/` if missing (needs network once).

Optional manual setup (same result): `.\scripts\install-git-hooks.ps1`

Emergency bypass only: `CODE_PLUS_PLUS_SKIP_SECRET_SCAN=1 git commit ...`

```bash
cargo test -p code-plus-plus-core
cargo run -p code-plus-plus-cli -- serve --directory . --port 4096
cargo run -p code-plus-plus-desktop   # requires GPUI / platform deps
```

See [AGENTS.md](AGENTS.md) for architecture and session-runtime conventions. Public architecture: [docs/architecture.md](docs/architecture.md).

## Before You Open a PR

1. **Format and lint**

   ```bash
   cargo fmt --all
   cargo clippy --all-targets -- -D warnings
   ```

2. **Run tests** (keyring disabled in CI)

   ```bash
   $env:CODE_PLUS_PLUS_DISABLE_KEYRING = "1"   # PowerShell
   cargo test --workspace
   ```

3. **Secret scan** (required for release-related changes)

   ```powershell
   .\scripts\release\scan-secrets.ps1
   ```

4. **Do not commit**
   - `.env`, `*.db`, `signing/*.key`, `signing/*.pfx`
   - `third-party-code/` (local upstream reference only — see [SECURITY.md](SECURITY.md))
   - Real API keys in `code-plus-plus.jsonc`

5. **Before first public release**, set `repository` in root `Cargo.toml` to your GitHub URL.

## Commit Messages

Use clear, imperative subjects focused on **why**:

- `fix: preserve inbox on interrupt per session-runtime spec`
- `feat: default session export sanitization to true`

## License

By contributing, you agree that your contributions are licensed under the [Apache License, Version 2.0](LICENSE).
