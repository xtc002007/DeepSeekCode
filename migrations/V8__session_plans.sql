CREATE TABLE IF NOT EXISTS session_plan_ui (
    session_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    implement_prompt_dismissed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session_id, plan_id)
);
