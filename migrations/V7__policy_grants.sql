CREATE TABLE IF NOT EXISTS policy_grants (
    id TEXT PRIMARY KEY NOT NULL,
    project_dir TEXT NOT NULL,
    scope TEXT NOT NULL,
    pattern TEXT NOT NULL,
    action TEXT NOT NULL DEFAULT 'permit',
    created_at TEXT NOT NULL,
    UNIQUE(project_dir, scope, pattern)
);

CREATE INDEX IF NOT EXISTS idx_policy_grants_project ON policy_grants(project_dir);

-- Migrate legacy always/allowed permissions into policy_grants where possible.
INSERT OR IGNORE INTO policy_grants (id, project_dir, scope, pattern, action, created_at)
SELECT
    id,
    COALESCE((SELECT working_directory FROM sessions WHERE sessions.id = permissions.session_id LIMIT 1), '.'),
    tool_name,
    pattern,
    'permit',
    created_at
FROM permissions
WHERE status IN ('always', 'allowed');
