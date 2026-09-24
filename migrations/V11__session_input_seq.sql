-- Sequence inbox admissions so revert can cut by pure seq instead of
-- wall-clock admitted_at (same-millisecond/reordered rows left orphans).
ALTER TABLE session_input ADD COLUMN admitted_seq INTEGER NOT NULL DEFAULT 0;

-- Backfill per-session admission order from rowid (insertion order).
UPDATE session_input
SET admitted_seq = (
    SELECT COUNT(*) FROM session_input AS s2
    WHERE s2.session_id = session_input.session_id
      AND s2.rowid <= session_input.rowid
)
WHERE admitted_seq = 0;

CREATE INDEX IF NOT EXISTS idx_session_input_seq ON session_input(session_id, admitted_seq);
