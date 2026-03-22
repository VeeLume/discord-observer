-- Schema v2: Normalize the god table into proper entities.
--
-- memberships → members (identity) + stays (lifecycle) + name_changes (history)
-- bot_messages → bot_messages (simplified) + bot_message_mentions (junction)
-- guild_settings.first_seen_at → guilds.first_seen_at

-- ============================================================
-- Phase 1: Create new tables
-- ============================================================

-- Guild lifecycle metadata (not a setting).
CREATE TABLE guilds (
    guild_id      TEXT PRIMARY KEY,
    first_seen_at TEXT NOT NULL
);

-- One row per (guild, user). Current/latest-known names.
CREATE TABLE members (
    guild_id          TEXT NOT NULL,
    user_id           TEXT NOT NULL,
    account_username  TEXT,
    display_name      TEXT,
    server_nickname   TEXT,
    first_seen_at     TEXT NOT NULL,
    is_present        INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (guild_id, user_id)
);
CREATE INDEX idx_members_guild_present ON members (guild_id, is_present);

-- Append-only name change log. Full snapshot at each change point.
CREATE TABLE name_changes (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id          TEXT NOT NULL,
    user_id           TEXT NOT NULL,
    changed_at        TEXT NOT NULL,
    account_username  TEXT,
    display_name      TEXT,
    server_nickname   TEXT
);
CREATE INDEX idx_name_changes_member ON name_changes (guild_id, user_id, id);

-- One row per join-to-leave cycle.
CREATE TABLE stays (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id        TEXT NOT NULL,
    user_id         TEXT NOT NULL,
    joined_at       TEXT NOT NULL,
    left_at         TEXT,
    departure_type  TEXT,
    moderator_id    TEXT,
    invite_code     TEXT,
    inviter_id      TEXT
);
CREATE INDEX idx_stays_guild_user ON stays (guild_id, user_id);
CREATE INDEX idx_stays_guild_id   ON stays (guild_id, id);

-- Simplified bot message tracking — links to stay, not to individual users.
CREATE TABLE bot_messages_new (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id            TEXT NOT NULL,
    channel_id          TEXT NOT NULL,
    message_id          TEXT NOT NULL,
    message_type        TEXT NOT NULL,
    stay_id             INTEGER,
    previous_message_id TEXT,
    created_at          TEXT NOT NULL
);

-- Junction table: which users does each embed reference?
CREATE TABLE bot_message_mentions (
    bot_message_id  INTEGER NOT NULL,
    user_id         TEXT NOT NULL,
    mention_role    TEXT NOT NULL,
    PRIMARY KEY (bot_message_id, user_id, mention_role)
);
CREATE INDEX idx_bmm_user ON bot_message_mentions (user_id);

-- ============================================================
-- Phase 2: Migrate data
-- ============================================================

-- 2a. Populate guilds from guild_settings.
INSERT INTO guilds (guild_id, first_seen_at)
SELECT guild_id, COALESCE(first_seen_at, datetime('now'))
FROM guild_settings;

-- Also capture any guild_ids that appear in memberships but not in guild_settings.
INSERT OR IGNORE INTO guilds (guild_id, first_seen_at)
SELECT DISTINCT guild_id, datetime('now')
FROM memberships
WHERE guild_id NOT IN (SELECT guild_id FROM guilds);

-- 2b. Populate members: latest names per (guild, user), earliest join, current presence.
INSERT INTO members (guild_id, user_id, account_username, display_name, server_nickname, first_seen_at, is_present)
WITH first_join AS (
    SELECT guild_id, user_id, MIN(joined_at) AS first_joined
    FROM memberships
    GROUP BY guild_id, user_id
),
last_stay AS (
    SELECT guild_id, user_id, MAX(id) AS last_id
    FROM memberships
    GROUP BY guild_id, user_id
)
SELECT
    m.guild_id,
    m.user_id,
    m.account_username,
    m.display_name,
    m.server_nickname,
    f.first_joined,
    CASE WHEN m.left_at IS NULL THEN 1 ELSE 0 END
FROM last_stay l
JOIN memberships m ON m.id = l.last_id
JOIN first_join f ON f.guild_id = m.guild_id AND f.user_id = m.user_id;

-- 2c. Populate stays from memberships (all rows).
INSERT INTO stays (id, guild_id, user_id, joined_at, left_at, departure_type, moderator_id, invite_code, inviter_id)
SELECT
    id,
    guild_id,
    user_id,
    joined_at,
    left_at,
    CASE
        WHEN left_at IS NULL THEN NULL
        WHEN banned = 1 THEN 'ban'
        WHEN kicked = 1 THEN 'kick'
        ELSE 'leave'
    END,
    moderator_id,
    invite_code,
    inviter_id
FROM memberships;

-- 2d. Populate bot_messages_new with stay_id derived from matching member + guild.
INSERT INTO bot_messages_new (id, guild_id, channel_id, message_id, message_type, stay_id, previous_message_id, created_at)
SELECT
    bm.id,
    bm.guild_id,
    bm.channel_id,
    bm.message_id,
    bm.message_type,
    (SELECT MAX(s.id) FROM stays s
     WHERE s.guild_id = bm.guild_id AND s.user_id = bm.member_id
       AND s.id <= bm.id),
    bm.previous_message_id,
    bm.created_at
FROM bot_messages bm;

-- 2e. Explode bot_messages user references into the junction table.
INSERT INTO bot_message_mentions (bot_message_id, user_id, mention_role)
SELECT id, member_id, 'member' FROM bot_messages WHERE member_id IS NOT NULL
UNION ALL
SELECT id, inviter_id, 'inviter' FROM bot_messages WHERE inviter_id IS NOT NULL
UNION ALL
SELECT id, moderator_id, 'moderator' FROM bot_messages WHERE moderator_id IS NOT NULL;

-- ============================================================
-- Phase 3: Swap tables
-- ============================================================

-- Drop old tables.
DROP TABLE memberships;
DROP TABLE bot_messages;

-- Rename new bot_messages into place.
ALTER TABLE bot_messages_new RENAME TO bot_messages;

-- Recreate guild_settings without first_seen_at.
CREATE TABLE guild_settings_new (
    guild_id              TEXT PRIMARY KEY,
    join_log_channel_id   TEXT,
    leave_log_channel_id  TEXT,
    mod_log_channel_id    TEXT,
    notices_enabled       INTEGER NOT NULL DEFAULT 1
);

INSERT INTO guild_settings_new (guild_id, join_log_channel_id, leave_log_channel_id, mod_log_channel_id, notices_enabled)
SELECT guild_id, join_log_channel_id, leave_log_channel_id, mod_log_channel_id, notices_enabled
FROM guild_settings;

DROP TABLE guild_settings;
ALTER TABLE guild_settings_new RENAME TO guild_settings;

-- ============================================================
-- Phase 4: Create indexes on swapped tables
-- ============================================================

CREATE INDEX idx_bot_messages_stay ON bot_messages (stay_id);
CREATE INDEX idx_bot_messages_guild_message ON bot_messages (guild_id, message_id);
