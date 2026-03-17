ALTER TABLE memberships ADD COLUMN invite_code TEXT;

-- Tracks one-time notices sent to guilds (patch notes, permission warnings, etc.)
CREATE TABLE IF NOT EXISTS guild_notices (
  guild_id   TEXT NOT NULL,
  notice_key TEXT NOT NULL,
  sent_at    TEXT NOT NULL,
  PRIMARY KEY (guild_id, notice_key)
);
