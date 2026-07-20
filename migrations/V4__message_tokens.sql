ALTER TABLE messages ADD COLUMN input_tokens INTEGER;
ALTER TABLE messages ADD COLUMN output_tokens INTEGER;
ALTER TABLE messages ADD COLUMN reasoning_tokens INTEGER;
ALTER TABLE messages ADD COLUMN cache_tokens INTEGER;
