//! Bot embed tracking and mention junction.
//!
//! Records every embed the bot posts, which users it references, and
//! provides lookups for cross-linking and mention editing.

use anyhow::Result;
use serenity::all::{GuildId, Timestamp};

use crate::db::Db;

// ── Types ────────────────────────────────────────────────────────────────────

/// Type of bot-posted embed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Join,
    Leave,
    Kick,
    Ban,
}

impl MessageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Join => "join",
            Self::Leave => "leave",
            Self::Kick => "kick",
            Self::Ban => "ban",
        }
    }

    pub fn is_departure(&self) -> bool {
        matches!(self, Self::Leave | Self::Kick | Self::Ban)
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "join" => Self::Join,
            "leave" => Self::Leave,
            "kick" => Self::Kick,
            "ban" => Self::Ban,
            _ => Self::Leave, // defensive fallback
        }
    }
}

/// Role a user plays in a bot-posted embed.
#[derive(Debug, Clone, Copy)]
pub enum MentionRole {
    Member,
    Inviter,
    Moderator,
}

impl MentionRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Inviter => "inviter",
            Self::Moderator => "moderator",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BotMessageRow {
    pub id: i64,
    pub channel_id: String,
    pub message_id: String,
    pub message_type: MessageType,
    pub stay_id: Option<i64>,
    pub previous_message_id: Option<String>,
}

/// Raw row from sqlx (message_type as String). Mapped to BotMessageRow.
#[derive(Debug, Clone)]
struct RawBotMessageRow {
    pub id: i64,
    pub channel_id: String,
    pub message_id: String,
    pub message_type: String,
    pub stay_id: Option<i64>,
    pub previous_message_id: Option<String>,
}

impl From<RawBotMessageRow> for BotMessageRow {
    fn from(r: RawBotMessageRow) -> Self {
        Self {
            id: r.id,
            channel_id: r.channel_id,
            message_id: r.message_id,
            message_type: MessageType::from_db(&r.message_type),
            stay_id: r.stay_id,
            previous_message_id: r.previous_message_id,
        }
    }
}

// ── Writes ───────────────────────────────────────────────────────────────────

/// Record a bot-posted embed and its user mentions. Returns the bot_message ID.
pub async fn record(
    db: &Db,
    guild_id: GuildId,
    channel_id: &str,
    message_id: &str,
    message_type: MessageType,
    stay_id: Option<i64>,
    previous_message_id: Option<&str>,
    mentions: &[(&str, MentionRole)],
) -> Result<i64> {
    let gid = guild_id.to_string();
    let mt = message_type.as_str();
    let created_at = Timestamp::now().to_rfc2822();

    let result = sqlx::query!(
        r#"
        INSERT INTO bot_messages (guild_id, channel_id, message_id, message_type,
            stay_id, previous_message_id, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
        gid,
        channel_id,
        message_id,
        mt,
        stay_id,
        previous_message_id,
        created_at
    )
    .execute(&db.pool)
    .await?;

    let bot_msg_id = result.last_insert_rowid();

    for (user_id, role) in mentions {
        let role_str = role.as_str();
        sqlx::query!(
            r#"
            INSERT INTO bot_message_mentions (bot_message_id, user_id, mention_role)
            VALUES (?, ?, ?)
            "#,
            bot_msg_id,
            *user_id,
            role_str
        )
        .execute(&db.pool)
        .await?;
    }

    Ok(bot_msg_id)
}

// ── Reads ────────────────────────────────────────────────────────────────────

/// Find all embeds in a guild that reference a user (via junction table).
pub async fn find_referencing_user(
    db: &Db,
    guild_id: GuildId,
    user_id: &str,
) -> Result<Vec<BotMessageRow>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query_as!(
        RawBotMessageRow,
        r#"
        SELECT DISTINCT
            bm.id AS "id!: i64",
            bm.channel_id AS "channel_id!: String",
            bm.message_id AS "message_id!: String",
            bm.message_type AS "message_type!: String",
            bm.stay_id,
            bm.previous_message_id
        FROM bot_messages bm
        JOIN bot_message_mentions bmm ON bmm.bot_message_id = bm.id
        WHERE bm.guild_id = ? AND bmm.user_id = ?
        ORDER BY bm.id DESC
        "#,
        gid,
        user_id
    )
    .fetch_all(&db.pool)
    .await?;
    Ok(rows.into_iter().map(BotMessageRow::from).collect())
}

/// Find the most recent embed of a type for a member in a guild (for cross-links).
pub async fn find_latest(
    db: &Db,
    guild_id: GuildId,
    message_type: MessageType,
    member_id: &str,
) -> Result<Option<BotMessageRow>> {
    let gid = guild_id.to_string();
    let mt = message_type.as_str();
    let row = sqlx::query_as!(
        RawBotMessageRow,
        r#"
        SELECT bm.id AS "id!: i64", bm.channel_id AS "channel_id!: String",
               bm.message_id AS "message_id!: String", bm.message_type AS "message_type!: String",
               bm.stay_id, bm.previous_message_id
        FROM bot_messages bm
        JOIN bot_message_mentions bmm ON bmm.bot_message_id = bm.id
        WHERE bm.guild_id = ? AND bm.message_type = ? AND bmm.user_id = ? AND bmm.mention_role = 'member'
        ORDER BY bm.id DESC
        LIMIT 1
        "#,
        gid,
        mt,
        member_id
    )
    .fetch_optional(&db.pool)
    .await?;
    Ok(row.map(BotMessageRow::from))
}

/// Find the most recent departure embed (leave, kick, or ban) for a member.
pub async fn find_latest_departure(
    db: &Db,
    guild_id: GuildId,
    member_id: &str,
) -> Result<Option<BotMessageRow>> {
    let gid = guild_id.to_string();
    let row = sqlx::query_as!(
        RawBotMessageRow,
        r#"
        SELECT bm.id AS "id!: i64", bm.channel_id AS "channel_id!: String",
               bm.message_id AS "message_id!: String", bm.message_type AS "message_type!: String",
               bm.stay_id, bm.previous_message_id
        FROM bot_messages bm
        JOIN bot_message_mentions bmm ON bmm.bot_message_id = bm.id
        WHERE bm.guild_id = ? AND bmm.user_id = ? AND bmm.mention_role = 'member'
          AND bm.message_type IN ('leave', 'kick', 'ban')
        ORDER BY bm.id DESC
        LIMIT 1
        "#,
        gid,
        member_id
    )
    .fetch_optional(&db.pool)
    .await?;
    Ok(row.map(BotMessageRow::from))
}
