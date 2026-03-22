-- Rename server_username to server_nickname for clarity (it stores member.nick, not a username).
ALTER TABLE memberships RENAME COLUMN server_username TO server_nickname;

-- Add global display name column (user.global_name from Discord API).
ALTER TABLE memberships ADD COLUMN display_name TEXT;

-- Recreate FTS table with the renamed column.
-- Data is rebuilt on startup via rebuild_usernames_fts_for_guild().
DROP TABLE IF EXISTS usernames_fts;
CREATE VIRTUAL TABLE usernames_fts USING fts5(
  guild_id UNINDEXED,
  user_id UNINDEXED,
  account_username,
  server_nickname,
  label,
  label_norm,
  tokenize = 'unicode61 remove_diacritics 2',
  prefix = '2 3 4'
);
