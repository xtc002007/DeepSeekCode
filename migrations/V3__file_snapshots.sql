CREATE TABLE IF NOT EXISTS file_snapshots (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    message_id TEXT,
    file_path TEXT NOT NULL,
    snapshot_path TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_file_snapshots_session ON file_snapshots(session_id, created_at);

CREATE TABLE IF NOT EXISTS session_revert (
    session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id),
    staged_message_id TEXT,
    staged_at TEXT,
    diff_json TEXT
);
