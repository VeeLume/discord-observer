//! Guild configuration and metadata.
//!
//! Merges guild lifecycle (first_seen_at) with admin-configurable settings
//! (log channels, notices toggle).

use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::all::{ChannelId, GuildId, Timestamp};

use crate::db::Db;

const ALLOWED_SETTING_COLUMNS: &[&str] = &[
    "join_log_channel_id",
    "leave_log_channel_id",
    "mod_log_channel_id",
];

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GuildSettings {
    pub join_log: Option<ChannelId>,
    pub leave_log: Option<ChannelId>,
    pub mod_log: Option<ChannelId>,
    pub notices_enabled: bool,
}

impl Default for GuildSettings {
    fn default() -> Self {
        Self {
            join_log: None,
            leave_log: None,
            mod_log: None,
            notices_enabled: true,
        }
    }
}

// ── Guild lifecycle ──────────────────────────────────────────────────────────

/// Ensure a guild row exists, setting first_seen_at if new.
pub async fn ensure_guild(db: &Db, guild_id: GuildId) -> Result<()> {
    let gid = guild_id.to_string();
    let now = Timestamp::now().to_rfc2822();
    sqlx::query!(
        r#"
        INSERT INTO guilds (guild_id, first_seen_at) VALUES (?, ?)
        ON CONFLICT(guild_id) DO NOTHING
        "#,
        gid,
        now
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Get first_seen_at for a guild (for notices current_only check).
pub async fn get_first_seen_at(db: &Db, guild_id: GuildId) -> Result<Option<String>> {
    let gid = guild_id.to_string();
    let row = sqlx::query!(
        r#"SELECT first_seen_at AS "first_seen_at: String" FROM guilds WHERE guild_id = ?"#,
        gid
    )
    .fetch_optional(&db.pool)
    .await?;
    Ok(row.map(|r| r.first_seen_at))
}

// ── Settings ─────────────────────────────────────────────────────────────────

/// Ensure a guild_settings row exists.
pub async fn ensure_settings_row(db: &Db, guild_id: GuildId) -> Result<()> {
    let gid = guild_id.to_string();
    sqlx::query!(
        r#"
        INSERT INTO guild_settings (guild_id) VALUES (?)
        ON CONFLICT(guild_id) DO NOTHING
        "#,
        gid
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Fetch all settings for a guild, returning defaults if no row exists.
pub async fn get_settings(db: &Db, guild_id: GuildId) -> Result<GuildSettings> {
    let guild = guild_id.to_string();
    let rec = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, i64)>(
        "SELECT join_log_channel_id, leave_log_channel_id, mod_log_channel_id, notices_enabled \
         FROM guild_settings WHERE guild_id = ?",
    )
    .bind(&guild)
    .fetch_optional(&db.pool)
    .await?;

    let parse_channel = |val: &Option<String>| {
        val.as_deref()
            .and_then(|s| s.parse::<u64>().ok())
            .map(ChannelId::new)
    };

    match rec {
        Some((join, leave, mlog, notices)) => Ok(GuildSettings {
            join_log: parse_channel(&join),
            leave_log: parse_channel(&leave),
            mod_log: parse_channel(&mlog),
            notices_enabled: notices != 0,
        }),
        None => Ok(GuildSettings::default()),
    }
}

/// Update a log channel setting by column name.
pub async fn set_column(
    db: &Db,
    guild_id: GuildId,
    column: &str,
    value: Option<ChannelId>,
) -> Result<()> {
    anyhow::ensure!(
        ALLOWED_SETTING_COLUMNS.contains(&column),
        "Invalid column: {column}"
    );

    let gid = guild_id.to_string();
    let result = if let Some(id) = value {
        let q = format!("UPDATE guild_settings SET {column} = ? WHERE guild_id = ?");
        sqlx::query(&q)
            .bind(id.get().to_string())
            .bind(&gid)
            .execute(&db.pool)
            .await?
    } else {
        let q = format!("UPDATE guild_settings SET {column} = NULL WHERE guild_id = ?");
        sqlx::query(&q).bind(&gid).execute(&db.pool).await?
    };
    anyhow::ensure!(
        result.rows_affected() > 0,
        "No settings row for guild {gid}"
    );
    Ok(())
}

/// Set notices enabled/disabled.
pub async fn set_notices_enabled(db: &Db, guild_id: GuildId, enabled: bool) -> Result<()> {
    let gid = guild_id.to_string();
    let val = if enabled { 1_i64 } else { 0_i64 };
    let result = sqlx::query!(
        "UPDATE guild_settings SET notices_enabled = ? WHERE guild_id = ?",
        val,
        gid
    )
    .execute(&db.pool)
    .await?;
    anyhow::ensure!(
        result.rows_affected() > 0,
        "No settings row for guild {gid}"
    );
    Ok(())
}

/// Set join-log channel.
pub async fn set_join_log(db: &Db, guild_id: GuildId, channel: Option<ChannelId>) -> Result<()> {
    set_column(db, guild_id, "join_log_channel_id", channel).await
}

/// Set leave-log channel.
pub async fn set_leave_log(db: &Db, guild_id: GuildId, channel: Option<ChannelId>) -> Result<()> {
    set_column(db, guild_id, "leave_log_channel_id", channel).await
}

/// Set mod-log channel.
pub async fn set_mod_log(db: &Db, guild_id: GuildId, channel: Option<ChannelId>) -> Result<()> {
    set_column(db, guild_id, "mod_log_channel_id", channel).await
}

/// Upsert guild settings (join/leave/mod log channels).
pub async fn upsert(
    db: &Db,
    guild_id: GuildId,
    join: Option<ChannelId>,
    leave: Option<ChannelId>,
    log_channel: Option<ChannelId>,
) -> Result<()> {
    let guild_id = guild_id.to_string();
    let join = join.map(|c| c.to_string());
    let leave = leave.map(|c| c.to_string());
    let modu = log_channel.map(|c| c.to_string());

    sqlx::query!(
        r#"
        INSERT INTO guild_settings (guild_id, join_log_channel_id, leave_log_channel_id, mod_log_channel_id)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(guild_id) DO UPDATE SET
          join_log_channel_id = COALESCE(excluded.join_log_channel_id, guild_settings.join_log_channel_id),
          leave_log_channel_id = COALESCE(excluded.leave_log_channel_id, guild_settings.leave_log_channel_id),
          mod_log_channel_id   = COALESCE(excluded.mod_log_channel_id,   guild_settings.mod_log_channel_id)
        "#,
        guild_id,
        join,
        leave,
        modu
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}
