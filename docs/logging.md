# DeepSeekCode Logging

DeepSeekCode uses the Rust [`tracing`](https://docs.rs/tracing) ecosystem for structured, product-grade observability. All binaries initialize logging through `deepseekcode_telemetry::init` — do not call `tracing_subscriber::fmt().init()` directly.

## Quick start (development)

```powershell
# Standard tracing filter (highest priority)
$env:RUST_LOG = "deepseekcode_llm=debug,deepseekcode_core::session=debug"

# Shortcut overrides
$env:DEEPSEEKCODE_LOG_LEVEL = "debug"
$env:DEEPSEEKCODE_LOG_CONSOLE = "1"

cargo run -p deepseekcode-desktop
```

Send a prompt and inspect the session trace:

```
%LOCALAPPDATA%\deepseekcode\logs\run-<ulid>\session.log
```

## Log layout

Each process start creates a new run directory:

```
%LOCALAPPDATA%/deepseekcode/logs/
  run-{ulid}/
    app.log       # full application log (JSON lines)
    session.log   # target=deepseekcode::session
    audit.log     # permission / tool / write audit events
    meta.json     # version, OS, component (no secrets)
```

Old runs are pruned when `logging.max_run_dirs` is exceeded (default 30).

## Configuration (`deepseekcode.jsonc`)

In the DeepSeekCode project config file:

```jsonc
{
  "logging": {
    "level": "info",
    "console": true,
    "file": true,
    "format": "json",
    "dir": null,
    "max_file_mb": 50,
    "max_run_dirs": 30,
    "modules": {
      "deepseekcode_core::session": "debug",
      "deepseekcode_llm": "debug"
    },
    "audit": {
      "enabled": true,
      "include_tool_output": false,
      "include_prompts": false
    }
  }
}
```

### Environment overrides

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | Standard `tracing` env filter (wins over config) |
| `DEEPSEEKCODE_LOG_LEVEL` | Global default level |
| `DEEPSEEKCODE_LOG_DIR` | Log root directory |
| `DEEPSEEKCODE_LOG_CONSOLE` | `0` or `1` force console sink |

Release builds default to `file=true`, `console=false`, `level=info`.

## Target naming

| Target | File | Purpose |
|--------|------|---------|
| `deepseekcode::app` | app.log | Process lifecycle |
| `deepseekcode::session` | session.log | Session / runner |
| `deepseekcode::llm` | app.log | Provider calls |
| `deepseekcode::tool` | app.log | Tool execution |
| `deepseekcode::permission` | audit.log | Permission prompts |
| `deepseekcode::audit` | audit.log | Cross-cutting audit |
| `deepseekcode::server` | app.log | HTTP / PTY |
| `deepseekcode::desktop` | app.log | Desktop UI |
| `deepseekcode::storage` | app.log | Database / events |

## Structured fields

Key spans and events should include:

- `run_id` — process start (ULID)
- `session_id` — active session
- `request_id` — HTTP header `x-deepseekcode-request-id`
- `provider`, `model`, `tool`, `duration_ms` where relevant

## Redaction

Secrets are redacted before write:

- API keys, Bearer tokens, `Authorization` headers
- `DEEPSEEKCODE_SERVER_PASSWORD`
- User prompts (hash + length + 80-char prefix by default)

Enable full prompt logging only with `logging.audit.include_prompts=true` (support scenarios).

## Export diagnostics (support bundle)

### Desktop

Settings → **Export Diagnostics** saves a zip and logs the `run_id`.

### CLI

```bash
deepseekcode-cli diagnostics export --output ./support-bundle.zip
```

### HTTP API

```
GET /api/v1/diagnostics/export?run_id=<optional>
```

Returns `application/zip` with `app.log`, `session.log`, `audit.log`, and `meta.json`.

### Support ticket template

```
Run ID: <paste run_id from meta.json or Settings>
Component: desktop | cli
OS: Windows 11
Steps: ...
Expected: ...
Actual: ...
Attachment: support-bundle.zip
```

## CI / test failures

When integration tests fail, dump recent log lines:

```rust
if let Some(ctx) = deepseekcode_telemetry::run_context() {
    eprintln!("{}", deepseekcode_telemetry::tail_app_log(&ctx.run_dir, 200));
}
```

The CI workflow runs tests with `RUST_BACKTRACE=1` and prints the tail of the current run log on failure.

## Adding instrumentation

```rust
tracing::info!(
    target: deepseekcode_telemetry::SESSION,
    session_id = %session_id,
    duration_ms = elapsed.as_millis(),
    "tool completed"
);

// Audit events
tracing::info!(
    target: deepseekcode_telemetry::AUDIT,
    permission_id = %id,
    decision = "denied",
    "permission resolved"
);
```

Use `#[tracing::instrument(skip(self), fields(session_id = %self.session_id))]` on async hot paths.
