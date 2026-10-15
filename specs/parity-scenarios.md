# Code++ Parity Scenarios

|-----------|------------|-------------------|
| Prompt admission | `prompt_admission.json` | sessions.prompt queue |
| Steer delivery | `steer_delivery.json` | delivery=steer |
| Queue no resume | `queue_no_resume.json` | resume=false |
| Steer pending | `steer_delivery_pending.json` | steer inbox |
| Idempotent prompt | `idempotent_prompt.json` | message id reuse |
| Double queue | `double_queue.json` | multiple inbox |
| Context baseline | `context_baseline_available.json` | Context Epoch init |
| AGENTS.md | `agents_md_loaded.json` | project/agents_md source |
| Session create | `session_create_only.json` | sessions.create |
| Interrupt noop | `interrupt_noop.json` | sessions.interrupt idle |
| Interrupt preserves inbox | `interrupt_preserves_inbox.json` | interrupt keeps durable inbox |
| Session move | `move_session.json` | epoch clear on move |
| Fork | `fork_session.json` | sessions.fork |
| Active sessions | `active_sessions_empty.json` | sessions.active |
| Permission ask | `permission_ask.json` | permission.requested / pending |
| Compaction epoch | `compaction_epoch.json` | Context Epoch init |
| Compaction replaces baseline | `compaction_replaces_baseline.json` | Rule 5: rebuild from complete context |
| Large output paged | `large_output_paged.json` | bounded projection + tool_result_page reassembly |
| Revert stage | `revert_stage.json` | sessions.revert.stage |
| Overflow retry | `overflow_retry.json` | context overflow detection |
| MCP tool bridge | `mcp_tool.json` | MCP bridged tool naming |
| MCP session E2E | `mcp_session_test.rs` | stdio MCP subprocess in LLM session |
| Skill tool | `skill_tool.json` | skill catalog discovery |
| Plan deny | `plan_deny.json` | plan tool permission denied |
| Steer mid-turn | `steer_mid_turn.json` | steer promotes during provider turn |
| Drain queue | `drain_queue_promote.json` | queue promotion at safe boundary |
| Mandatory unavailable | `mandatory_context_unavailable.json` | prompt stays inbox when no AGENTS.md/instructions |
| Todo write | `todo_write.json` | Todo.Service update + todo.updated |
| Activity turn summary | `activity_turn_summary.json` | provider turn grouping + tool merge |
| Tool metadata diff | `tool_metadata_diff.json` | write/edit diff_stats in tool.completed |
