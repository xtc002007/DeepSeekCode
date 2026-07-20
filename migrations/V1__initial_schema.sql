CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY NOT NULL,
    location_id TEXT NOT NULL,
    title TEXT,
    agent TEXT NOT NULL DEFAULT 'build',
    model_id TEXT,
    provider_id TEXT,
    working_directory TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session_input (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    prompt TEXT NOT NULL,
    delivery TEXT NOT NULL DEFAULT 'queue',
    admitted_at TEXT NOT NULL,
    promoted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    created_at TEXT NOT NULL,
    seq INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS message_parts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL REFERENCES messages(id),
    part_index INTEGER NOT NULL,
    kind TEXT NOT NULL,
    text TEXT,
    tool_call_id TEXT,
    tool_name TEXT,
    tool_input TEXT,
    tool_output TEXT
);

CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    session_id TEXT,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS context_epochs (
    session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id),
    baseline TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    created_at TEXT NOT NULL,
    baseline_seq INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS permissions (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT,
    tool_name TEXT NOT NULL,
    pattern TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS credentials (
    provider_id TEXT PRIMARY KEY NOT NULL,
    encrypted_value TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    path TEXT PRIMARY KEY NOT NULL,
    last_opened_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_session_input_session ON session_input(session_id, promoted);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_events_seq ON events(seq);
