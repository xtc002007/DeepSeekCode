# DeepSeekCode Policy System

Native permission governance for tool execution. Replaces the legacy allow/deny list with a three-state ruleset model.

## Semantics

| Action | Meaning |
|--------|---------|
| `permit` | Run without prompting |
| `prompt` | Block until user confirms in UI |
| `block` | Hard deny; no prompt |

Legacy aliases (`allow` / `ask` / `deny`) deserialize to the same values.

## Configuration

In the DeepSeekCode project config file (`deepseekcode.jsonc`):

```jsonc
{
  "policy": {
    "bash": { "*": "prompt", "git *": "permit", "rm *": "block" },
    "read": { "*": "permit", "*.env": "block" },
    "outside_workspace": { "*": "prompt", "~/.ssh": "block" }
  },
  "policy_mode": "interactive",
  "agents": {
    "plan": {
      "policy": {
        "edit": { "*": "block", ".deepseekcode/plan/**": "permit" }
      }
    }
  }
}
```

## Evaluation

Rules merge in order (later wins on pattern match):

1. Agent defaults (`build` / `plan` / `general`)
2. Global `policy`
3. Agent-specific `agents.<name>.policy`
4. Session grants (in-memory, `once`)
5. Project grants (SQLite `policy_grants`, from **Always (project)**)

For each extracted pattern, the last matching rule applies. Any `block` → denied; any `prompt` (and no block) → UI prompt; all `permit` → run.

## Scopes

| Scope | Tools / guards |
|-------|----------------|
| `read`, `edit`, `bash`, `grep`, `glob`, `list`, `webfetch`, `websearch`, `skill`, `task`, `plan`, `lsp`, `question` | Per-tool patterns |
| `outside_workspace` | Paths outside project worktree |
| `loop_guard` | Same tool+args repeated 3× |

## User replies

| Reply | Effect |
|-------|--------|
| `once` | Allow this invocation only |
| `project` | Persist permit grant for scope/pattern in SQLite |
| `reject` | Tool failure returned to model (optional message) |

## Events

- `policy.prompted` / `policy.resolved` (primary)
- `permission.requested` / `permission.resolved` (compat alias, one release cycle)

## HTTP API

| Method | Path |
|--------|------|
| GET | `/api/v1/policy/pending` |
| POST | `/api/v1/policy/{id}/reply` |
| GET | `/api/v1/policy/grants` |
| DELETE | `/api/v1/policy/grants/{id}` |
| PUT | `/api/v1/policy/config` |

Legacy `/api/v1/permission/*` routes remain as thin wrappers carrying a
`Sunset` response header, and are **removed in v0.3.0** together with the
legacy `permissions` table (see `PermissionStore` deprecation). Migration:

| Legacy | Replacement | In-repo callers migrated |
|--------|-------------|--------------------------|
| `GET /api/v1/permission/pending` | `GET /api/v1/policy/pending` | desktop, CLI |
| `POST /api/v1/permission/{id}/respond` | `POST /api/v1/policy/{id}/reply` | desktop, CLI |
| `GET /api/v1/permission/grants` | `GET /api/v1/policy/grants` | CLI (`policy grants`) |
| `DELETE /api/v1/permission/grants/{id}` | `DELETE /api/v1/policy/grants/{id}` | CLI (`policy grant-delete`) |
| `PUT /api/v1/permission/config` | `PUT /api/v1/policy/config` | settings UI |
| `permissions` SQLite table | `policy_grants` table | server wrappers only |

## Desktop UI

- **Permission dock** above composer when pending; blocks input until resolved
- **Settings → Permissions**: policy rules editor, remembered grants, presets (dev / read-only / strict)

## Hot reload

`engine.reload_catalog()` reloads config and calls `PolicyService::reload_policy()`.

## Policy mode (Phase 2)

`policy_mode` in the DeepSeekCode config file (`deepseekcode.jsonc`):

| Value | Behavior |
|-------|----------|
| `interactive` (default) | Prompt the user for `prompt` rules |
| `auto` | YOLO — auto-permit non-`block` rules; no dock/composer block |

CLI: `deepseekcode serve --auto`, `deepseekcode run --auto`, or `DEEPSEEKCODE_POLICY_MODE=auto`.

Headless replies: `deepseekcode policy pending`, `deepseekcode policy reply --id … --reply once|project|reject [--message …]`.
