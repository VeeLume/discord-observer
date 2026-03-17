use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::all::{GuildId, Member, Timestamp, UserId};
use sqlx::FromRow;

use crate::db::Db;

#[derive(Clone)]
pub struct MembershipsRepo<'a> {
    db: &'a Db,
}

impl<'a> MembershipsRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    // ---------- writes ----------

    /// Start a membership stay for this user, optionally recording the invite code used.
    pub async fn record_join(
        &self,
        guild_id: GuildId,
        member: &Member,
        invite_code: Option<&str>,
        inviter_name: Option<&str>,
    ) -> Result<()> {
        let guild_id = guild_id.to_string();
        let user_id = member.user.id.to_string();
        let joined_at = Timestamp::now().to_rfc2822();

        let account_username = member.user.name.clone();
        let server_username = member.nick.clone();

        sqlx::query!(
            r#"
            INSERT INTO memberships (
                guild_id, user_id, joined_at, left_at, banned,
                account_username, server_username, invite_code, inviter_name
            )
            VALUES (?, ?, ?, NULL, 0, ?, ?, ?, ?)
            "#,
            guild_id,
            user_id,
            joined_at,
            account_username,
            server_username,
            invite_code,
            inviter_name
        )
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Store the embed message ID for the most recent open stay (join embed).
    pub async fn set_embed_ref(
        &self,
        guild_id: GuildId,
        user_id: UserId,
        channel_id: &str,
        message_id: &str,
    ) -> Result<()> {
        let gid = guild_id.to_string();
        let uid = user_id.to_string();
        sqlx::query(
            "UPDATE memberships SET embed_channel_id = ?, embed_message_id = ? \
             WHERE guild_id = ? AND user_id = ? AND id = (SELECT MAX(id) FROM memberships WHERE guild_id = ? AND user_id = ?)",
        )
        .bind(channel_id)
        .bind(message_id)
        .bind(&gid)
        .bind(&uid)
        .bind(&gid)
        .bind(&uid)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Close the latest open membership stay: set left_at + banned flag.
    pub async fn record_leave(
        &self,
        guild_id: GuildId,
        user_id: UserId,
        banned: bool,
    ) -> Result<()> {
        let guild_id = guild_id.to_string();
        let user_id = user_id.to_string();
        let left_at = Timestamp::now().to_rfc2822();
        let banned_i64 = if banned { 1_i64 } else { 0_i64 };

        sqlx::query!(
            r#"
            UPDATE memberships
               SET left_at = ?, banned = ?
             WHERE guild_id = ? AND user_id = ? AND left_at IS NULL
            "#,
            left_at,
            banned_i64,
            guild_id,
            user_id
        )
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Mark the most recent stay as banned, regardless of whether it's already closed.
    /// This handles the case where GuildMemberRemoval fires before GuildBanAddition.
    pub async fn mark_banned(&self, guild_id: GuildId, user_id: UserId) -> Result<()> {
        let guild_id = guild_id.to_string();
        let user_id = user_id.to_string();
        let left_at = Timestamp::now().to_rfc2822();

        sqlx::query!(
            r#"
            UPDATE memberships
               SET banned = 1,
                   left_at = COALESCE(left_at, ?)
             WHERE guild_id = ? AND user_id = ?
               AND id = (
                   SELECT MAX(id) FROM memberships
                   WHERE guild_id = ? AND user_id = ?
               )
            "#,
            left_at,
            guild_id,
            user_id,
            guild_id,
            user_id
        )
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    // ---------- reads ----------

    pub async fn history_for_user(
        &self,
        guild_id: GuildId,
        user_id: UserId,
    ) -> Result<Vec<MembershipRow>> {
        let guild_id = guild_id.to_string();
        let user_id = user_id.to_string();
        let rows = sqlx::query_as!(
            MembershipRow,
            r#"
            SELECT joined_at,
                   left_at,
                   banned        AS "banned: bool",
                   account_username,
                   server_username,
                   invite_code,
                   inviter_name,
                   embed_channel_id,
                   embed_message_id
            FROM memberships
            WHERE guild_id = ? AND user_id = ?
            ORDER BY id ASC
            "#,
            guild_id,
            user_id
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }

    /// Last row per user for this guild, with last-known names.
    pub async fn recent_user_summaries(
        &self,
        guild_id: GuildId,
        limit: i64,
    ) -> Result<Vec<UserSummary>> {
        let rows = sqlx::query_as::<_, UserSummary>(
            r#"
            WITH last AS (
              SELECT user_id, MAX(id) AS last_row_id
              FROM memberships
              WHERE guild_id = ?
              GROUP BY user_id
            )
            SELECT
              m.user_id          AS user_id,
              l.last_row_id      AS last_row_id,
              m.account_username AS account_username,
              m.server_username  AS server_username
            FROM last l
            JOIN memberships m
              ON m.id = l.last_row_id
            ORDER BY l.last_row_id DESC
            LIMIT ?
            "#,
        )
        .bind(guild_id.to_string())
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }

    /// Paged “recent user summaries”.
    /// Pass `after_last_row_id` to continue where the previous page ended (strictly older).
    pub async fn recent_user_summaries_page(
        &self,
        guild_id: serenity::all::GuildId,
        limit: i64,
        after_last_row_id: Option<i64>,
    ) -> Result<Vec<UserSummary>> {
        // We page by the synthetic "last_row_id" (MAX(id) per user). We want strictly older rows.
        let mut q = String::from(
            r#"
        WITH last AS (
          SELECT user_id, MAX(id) AS last_row_id
          FROM memberships
          WHERE guild_id = ?
          GROUP BY user_id
        )
        SELECT
          m.user_id          AS user_id,
          l.last_row_id      AS last_row_id,
          m.account_username AS account_username,
          m.server_username  AS server_username
        FROM last l
        JOIN memberships m
          ON m.id = l.last_row_id
        "#,
        );

        if after_last_row_id.is_some() {
            q.push_str(" WHERE l.last_row_id < ? ");
        }

        q.push_str(" ORDER BY l.last_row_id DESC LIMIT ? ");

        let mut query = sqlx::query_as::<_, UserSummary>(&q).bind(guild_id.to_string());

        if let Some(cursor) = after_last_row_id {
            query = query.bind(cursor);
        }

        query = query.bind(limit);

        let rows = query.fetch_all(&self.db.pool).await?;
        Ok(rows)
    }

    /// Search by last-known account/server name.
    pub async fn search_user_summaries(
        &self,
        guild_id: GuildId,
        like: &str,
        limit: i64,
    ) -> Result<Vec<UserSummary>> {
        let rows = sqlx::query_as::<_, UserSummary>(
            r#"
            WITH last AS (
              SELECT user_id, MAX(id) AS last_row_id
              FROM memberships
              WHERE guild_id = ?
              GROUP BY user_id
            )
            SELECT
              m.user_id          AS user_id,
              l.last_row_id      AS last_row_id,
              m.account_username AS account_username,
              m.server_username  AS server_username
            FROM last l
            JOIN memberships m
              ON m.id = l.last_row_id
            WHERE (m.account_username IS NOT NULL AND m.account_username LIKE ?)
               OR (m.server_username  IS NOT NULL AND m.server_username  LIKE ?)
            ORDER BY l.last_row_id DESC
            LIMIT ?
            "#,
        )
        .bind(guild_id.to_string())
        .bind(like)
        .bind(like)
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }

    /// Users with >= min_stays stays (i.e., joined multiple times).
    pub async fn rejoiners(
        &self,
        guild_id: serenity::all::GuildId,
        min_rejoins: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<RejoinerRow>> {
        let guild_id = guild_id.to_string();
        let rows = sqlx::query!(
            r#"
        WITH last AS (
          SELECT user_id, MAX(id) AS last_row_id
          FROM memberships
          WHERE guild_id = ?
          GROUP BY user_id
        ),
        agg AS (
          SELECT user_id,
                 COUNT(*) AS stays,
                 SUM(CASE WHEN left_at IS NOT NULL THEN 1 ELSE 0 END) AS times_left
          FROM memberships
          WHERE guild_id = ?
          GROUP BY user_id
        )
        SELECT a.user_id                         AS "user_id: String",
               a.stays                          AS "stay_count: i64",
               a.times_left                      AS "times_left: i64",
               m.account_username                AS "account_username: Option<String>",
               m.server_username                 AS "server_username: Option<String>"
        FROM agg a
        JOIN last l ON l.user_id = a.user_id
        JOIN memberships m ON m.id = l.last_row_id
        WHERE a.stays >= ?
        ORDER BY a.stays DESC, l.last_row_id DESC
        LIMIT ?
        "#,
            guild_id,
            guild_id,
            min_rejoins,
            limit
        )
        .fetch_all(&self.db.pool)
        .await?;

        let out = rows
            .into_iter()
            .map(|r| RejoinerRow {
                user_id: r.user_id.expect("User id cant be NULL"),
                rejoin_count: r.stay_count.unwrap_or(0),
                times_left: r.times_left.unwrap_or(0),
                account_username: r.account_username.flatten(),
                server_username: r.server_username.flatten(),
            })
            .collect();

        Ok(out)
    }

    /// Fetch exits (left_at IS NOT NULL) and let caller filter by time window.
    pub async fn all_exits(
        &self,
        guild_id: serenity::all::GuildId,
        limit: i64, // cap for safety; set high if you want "all"
    ) -> anyhow::Result<Vec<ExitRow>> {
        let guild_id = guild_id.to_string();
        let rows = sqlx::query!(
            r#"
        WITH last AS (
          SELECT user_id, MAX(id) AS last_row_id
          FROM memberships
          WHERE guild_id = ?
          GROUP BY user_id
        )
        SELECT m.user_id                      AS "user_id: String",
               m.left_at                      AS "left_at: String",
               m.banned                       AS "banned: bool",
               n.account_username             AS "account_username: Option<String>",
               n.server_username              AS "server_username: Option<String>"
        FROM memberships m
        JOIN last l ON l.user_id = m.user_id
        JOIN memberships n ON n.id = l.last_row_id
        WHERE m.guild_id = ?
          AND m.left_at IS NOT NULL
        ORDER BY m.id DESC
        LIMIT ?
        "#,
            guild_id,
            guild_id,
            limit
        )
        .fetch_all(&self.db.pool)
        .await?;

        let out = rows
            .into_iter()
            .map(|r| ExitRow {
                user_id: r.user_id,
                left_at: r.left_at.expect("left_at is NOT NULL"),
                banned: r.banned,
                account_username: r.account_username.flatten(),
                server_username: r.server_username.flatten(),
            })
            .collect();

        Ok(out)
    }

    /// Current point-in-time + lifetime counters.
    pub async fn stats_current(
        &self,
        guild_id: serenity::all::GuildId,
    ) -> anyhow::Result<StatsCurrent> {
        let gid = guild_id.to_string();

        // DISTINCT counts must be separate queries for clarity/perf on SQLite.
        let current_members = sqlx::query!(
            r#"
        SELECT COUNT(DISTINCT user_id) AS "cnt!: i64"
        FROM memberships
        WHERE guild_id = ? AND left_at IS NULL
        "#,
            gid
        )
        .fetch_one(&self.db.pool)
        .await?
        .cnt;

        let unique_ever = sqlx::query!(
            r#"
        SELECT COUNT(DISTINCT user_id) AS "cnt!: i64"
        FROM memberships
        WHERE guild_id = ?
        "#,
            gid
        )
        .fetch_one(&self.db.pool)
        .await?
        .cnt;

        let total_rejoins = sqlx::query!(
            r#"
        SELECT COUNT(*) AS "cnt!: i64"
        FROM memberships
        WHERE guild_id = ?
        "#,
            gid
        )
        .fetch_one(&self.db.pool)
        .await?
        .cnt;

        let total_exits = sqlx::query!(
            r#"
        SELECT COUNT(*) AS "cnt!: i64"
        FROM memberships
        WHERE guild_id = ? AND left_at IS NOT NULL
        "#,
            gid
        )
        .fetch_one(&self.db.pool)
        .await?
        .cnt;

        let total_banned = sqlx::query!(
            r#"
        SELECT COUNT(*) AS "cnt!: i64"
        FROM memberships
        WHERE guild_id = ? AND left_at IS NOT NULL AND banned = 1
        "#,
            gid
        )
        .fetch_one(&self.db.pool)
        .await?
        .cnt;

        Ok(StatsCurrent {
            current_members,
            unique_ever,
            total_rejoins,
            total_exits,
            total_banned,
        })
    }

    /// Load a capped set of join timestamps for a trend window (filtered in Rust).
    /// For simplicity, pull up to `cap` rows newest-first.
    pub async fn recent_joins_raw(
        &self,
        guild_id: serenity::all::GuildId,
        cap: i64,
    ) -> anyhow::Result<Vec<String>> {
        let gid = guild_id.to_string();
        let rows = sqlx::query!(
            r#"
        SELECT joined_at AS "joined_at: String"
        FROM memberships
        WHERE guild_id = ?
        ORDER BY id DESC
        LIMIT ?
        "#,
            gid,
            cap
        )
        .fetch_all(&self.db.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.joined_at).collect())
    }

    /// Load a capped set of (joined_at,left_at,banned) timestamps for trend deltas.
    pub async fn recent_rejoins_raw(
        &self,
        guild_id: serenity::all::GuildId,
        cap: i64,
    ) -> anyhow::Result<Vec<RejoinTimes>> {
        let gid = guild_id.to_string();
        let rows = sqlx::query!(
            r#"
        SELECT user_id                AS "user_id: String",
               joined_at              AS "joined_at: String",
               left_at                AS "left_at: Option<String>",
               banned                 AS "banned: bool"
        FROM memberships
        WHERE guild_id = ?
        ORDER BY id DESC
        LIMIT ?
        "#,
            gid,
            cap
        )
        .fetch_all(&self.db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| RejoinTimes {
                user_id: r.user_id,
                joined_at: r.joined_at,
                left_at: r.left_at.flatten(),
                banned: r.banned,
            })
            .collect())
    }

    /// Rebuild FTS rows for a guild from the latest membership row per user.
    pub async fn rebuild_usernames_fts_for_guild(
        &self,
        guild_id: serenity::all::GuildId,
    ) -> anyhow::Result<()> {
        let gid = guild_id.to_string();

        // Wipe existing rows for this guild
        sqlx::query!("DELETE FROM usernames_fts WHERE guild_id = ?", gid)
            .execute(&self.db.pool)
            .await?;

        // Insert one row per user (latest stay) into FTS
        // label + label_norm help both display-like and strict prefix matching.
        sqlx::query!(
        r#"
        WITH last AS (
          SELECT user_id, MAX(id) AS last_row_id
          FROM memberships
          WHERE guild_id = ?
          GROUP BY user_id
        )
        INSERT INTO usernames_fts (guild_id, user_id, account_username, server_username, label, label_norm)
        SELECT
          ?                               AS guild_id,
          m.user_id                       AS user_id,
          m.account_username              AS account_username,
          m.server_username               AS server_username,
          COALESCE(NULLIF(m.server_username, ''), m.account_username, 'User ' || m.user_id) AS label,
          LOWER(COALESCE(NULLIF(m.server_username, ''), m.account_username, m.user_id))      AS label_norm
        FROM last l
        JOIN memberships m ON m.id = l.last_row_id
        "#,
        gid, gid
    )
    .execute(&self.db.pool)
    .await?;

        Ok(())
    }

    /// Upsert a single user into FTS (call on join or when you refresh names).
    pub async fn upsert_usernames_fts_row(
        &self,
        guild_id: serenity::all::GuildId,
        user_id: &str,
    ) -> anyhow::Result<()> {
        let gid = guild_id.to_string();
        let uid = user_id.to_string();

        // Grab the latest membership row to get last-known names.
        let row = sqlx::query!(
            r#"
        SELECT m.user_id, m.account_username, m.server_username
        FROM memberships m
        WHERE m.guild_id = ? AND m.user_id = ?
        ORDER BY m.id DESC
        LIMIT 1
        "#,
            gid,
            uid
        )
        .fetch_optional(&self.db.pool)
        .await?;

        // Remove old FTS row (if any)
        sqlx::query!(
            "DELETE FROM usernames_fts WHERE guild_id = ? AND user_id = ?",
            gid,
            uid
        )
        .execute(&self.db.pool)
        .await?;

        if let Some(r) = row {
            let label = r
                .server_username
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .or(r.account_username.clone())
                .unwrap_or_else(|| format!("User {}", r.user_id));

            let label_norm = label.to_lowercase();

            sqlx::query!(
            r#"
            INSERT INTO usernames_fts (guild_id, user_id, account_username, server_username, label, label_norm)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
            gid,
            uid,
            r.account_username,
            r.server_username,
            label,
            label_norm
        )
        .execute(&self.db.pool)
        .await?;
        }

        Ok(())
    }

    /// FTS-backed search for autocomplete. Falls back to LIKE if FTS is missing.
    pub async fn search_user_summaries_prefix(
        &self,
        guild_id: serenity::all::GuildId,
        partial: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<UserSummary>> {
        let gid = guild_id.to_string();
        let trimmed = partial.trim();

        // If user hasn't typed, just reuse your recent list.
        if trimmed.is_empty() {
            return self.recent_user_summaries(guild_id, limit).await;
        }

        // Try FTS5 first.
        // Build a MATCH query that hits normalized label and raw fields with prefix.
        // Example: label_norm:par* OR account_username:par* OR server_username:par*
        let match_expr = format!(
            "label_norm:{q}* OR account_username:{q}* OR server_username:{q}*",
            q = trimmed.to_lowercase().replace('"', "") // simplistic sanitize
        );

        // We select through the "last" CTE to return consistent UserSummary (latest names).
        let fts_rows = sqlx::query_as::<_, UserSummary>(
            r#"
        WITH last AS (
          SELECT user_id, MAX(id) AS last_row_id
          FROM memberships
          WHERE guild_id = ?
          GROUP BY user_id
        ),
        hits AS (
          SELECT user_id, bm25(usernames_fts) AS rank
          FROM usernames_fts
          WHERE guild_id = ?
            AND usernames_fts MATCH ?
        )
        SELECT
          m.user_id          AS user_id,
          l.last_row_id      AS last_row_id,
          m.account_username AS account_username,
          m.server_username  AS server_username
        FROM hits h
        JOIN last l ON l.user_id = h.user_id
        JOIN memberships m ON m.id = l.last_row_id
        ORDER BY h.rank, l.last_row_id DESC
        LIMIT ?
        "#,
        )
        .bind(&gid) // last CTE
        .bind(&gid) // hits filter
        .bind(&match_expr) // MATCH string
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await;

        match fts_rows {
            Ok(rows) => return Ok(rows),
            Err(e) => {
                // If FTS is unavailable (e.g., "no such module: fts5") or MATCH failed,
                // gracefully fall back to LIKE-based search.
                let msg = e.to_string();
                let is_fts_missing =
                    msg.contains("no such module: fts5") || msg.contains("malformed MATCH");
                if !is_fts_missing {
                    // unknown SQL error: propagate
                    return Err(e.into());
                }
            }
        }

        // Fallback to your known-good LIKE search:
        let like = format!("%{}%", trimmed);
        let rows = sqlx::query_as::<_, UserSummary>(
            r#"
        WITH last AS (
          SELECT user_id, MAX(id) AS last_row_id
          FROM memberships
          WHERE guild_id = ?
          GROUP BY user_id
        )
        SELECT
          m.user_id          AS user_id,
          l.last_row_id      AS last_row_id,
          m.account_username AS account_username,
          m.server_username  AS server_username
        FROM last l
        JOIN memberships m
          ON m.id = l.last_row_id
        WHERE (m.account_username IS NOT NULL AND m.account_username LIKE ?)
           OR (m.server_username  IS NOT NULL AND m.server_username  LIKE ?)
        ORDER BY l.last_row_id DESC
        LIMIT ?
        "#,
        )
        .bind(&gid)
        .bind(&like)
        .bind(&like)
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await?;

        Ok(rows)
    }
}

// ---------- row types ----------

#[derive(Debug, Clone)]
pub struct MembershipRow {
    pub joined_at: String,
    pub left_at: Option<String>,
    pub banned: bool,
    pub account_username: Option<String>,
    pub server_username: Option<String>,
    pub invite_code: Option<String>,
    pub inviter_name: Option<String>,
    pub embed_channel_id: Option<String>,
    pub embed_message_id: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct UserSummary {
    pub user_id: String,
    pub last_row_id: i64,
    pub account_username: Option<String>,
    pub server_username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RejoinerRow {
    pub user_id: String,
    pub rejoin_count: i64,
    pub times_left: i64,
    pub account_username: Option<String>,
    pub server_username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExitRow {
    pub user_id: String,
    pub left_at: String, // RFC2822
    pub banned: bool,
    pub account_username: Option<String>,
    pub server_username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StatsCurrent {
    pub current_members: i64, // DISTINCT user_id with left_at IS NULL
    pub unique_ever: i64,     // DISTINCT user_id seen ever
    pub total_rejoins: i64,   // total server stays recorded (rows in memberships)
    pub total_exits: i64,     // rows with left_at NOT NULL
    pub total_banned: i64,    // rows with left_at NOT NULL AND banned=1
}

#[derive(Debug, Clone)]
pub struct RejoinTimes {
    pub user_id: String,
    pub joined_at: String,       // RFC2822
    pub left_at: Option<String>, // RFC2822
    pub banned: bool,
}
