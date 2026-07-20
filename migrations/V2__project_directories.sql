CREATE TABLE IF NOT EXISTS project_directories (
    id TEXT PRIMARY KEY NOT NULL,
    project_path TEXT NOT NULL,
    directory_path TEXT NOT NULL,
    strategy TEXT NOT NULL DEFAULT 'default',
    created_at TEXT NOT NULL,
    UNIQUE(project_path, directory_path)
);

CREATE INDEX IF NOT EXISTS idx_project_directories_project ON project_directories(project_path);
