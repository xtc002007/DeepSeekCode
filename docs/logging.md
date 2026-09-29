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
    app.log       # full application log (JSON lines) + app.log.1 when rotated
    session.log   # SESSION + TOOL + LLM targets (one file per turn) + .1
    audit.log     # permission / policy audit events + .1
    panic.log     # panic hook (plain text)
    meta.json     # version, OS, component (no secrets)
```

Each file is size-capped by `logging.max_file_mb` (default 50) with true
per-write rotation: when the next write would exceed the cap, the current
file is closed (Windows-safe close-before-rename) and rolls shift
(`.3`→`.4`, …, `name`→`.1`, dropping old `.4`), keeping at most
`name` + `name.1` … `name.4` (5× cap per log type, e.g. 250MB at default).
Diagnostic bundles include the rolled files and redact them like the primaries.

Old runs are pruned when `logging.max_run_dirs` is exceeded (default 10).

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
    "max_run_dirs": 10,
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
| `deepseekcode::llm` | session.log + app.log | Provider calls (duplicated for turn reconstruction) |
| `deepseekcode::tool` | session.log + app.log | Tool execution (duplicated for turn reconstruction) |
| `deepseekcode::permission` | audit.log | Permission prompts |
| `deepseekcode::audit` | audit.log | Cross-cutting audit (policy permit/block/prompt) |
| `deepseekcode::server` | app.log | HTTP / PTY (with `request_id` + `run_id`) |
| `deepseekcode::desktop` | app.log | Desktop UI (with `run_id`) |
| `deepseekcode::storage` | app.log | Database / events |
| `deepseekcode::cli` | app.log | CLI commands |

`session.log` intentionally carries SESSION+TOOL+LLM so a full turn can be
reconstructed from one file; `app.log` remains the catch-all.

## Structured fields

Every event carries:

- `run_id` — process start (ULID, also the `run-{ulid}` directory name)
- `session_id` — where a session is in scope (`"-"` for non-session routes)
- `request_id` — HTTP `x-deepseekcode-request-id` (task-local, `"-"` outside
  HTTP such as CLI direct calls, SDK startup tasks, and background session
  drains; those correlate via `run_id` + `session_id` instead)
- `provider`, `model`, `tool`, `tool_call_id`, `duration_ms`, `status`, `code`, `error` where relevant

One request → one failure log: handlers stash `{code, error, session_id}` on
the response extension (`ApiErrorDetail`) and never log themselves; the
`request_id` middleware emits the single canonical record with
`request_id + run_id + method + path + status + duration_ms + session_id +
code + error`. Successes log once at the call site / middleware completion.

Initialization is retryable: a failed first `init` (e.g. unwritable log dir)
logs to stderr and retries on the next call instead of sticking at
`run_id="unknown"`. The default filter covers all `deepseekcode::` subsystem
targets plus `deepseekcode_core/_llm/_storage/_embed/_sdk/_plugin/...` crate
prefixes so untargeted module-path events are never silently dropped.

## Redaction

Secrets are redacted before write:

- API keys, Bearer tokens, `Authorization` headers
- `DEEPSEEKCODE_SERVER_PASSWORD`
- User prompts (hash + length + 80-char prefix by default)

Enable full prompt logging only with `logging.audit.include_prompts=true` (support scenarios).
Tool output previews (redacted first 500 chars) require
`logging.audit.include_tool_output=true` (default false: only
`output_bytes`+`title`+`duration_ms` are logged); the flag hot-reloads on
`reload_catalog`/`reload_materialization` without restart.

## Export diagnostics (support bundle)

### Desktop

Settings → **Export Diagnostics** saves a zip and logs the `run_id`.

### CLI

```bash
deepseekcode diagnostics export --output ./support-bundle.zip
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
