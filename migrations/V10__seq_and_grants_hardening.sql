-- Harden seq defaults and grant dedup (fixes V1/V9 drift + V7 churn).
-- V1 messages.seq DEFAULT 0 vs V9 next_message_seq DEFAULT 1: normalize
-- legacy 0-seqs so visibility filters (seq >= cutoff) never hide them.
UPDATE messages SET seq = seq + 1 WHERE seq = 0;

-- Ensure next_message_seq tracks MAX(seq)+1 after the shift above.
UPDATE sessions
SET next_message_seq = COALESCE(
    (SELECT MAX(seq) + 1 FROM messages WHERE messages.session_id = sessions.id),
    1
);

-- Deduplicate legacy permission rows (tool_name,pattern) keeping newest.
DELETE FROM permissions
WHERE id NOT IN (
    SELECT id FROM (
        SELECT id, ROW_NUMBER() OVER (PARTITION BY tool_name, pattern ORDER BY created_at DESC) AS rn
        FROM permissions WHERE session_id IS NULL
    ) WHERE rn = 1
) AND session_id IS NULL;

-- Document V8 naming: session_plan_ui is the dismiss-state table (not full
-- plan metadata, which lives in events). No rename to avoid breaking clients.
