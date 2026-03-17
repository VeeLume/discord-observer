-- memberships: one row per join->leave stint
CREATE TABLE IF NOT EXISTS memberships (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  guild_id          TEXT NOT NULL,
  user_id           TEXT NOT NULL,
  joined_at         TEXT NOT NULL,    -- RFC2822 string
  left_at           TEXT,             -- RFC2822 string or NULL while open
  banned            BOOLEAN NOT NULL DEFAULT 0,

  -- last-known names for search/history display
  account_username  TEXT,             -- Discord account username
  server_username   TEXT              -- guild nickname
);

-- per-guild log channels
CREATE TABLE IF NOT EXISTS guild_settings (
  guild_id            TEXT PRIMARY KEY,
  join_log_channel_id TEXT,
  leave_log_channel_id TEXT,
  mod_log_channel_id   TEXT
);

CREATE INDEX IF NOT EXISTS idx_memberships_guild_user
  ON memberships (guild_id, user_id);

CREATE INDEX IF NOT EXISTS idx_memberships_guild_lastid
  ON memberships (guild_id, id);

CREATE INDEX IF NOT EXISTS idx_memberships_name_search
  ON memberships (guild_id, account_username, server_username);
