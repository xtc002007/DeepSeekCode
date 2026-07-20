CREATE TABLE IF NOT EXISTS session_todos (
    session_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    content TEXT NOT NULL,
    status TEXT NOT NULL,
    priority TEXT NOT NULL DEFAULT 'medium',
    PRIMARY KEY (session_id, position)
);

CREATE INDEX IF NOT EXISTS idx_session_todos_session ON session_todos(session_id);

ALTER TABLE sessions ADD COLUMN parent_session_id TEXT;
