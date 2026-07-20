CREATE TABLE IF NOT EXISTS mcp_connections (
    server_name TEXT NOT NULL,
    location_path TEXT NOT NULL,
    connected INTEGER NOT NULL DEFAULT 1,
    tools_json TEXT NOT NULL DEFAULT '[]',
    connected_at TEXT NOT NULL,
    PRIMARY KEY (server_name, location_path)
);

CREATE INDEX IF NOT EXISTS idx_mcp_connections_location ON mcp_connections(location_path);
