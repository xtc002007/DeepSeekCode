ALTER TABLE sessions ADD COLUMN next_message_seq INTEGER NOT NULL DEFAULT 1;

UPDATE sessions
SET next_message_seq = COALESCE(
    (SELECT MAX(seq) + 1 FROM messages WHERE messages.session_id = sessions.id),
    1
);
