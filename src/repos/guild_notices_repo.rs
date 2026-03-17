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

    // ── Guild-level tracking (which guilds received which notices) ───────

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

    // ── Notice definitions (global, created by bot owner) ───────────────

    /// Create a new notice definition.
    pub async fn create_notice(
        &self,
        key: &str,
        title: &str,
        body: &str,
        color: i64,
        current_only: bool,
    ) -> Result<()> {
        let created_at = Timestamp::now().to_rfc2822();
        let current_only_val: i64 = if current_only { 1 } else { 0 };
        sqlx::query(
            "INSERT INTO notices (key, title, body, color, current_only, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(key)
        .bind(title)
        .bind(body)
        .bind(color)
        .bind(current_only_val)
        .bind(created_at)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Get all notice definitions.
    pub async fn get_all_notices(&self) -> Result<Vec<NoticeRow>> {
        let rows = sqlx::query_as::<_, NoticeRow>(
            "SELECT id, key, title, body, color, current_only, created_at FROM notices ORDER BY id",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }

    /// Get a single notice by key.
    pub async fn get_notice_by_key(&self, key: &str) -> Result<Option<NoticeRow>> {
        let row = sqlx::query_as::<_, NoticeRow>(
            "SELECT id, key, title, body, color, current_only, created_at FROM notices WHERE key = ?",
        )
        .bind(key)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row)
    }

    /// Delete a notice definition by key. Guild send records are kept for history.
    pub async fn delete_notice(&self, key: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM notices WHERE key = ?")
            .bind(key)
            .execute(&self.db.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Count how many guilds have received a specific notice.
    pub async fn count_sent(&self, notice_key: &str) -> Result<i64> {
        let row =
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM guild_notices WHERE notice_key = ?")
                .bind(notice_key)
                .fetch_one(&self.db.pool)
                .await?;
        Ok(row.0)
    }
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
