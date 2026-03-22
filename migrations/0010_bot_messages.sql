-- Track bot-posted embeds and the users mentioned in them.
-- When a mentioned user leaves, we find and edit these embeds
-- to replace broken <@id> mentions with display names.
CREATE TABLE bot_messages (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  guild_id            TEXT NOT NULL,
  channel_id          TEXT NOT NULL,
  message_id          TEXT NOT NULL,
  message_type        TEXT NOT NULL,     -- 'join' | 'leave' | 'kick' | 'ban'
  member_id           TEXT NOT NULL,     -- the user who joined/left
  inviter_id          TEXT,              -- who invited them (join embeds)
  moderator_id        TEXT,              -- who banned/kicked (kick/ban embeds)
  previous_message_id TEXT,              -- cross-link to prior related embed
  created_at          TEXT NOT NULL
);

CREATE INDEX idx_bot_messages_guild_member
  ON bot_messages (guild_id, member_id);
CREATE INDEX idx_bot_messages_guild_inviter
  ON bot_messages (guild_id, inviter_id);
CREATE INDEX idx_bot_messages_guild_moderator
  ON bot_messages (guild_id, moderator_id);
