-- P0 hardening: missing indexes, composite event lookup, permission scoping.
-- All statements are idempotent (IF NOT EXISTS) so fresh and upgraded DBs converge.

-- Session lookup by working directory (list_for_working_directory).
CREATE INDEX IF NOT EXISTS idx_sessions_working_directory ON sessions(working_directory);

-- Event tail by session (projector / SSE replay).
CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq);

-- Inbox promotion order.
CREATE INDEX IF NOT EXISTS idx_session_input_session_promoted ON session_input(session_id, promoted, admitted_seq);

-- Messages in session order.
CREATE INDEX IF NOT EXISTS idx_messages_session_seq ON messages(session_id, seq);

-- Legacy permissions: add project scope for global rows so one project
-- cannot overwrite another's `session_id IS NULL` rules. Nullable for
-- backwards compat; new writes set it.
ALTER TABLE permissions ADD COLUMN project_dir TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_permissions_scope ON permissions(tool_name, pattern, project_dir);

-- Policy grants lookup by project.
CREATE INDEX IF NOT EXISTS idx_policy_grants_project ON policy_grants(project_dir);
