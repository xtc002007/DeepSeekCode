# Code++ Session Runtime Specification

Implementation must match these rules; tests assert parity via JSON fixtures.

## Terminology

| Term | Definition |
|------|------------|
| **System Context** | Structured contextual facts presented to the model as baseline + chronological updates |
| **Session History** | Projected conversation after compaction and Context Epoch cutoffs |
| **Context Source** | Independently observed typed value with stable key, codec, loaders, renderers |
| **Context Epoch** | Span where one baseline System Context remains immutable until compaction or move |
| **Safe Provider-Turn Boundary** | Point before a provider call where context changes may be admitted |
| **Admitted Prompt** | Durable user input in inbox, not yet in Session History |
| **Prompt Promotion** | Transition from inbox to model-visible user message |
| **Provider Turn** | One model request + projected response |
| **Session Drain** | Process-local span promoting input and running Provider Turns |

## API Semantics

### sessions.create
- Omitted ID generates internal Session ID
- Supplied ID creates when absent; reused ID returns existing identity

### sessions.prompt
- Omitted message ID generates one; exact reuse returns same admission receipt
- `delivery`: `steer` (promote at next safe boundary during continuation) or `queue`
- `resume: true` schedules execution after admission; `false` admits only

### sessions.interrupt
- Interrupts active execution; preserves durable inbox rows
- Idle or missing session is no-op

### sessions.fork
- Creates a new session inheriting a prefix of history (`until_message_id`, or all)
- Single atomic transaction: session row + messages + unpromoted inbox + epoch; any failure rolls back everything, never a half-forked session
- Epoch is inherited as a snapshot clamped to the forked prefix (`min(src_baseline, fork_max)`), so the fork's baseline never references messages it does not contain
- Unknown `agent` values are rejected (no silent fallback to `build`)

## Context Epoch Rules

1. First complete baseline initializes epoch before any prompt becomes model-visible
2. Unavailable initial context blocks turn; prompt stays pending and retryable
3. At safe boundary: promote input → reconcile context → run provider turn
4. Changed sources combine into one chronological system message
5. Compaction replaces baseline from current complete context
6. Session move clears epoch; destination initializes fresh baseline

## Tool Execution

- One `llm.stream` per provider turn
- Persist each tool call before settlement
- Plan agent denies write/edit/bash/apply_patch by default
- Large outputs stored in managed files; history keeps bounded projection

## Plan / Build Workflow

Plan mode produces implementation plans under `<project>/.code-plus-plus/plan/<plan_id>/`:

- `PLAN.md` — flexible markdown body (no fixed section schema; "template"
  was a wording variant of the same rule)
- `meta.json` — machine metadata (`plan_id`, `session_id`, `title`, `status`, timestamps)

### Plan agent

- Provider-visible tools: `read`, `grep`, `glob`, `list`, `skill`, `question`,
  `plan_complete`, `tool_result_page` (paged recovery for large outputs).
  `write`/`edit`/`bash`/`apply_patch` are NOT provider-visible in plan mode;
  writes are confined to `.code-plus-plus/plan/**` via the plan-scoped policy.
- Policy: `block` `edit`/`bash`/`apply_patch` outside `.code-plus-plus/plan/**`;
  `permit` writes under `.code-plus-plus/plan/**`; `permit` `question` and
  `plan_complete`. (Earlier drafts said "block globally"; the scoped form
  here is canonical.)
- Runner always injects the built-in plan system prompt (config `agents.plan.system_prompt` appends)

### plan_complete

Validates `primary_path` (alias: `plan_path`) exists with minimum content
length (200 bytes, Unicode-aware char count), writes `meta.json`, emits
`plan.completed` with `{ plan_id, plan_path, title, summary, session_id }`.

### Question tool

- `options` plus `allow_custom` (default `true`)
- Desktop UI always offers a final custom-input path ("Other (custom input)"
  option + text field) when options are present

### Implement from plan

Desktop switches session agent to `build`, prefills the composer with an
implementation prompt referencing the plan path, and waits for user
confirmation (submit) before sending.

### Persistence

- `session_plan_ui` stores per-session dismiss state for the prominent implement dock
- `GET /api/v1/session/{id}/plans` lists completed plans; `POST .../plans/{plan_id}/dismiss` persists dismiss

## Event Types

- `session.created`, `prompt.admitted`, `prompted`
- `message.delta`, `message.completed`
- `tool.started`, `tool.completed`
- `plan.completed`
- `permission.requested`, `permission.resolved` (legacy alias)
- `policy.prompted`, `policy.resolved`
- `context.updated`, `session.compacted`, `session.interrupted`

## Behavioral Test Fixtures

See `crates/code-plus-plus-core/tests/fixtures/` for Given/When/Then JSON scenarios.
