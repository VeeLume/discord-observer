use anyhow::Result;
use serenity::all::{GuildId, Timestamp};

use crate::db::Db;

#[derive(Clone)]
pub struct GuildNoticesRepo<'a> {
    db: &'a Db,
}

impl<'a> GuildNoticesRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// Check whether a notice has already been sent to this guild.
    pub async fn has_been_sent(&self, guild_id: GuildId, notice_key: &str) -> Result<bool> {
        let gid = guild_id.to_string();
        let row = sqlx::query!(
            r#"SELECT 1 AS "exists!: bool" FROM guild_notices WHERE guild_id = ? AND notice_key = ?"#,
            gid,
            notice_key
        )
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Record that a notice has been sent to this guild.
    pub async fn mark_sent(&self, guild_id: GuildId, notice_key: &str) -> Result<()> {
        let gid = guild_id.to_string();
        let sent_at = Timestamp::now().to_rfc2822();
        sqlx::query!(
            r#"INSERT OR IGNORE INTO guild_notices (guild_id, notice_key, sent_at) VALUES (?, ?, ?)"#,
            gid,
            notice_key,
            sent_at
        )
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Get all sent notices for a guild, most recent first.
    pub async fn get_history(&self, guild_id: GuildId) -> Result<Vec<SentNotice>> {
        let gid = guild_id.to_string();
        let rows = sqlx::query_as::<_, SentNotice>(
            "SELECT notice_key, sent_at FROM guild_notices WHERE guild_id = ? ORDER BY rowid DESC",
        )
        .bind(gid)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SentNotice {
    pub notice_key: String,
    pub sent_at: String,
}
