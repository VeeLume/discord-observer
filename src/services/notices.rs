//! Notice definitions, sending, and tracking.
//!
//! Merges guild_notices_repo + notices.rs into one service.

use anyhow::Result;
use serenity::all::{ChannelId, CreateEmbed, CreateMessage, GuildId, Timestamp};
use serenity::http::Http;

use crate::db::Db;
use crate::services::settings::GuildSettings;

// ── Types ────────────────────────────────────────────────────────────────────

/// Context for sending notices to a guild.
pub struct NoticeContext {
    pub guild_id: GuildId,
    pub settings: GuildSettings,
    pub first_seen_at: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SentNotice {
    pub notice_key: String,
    pub sent_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NoticeRow {
    pub id: i64,
    pub key: String,
    pub title: String,
    pub body: String,
    pub color: i64,
    pub current_only: i64,
    pub created_at: String,
}

// ── Send logic ───────────────────────────────────────────────────────────────

/// Send any pending notices for a guild. Returns the number actually sent.
pub async fn send_pending_notices(http: &Http, db: &Db, ctx: &NoticeContext) -> Result<usize> {
    if !ctx.settings.notices_enabled {
        return Ok(0);
    }

    let target = ctx
        .settings
        .mod_log
        .or(ctx.settings.join_log)
        .or(ctx.settings.leave_log);

    let channel = match target {
        Some(ch) => ch,
        None => return Ok(0),
    };

    let notices = get_all_notices(db).await?;
    let mut sent = 0usize;

    for notice in &notices {
        // current_only: only send to guilds that existed when the notice was created.
        // Skip if first_seen_at is unknown (guild too new to have a record).
        if notice.current_only != 0 {
            match &ctx.first_seen_at {
                Some(first_seen) if notice.created_at >= *first_seen => {}
                _ => continue,
            }
        }

        if has_been_sent(db, ctx.guild_id, &notice.key).await? {
            continue;
        }

        let embed = CreateEmbed::new()
            .color(notice.color as u32)
            .title(&notice.title)
            .description(&notice.body);

        if send_to_channel(http, channel, embed).await {
            mark_sent(db, ctx.guild_id, &notice.key).await?;
            sent += 1;
        }
    }

    Ok(sent)
}

async fn send_to_channel(http: &Http, channel: ChannelId, embed: CreateEmbed) -> bool {
    channel
        .send_message(http, CreateMessage::new().embed(embed))
        .await
        .is_ok()
}

// ── Guild-level tracking ─────────────────────────────────────────────────────

/// Check whether a notice has already been sent to this guild.
pub async fn has_been_sent(db: &Db, guild_id: GuildId, notice_key: &str) -> Result<bool> {
    let gid = guild_id.to_string();
    let row = sqlx::query!(
        r#"SELECT 1 AS "exists!: bool" FROM guild_notices WHERE guild_id = ? AND notice_key = ?"#,
        gid,
        notice_key
    )
    .fetch_optional(&db.pool)
    .await?;
    Ok(row.is_some())
}

/// Record that a notice has been sent to this guild.
pub async fn mark_sent(db: &Db, guild_id: GuildId, notice_key: &str) -> Result<()> {
    let gid = guild_id.to_string();
    let sent_at = Timestamp::now().to_rfc2822();
    sqlx::query!(
        r#"INSERT OR IGNORE INTO guild_notices (guild_id, notice_key, sent_at) VALUES (?, ?, ?)"#,
        gid,
        notice_key,
        sent_at
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Get all sent notices for a guild, most recent first.
pub async fn get_history(db: &Db, guild_id: GuildId) -> Result<Vec<SentNotice>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query_as!(
        SentNotice,
        "SELECT notice_key, sent_at FROM guild_notices WHERE guild_id = ? ORDER BY rowid DESC",
        gid
    )
    .fetch_all(&db.pool)
    .await?;
    Ok(rows)
}

// ── Notice definitions ───────────────────────────────────────────────────────

/// Create a new notice definition.
pub async fn create_notice(
    db: &Db,
    key: &str,
    title: &str,
    body: &str,
    color: i64,
    current_only: bool,
) -> Result<()> {
    let created_at = Timestamp::now().to_rfc2822();
    let current_only_val: i64 = if current_only { 1 } else { 0 };
    sqlx::query!(
        "INSERT INTO notices (key, title, body, color, current_only, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        key, title, body, color, current_only_val, created_at
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Get all notice definitions.
pub async fn get_all_notices(db: &Db) -> Result<Vec<NoticeRow>> {
    let rows = sqlx::query_as!(
        NoticeRow,
        r#"SELECT id AS "id!: i64", key, title, body, color, current_only, created_at FROM notices ORDER BY id"#
    )
    .fetch_all(&db.pool)
    .await?;
    Ok(rows)
}

/// Get a single notice by key.
pub async fn get_notice_by_key(db: &Db, key: &str) -> Result<Option<NoticeRow>> {
    let row = sqlx::query_as!(
        NoticeRow,
        r#"SELECT id AS "id!: i64", key, title, body, color, current_only, created_at FROM notices WHERE key = ?"#,
        key
    )
    .fetch_optional(&db.pool)
    .await?;
    Ok(row)
}

/// Delete a notice definition by key.
pub async fn delete_notice(db: &Db, key: &str) -> Result<bool> {
    let result = sqlx::query!("DELETE FROM notices WHERE key = ?", key)
        .execute(&db.pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Count how many guilds have received a specific notice.
pub async fn count_sent(db: &Db, notice_key: &str) -> Result<i64> {
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "cnt!: i64" FROM guild_notices WHERE notice_key = ?"#,
        notice_key
    )
    .fetch_one(&db.pool)
    .await?;
    Ok(row.cnt)
}
