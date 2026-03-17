-- FTS5 for fast name search (accent-insensitive + prefix)
CREATE VIRTUAL TABLE IF NOT EXISTS usernames_fts USING fts5(
  guild_id UNINDEXED,      -- filter-only field (not tokenized)
  user_id  UNINDEXED,      -- value we return
  account_username,        -- searchable
  server_username,         -- searchable
  label,                   -- searchable composite label
  label_norm,              -- lowercase label for stricter prefix hits
  tokenize = 'unicode61 remove_diacritics 2',
  prefix = '2 3 4'         -- build prefix indexes for 2/3/4+ char prefixes
);
