-- Notice definitions created by the bot owner via /notice create.
-- Replaces the hardcoded NOTICES const in notices.rs.
CREATE TABLE IF NOT EXISTS notices (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  key          TEXT NOT NULL UNIQUE,
  title        TEXT NOT NULL,
  body         TEXT NOT NULL,
  color        INTEGER NOT NULL DEFAULT 3447003,
  current_only INTEGER NOT NULL DEFAULT 1,
  created_at   TEXT NOT NULL
);

-- Track when the bot first connected to each guild.
-- Used to skip current_only notices for guilds that joined after the notice was created.
ALTER TABLE guild_settings ADD COLUMN first_seen_at TEXT;
