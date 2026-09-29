# Third-Party Notices

DeepSeekCode is licensed under the [Apache License, Version 2.0](LICENSE). See also [NOTICE](NOTICE).

This file lists major third-party components and their licenses. For a full dependency tree, run:

```bash
cargo install cargo-license --locked
cargo license --json
```

## UI Framework

| Component | Source | License |
|-----------|--------|---------|
| GPUI, gpui_platform | [zed-industries/zed](https://github.com/zed-industries/zed) (pinned git rev in `deepseekcode-desktop/Cargo.toml`) | Apache-2.0 (Zed project) |


## Major Rust Dependencies

| Crate | Typical use | License |
|-------|-------------|---------|
| tokio | Async runtime | MIT |
| axum, tower, tower-http | HTTP server | MIT |
| sqlx | SQLite | MIT / Apache-2.0 |
| reqwest | HTTP client | MIT / Apache-2.0 |
| serde, serde_json | Serialization | MIT / Apache-2.0 |
| tracing, tracing-subscriber | Logging | MIT |
| clap | CLI parsing | MIT / Apache-2.0 |
| utoipa, schemars | OpenAPI / JSON Schema | MIT / Apache-2.0 |
| keyring | OS credential store | MIT / Apache-2.0 |
| aes-gcm, sha2, base64 | Credential encryption | MIT / Apache-2.0 OR BSD-3-Clause |
| wasmtime | WASM plugin host | Apache-2.0 |
| portable-pty, alacritty_terminal | Terminal | MIT |
| pulldown-cmark | Markdown | MIT |
| ulid, uuid, chrono | IDs and time | MIT / Apache-2.0 |
| zip | Diagnostic bundles | MIT |
| similar | Diff rendering | MIT |
| tree-sitter (+ grammars) | Syntax highlighting | MIT |
| rfd | Native file dialogs | MIT |
| refinery | SQL migrations | MIT |
| wiremock | Tests | MIT |
| tempfile | Tests | MIT / Apache-2.0 |

## Release Tooling (not shipped in application binaries)

| Tool | Purpose | License |
|------|---------|---------|
| minisign | Release artifact signing | ISC |
| NSIS | Windows installer | zlib/libpng |
| cargo-dist | CLI distribution (optional channel) | MIT / Apache-2.0 |
| release-plz | crates.io publishing (optional channel) | MIT / Apache-2.0 |

## Local Reference Checkouts (not shipped)

| Directory | Content | License holder |
|-----------|---------|----------------|
| `third-party-code/` (gitignored, dev-only) | Upstream reference checkout(s), currently `DeepSeek-Reasonix` — design reference only, never built, packaged, or published | Respective upstream owners |

See `third-party-code/README.md`. If any snippet from these trees is copied
into this repository, add a row above naming the file, upstream commit, and
license, per the upstream terms.

## Trademarks

DeepSeek, OpenAI, Anthropic, and other names are trademarks of their respective owners. This project is not affiliated with or endorsed by those organizations unless explicitly stated.
