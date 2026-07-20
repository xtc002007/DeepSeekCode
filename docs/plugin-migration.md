# Plugin & MCP Migration Guide

This document describes how DeepSeekCode integrates MCP servers, skills, and the plugin hook surface.

## MCP Servers

### Configuration

MCP servers are declared in `deepseekcode.jsonc` under `mcp.servers`:

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

Pass `x-deepseekcode-directory` to target the project location.

## Skills

Skills are discovered from:

- Paths in config `skills`
- `.deepseekcode/skills/*/SKILL.md`
- Optional remote index URL (`skills_index_url`)

`SkillCatalog` injects a summary into system context via `skill_guidance`. Agents load full instructions with the builtin `skill` tool:

```json
{ "name": "my-skill" }
```

`GET /api/v1/skills` triggers `reload_materialization()` to refresh skill tools on active runners.


| Hook | When |
|------|------|
| `tool.execute.before` | Before a tool runs; can mutate arguments |
| `tool.execute.after` | After a tool succeeds; can mutate output |
| `chat.system.transform` | Before each provider turn; can extend system prompt |
| `session.compacting` | During compaction; can inject context or replace summary prompt |

### WASM plugins

Place `.wasm` files in `.deepseekcode/plugins/` or list paths in config `plugins`. The host links imports under module `deepseekcode`:

- `tool_execute` — proxies to location `ToolRegistry` via `set_wasm_tool_proxy`
- `config_transform` — parses JSON config, applies transform metadata, stores result
- `log` — structured logging to tracing

`PluginHost::invoke_wasm_export(name, export)` instantiates a module and calls an export (default `_initialize`).

Native Rust plugins implement `SessionHook` and register via `HookRegistry::register`. See `examples/plugins/hello-hook/`.

| JS plugin `hooks` | `SessionHook` trait in `deepseekcode-plugin` |
| Bun plugin runtime | WASM sandbox in `.deepseekcode/plugins/` |

## Session integration

On location creation (`location.rs`):

1. `McpConnectionPool` connects configured servers
2. `PluginHost` loads WASM from config and `.deepseekcode/plugins`
3. `HookRegistry` receives hooks from the plugin host
4. `set_wasm_tool_proxy` wires WASM `tool_execute` to a location-scoped `ToolRegistry`
5. `SessionRunner` registers MCP bridged tools and skill tool, runs hooks around tool execution and system prompt transforms
6. Catalog changes call `LocationMap::reload_materialization()` to refresh skill/MCP tools on cached runners
