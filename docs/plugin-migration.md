# Plugin & MCP Migration Guide

This document describes how Code++ integrates MCP servers, skills, and the plugin hook surface.

## MCP Servers

### Configuration

MCP servers are declared in `code-plus-plus.jsonc` under `mcp.servers`:

```jsonc
{
  "mcp": {
    "servers": {
      "filesystem": {
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"],
        "enabled": true
      },
      "remote": {
        "url": "https://example.com/mcp",
        "enabled": true
      }
    }
  }
}
```

### Transports

- **Local stdio**: `command` + `args` — long-lived subprocess with JSON-RPC 2.0
- **Remote HTTP/SSE**: `url` — Streamable HTTP with SSE fallback (`mcp/remote.rs`)

Remote hardening is ported from `my-notebook` (`crates/my_notebook_ai/src/mcp.rs`):
60s read / 120s total HTTP timeouts (reqwest's default is none), `http(s)`-only
URL validation, incremental SSE reads with a 60s wall deadline (never `read to
EOF` on `text/event-stream`), 1MiB per-payload / 4MiB buffer caps, strict JSON
(non-JSON `data:` is an error, never tool output), exact numeric `id` matching,
`Mcp-Session-Id` passthrough, and a 30s budget on every local `call_tool`.

### Windows desktop automation (windows-mcp style)

Any MCP server speaking the line-protocol JSON-RPC subset (`initialize`,
`notifications/initialized`, `tools/list`, `tools/call`) works. A minimal
windows-mcp style example lives at
`crates/code-plus-plus-core/tests/support/mcp_windows_server.py` (tools:
`screenshot`, `snapshot`, `click`, `type_text`, `powershell`) with coverage in
`crates/code-plus-plus-core/tests/mcp_windows_test.rs`:

```jsonc
{ "mcp": { "servers": {
  "windows-desktop": {
    "command": "python",
    "args": ["D:/tools/windows-mcp-server.py"],
    "enabled": true
  }
} } }
```

Then `mcp → list` shows the desktop tools and `mcp → connect windows-desktop`
bridges them as `mcp_windows-desktop_screenshot` (or via the stable
`use_capability` proxy: `{action: call, server: windows-desktop, ...}`). The
test server mocks side effects (click/type/powershell only echo); point the
command at a real desktop-automation server for live control.

### Location scope

Each project directory gets its own `McpConnectionPool` when `LocationServices` is created. Servers auto-connect on location creation when `enabled` is true.

### Tool bridging

Connected MCP tools are registered in the session `ToolRegistry` as `{server}_{tool}`. For example, a `read_file` tool on server `filesystem` becomes `filesystem_read_file`.

`POST /api/v1/mcp/{name}/connect` and `disconnect` refresh bridged tools on **all cached session runners** for that location via `SessionService::refresh_mcp_tools()`.

### HTTP API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/mcp` | List configured servers and connection status |
| POST | `/api/v1/mcp/{name}/connect` | Connect a server and refresh bridged tools |
| POST | `/api/v1/mcp/{name}/disconnect` | Disconnect a server and refresh bridged tools |

Pass `x-code-plus-plus-directory` to target the project location.

## Skills

Skills are discovered from:

- Paths in config `skills`
- `.code-plus-plus/skills/*/SKILL.md`
- Optional remote index URL (`skills_index_url`)

`SkillCatalog` injects a summary into system context via `skill_guidance`. Agents load full instructions with the builtin `skill` tool:

```json
{ "name": "my-skill" }
```

`GET /api/v1/skills` is read-only and never triggers `reload_materialization()`
(it must not serialize the server per list call). Catalog changes are applied
via explicit reload paths (`engine.reload_catalog()` / runner
`reload_materialization_for_all_runners`).


| Hook | When |
|------|------|
| `tool.execute.before` | Before a tool runs; can mutate arguments |
| `tool.execute.after` | After a tool succeeds; can mutate output |
| `chat.system.transform` | Before each provider turn; can extend system prompt |
| `session.compacting` | During compaction; can inject context or replace summary prompt |

### WASM plugins

Place `.wasm` files in `.code-plus-plus/plugins/` or list paths in config `plugins`. The host links imports under module `code-plus-plus`:

- `tool_execute` — proxies to location `ToolRegistry` via `set_wasm_tool_proxy`
- `config_transform` — parses JSON config, applies transform metadata, stores result
- `log` — structured logging to tracing

`PluginHost::invoke_wasm_export(name, export)` instantiates a module and calls an export (default `_initialize`).

Native Rust plugins implement `SessionHook` and register via `HookRegistry::register`. See `examples/plugins/hello-hook/`.

| JS plugin `hooks` | `SessionHook` trait in `code-plus-plus-plugin` |
| Bun plugin runtime | WASM sandbox in `.code-plus-plus/plugins/` |

## Session integration

On location creation (`location.rs`):

1. `McpConnectionPool` connects configured servers
2. `PluginHost` loads WASM from config and `.code-plus-plus/plugins`
3. `HookRegistry` receives hooks from the plugin host
4. `set_wasm_tool_proxy` wires WASM `tool_execute` to a location-scoped `ToolRegistry`
5. `SessionRunner` registers MCP bridged tools and skill tool, runs hooks around tool execution and system prompt transforms
6. Catalog changes call `LocationMap::reload_materialization()` to refresh skill/MCP tools on cached runners
