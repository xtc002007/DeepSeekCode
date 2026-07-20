# DeepSeekCode Session Runtime Specification

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

Plan mode produces implementation plans under `.deepseekcode/plan/<plan_id>/`:

- `PLAN.md` — flexible markdown body (no fixed section schema)
- `meta.json` — machine metadata (`plan_id`, `session_id`, `title`, `status`, timestamps)

### Plan agent

- Tools: `read`, `grep`, `glob`, `list`, `write`, `edit`, `skill`, `question`, `plan_complete`
- Policy: block `edit`/`bash` globally; permit `.deepseekcode/plan/**` writes; permit `question` and `plan_complete`
- Runner always injects the built-in plan system prompt (config `agents.plan.system_prompt` appends)

### Completion

`plan_complete` validates the plan file exists with minimum content, writes `meta.json`, and emits `plan.completed` with `{ plan_id, plan_path, title, summary, session_id }`.

### Question tool

`question` supports `options` plus `allow_custom` (default `true`). Desktop UI always offers a final custom-input path when options are present.

### Implement from plan

Desktop switches the session to `build`, prefills the composer with an implementation prompt referencing `plan_path`, and waits for the user to submit. Dismiss state for the prominent implement dock persists in `session_plan_ui`.

## Plan / Build Workflow

Plan mode produces implementation plans under `<project>/.deepseekcode/plan/<plan_id>/`:

- `PLAN.md` — flexible markdown body (no fixed section template)
- `meta.json` — machine metadata (`plan_id`, `session_id`, `title`, `status`, timestamps)

### Plan agent

- Tools: `read`, `grep`, `glob`, `list`, `write`, `edit`, `skill`, `question`, `plan_complete`
- Policy: block `edit`/`bash` outside `.deepseekcode/plan/**`; permit `question` and `plan_complete`
- Runner always injects the built-in plan system prompt (config `agents.plan.system_prompt` appends)

### plan_complete

Validates `primary_path` exists with minimum content length, writes `meta.json`, emits `plan.completed`.

### Question tool

- `allow_custom` defaults to `true`
- Desktop UI appends an "Other (custom input)" option and text field when options are provided

### Implement from plan

Desktop switches session agent to `build`, prefills the composer with an implementation prompt referencing the plan path, and waits for user confirmation before sending.

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

See `crates/deepseekcode-core/tests/fixtures/` for Given/When/Then JSON scenarios.
