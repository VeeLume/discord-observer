//! Read-only aggregations: history, rejoiners, exits, balance, counts.

use anyhow::Result;
use serenity::all::{GuildId, UserId};

use crate::db::Db;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepartureType {
    Leave,
    Kick,
    Ban,
}

impl DepartureType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Leave => "leave",
            Self::Kick => "kick",
            Self::Ban => "ban",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "kick" => Self::Kick,
            "ban" => Self::Ban,
            _ => Self::Leave,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Leave => "left",
            Self::Kick => "kicked",
            Self::Ban => "banned",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StayRow {
    pub id: i64,
    pub joined_at: String,
    pub left_at: Option<String>,
    pub departure_type: Option<DepartureType>,
    pub invite_code: Option<String>,
    pub inviter_id: Option<String>,
    pub moderator_id: Option<String>,
    pub member_flags: i64,
}

#[derive(Debug, Clone)]
pub struct RejoinerRow {
    pub user_id: String,
    pub stay_count: i64,
    pub times_left: i64,
    pub account_username: Option<String>,
    pub display_name: Option<String>,
    pub server_nickname: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExitRow {
    pub user_id: String,
    pub left_at: String,
    pub departure_type: Option<DepartureType>,
    pub account_username: Option<String>,
    pub display_name: Option<String>,
    pub server_nickname: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StatsCurrent {
    pub current_members: i64,
    pub unique_ever: i64,
    pub total_stays: i64,
    pub total_exits: i64,
    pub total_banned: i64,
}

#[derive(Debug, Clone)]
pub struct RejoinTimes {
    pub user_id: String,
    pub joined_at: String,
    pub left_at: Option<String>,
    pub departure_type: Option<DepartureType>,
}

// ── History ──────────────────────────────────────────────────────────────────

/// Full stay history for a user.
pub async fn history_for_user(db: &Db, guild_id: GuildId, user_id: UserId) -> Result<Vec<StayRow>> {
    let gid = guild_id.to_string();
    let uid = user_id.to_string();
    let rows = sqlx::query!(
        r#"
        SELECT id AS "id!: i64", joined_at, left_at, departure_type, invite_code, inviter_id, moderator_id, member_flags
        FROM stays
        WHERE guild_id = ? AND user_id = ?
        ORDER BY id ASC
        "#,
        gid,
        uid
    )
    .fetch_all(&db.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| StayRow {
            id: r.id,
            joined_at: r.joined_at,
            left_at: r.left_at,
            departure_type: r.departure_type.as_deref().map(DepartureType::from_db),
            invite_code: r.invite_code,
            inviter_id: r.inviter_id,
            moderator_id: r.moderator_id,
            member_flags: r.member_flags,
        })
        .collect())
}

// ── Aggregations ─────────────────────────────────────────────────────────────

/// Users with >= min_stays stays.
pub async fn rejoiners(
    db: &Db,
    guild_id: GuildId,
    min_stays: i64,
    limit: i64,
) -> Result<Vec<RejoinerRow>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query!(
        r#"
        SELECT
            s.user_id                AS "user_id: String",
            COUNT(*)                 AS "stay_count: i64",
            SUM(CASE WHEN s.left_at IS NOT NULL THEN 1 ELSE 0 END) AS "times_left: i64",
            m.account_username       AS "account_username: Option<String>",
            m.display_name           AS "display_name: Option<String>",
            m.server_nickname        AS "server_nickname: Option<String>"
        FROM stays s
        JOIN members m ON m.guild_id = s.guild_id AND m.user_id = s.user_id
        WHERE s.guild_id = ?
        GROUP BY s.user_id
        HAVING COUNT(*) >= ?
        ORDER BY COUNT(*) DESC
        LIMIT ?
        "#,
        gid,
        min_stays,
        limit
    )
    .fetch_all(&db.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RejoinerRow {
            user_id: r.user_id,
            stay_count: r.stay_count,
            times_left: r.times_left,
            account_username: r.account_username.flatten(),
            display_name: r.display_name.flatten(),
            server_nickname: r.server_nickname.flatten(),
        })
        .collect())
}

/// Recent exits (left_at IS NOT NULL), optionally filtered to exits after `since`.
pub async fn all_exits(db: &Db, guild_id: GuildId, since: Option<&str>) -> Result<Vec<ExitRow>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query!(
        r#"
        SELECT
            s.user_id         AS "user_id!: String",
            s.left_at         AS "left_at!: String",
            s.departure_type,
            m.account_username,
            m.display_name,
            m.server_nickname
        FROM stays s
        JOIN members m ON m.guild_id = s.guild_id AND m.user_id = s.user_id
        WHERE s.guild_id = ? AND s.left_at IS NOT NULL
          AND (? IS NULL OR s.left_at >= ?)
        ORDER BY s.id DESC
        "#,
        gid,
        since,
        since,
    )
    .fetch_all(&db.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ExitRow {
            user_id: r.user_id,
            left_at: r.left_at,
            departure_type: r.departure_type.as_deref().map(DepartureType::from_db),
            account_username: r.account_username,
            display_name: r.display_name,
            server_nickname: r.server_nickname,
        })
        .collect())
}

/// Aggregate stats (takes pre-computed member counts from search module).
pub async fn stats_current(
    db: &Db,
    guild_id: GuildId,
    current_members: i64,
    unique_ever: i64,
) -> Result<StatsCurrent> {
    let gid = guild_id.to_string();

    let total_stays = sqlx::query!(
        r#"SELECT COUNT(*) AS "cnt!: i64" FROM stays WHERE guild_id = ?"#,
        gid
    )
    .fetch_one(&db.pool)
    .await?
    .cnt;

    let total_exits = sqlx::query!(
        r#"SELECT COUNT(*) AS "cnt!: i64" FROM stays WHERE guild_id = ? AND left_at IS NOT NULL"#,
        gid
    )
    .fetch_one(&db.pool)
    .await?
    .cnt;

    let total_banned = sqlx::query!(
        r#"SELECT COUNT(*) AS "cnt!: i64" FROM stays WHERE guild_id = ? AND departure_type = 'ban'"#,
        gid
    )
    .fetch_one(&db.pool)
    .await?
    .cnt;

    Ok(StatsCurrent {
        current_members,
        unique_ever,
        total_stays,
        total_exits,
        total_banned,
    })
}

/// Raw join timestamps for trend analysis.
pub async fn recent_joins_raw(db: &Db, guild_id: GuildId, cap: i64) -> Result<Vec<String>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query!(
        r#"
        SELECT joined_at AS "joined_at: String"
        FROM stays
        WHERE guild_id = ?
        ORDER BY id DESC
        LIMIT ?
        "#,
        gid,
        cap
    )
    .fetch_all(&db.pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.joined_at).collect())
}

/// Raw join/leave/departure data for trend deltas.
pub async fn recent_rejoins_raw(db: &Db, guild_id: GuildId, cap: i64) -> Result<Vec<RejoinTimes>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query!(
        r#"
        SELECT
            user_id        AS "user_id: String",
            joined_at      AS "joined_at: String",
            left_at        AS "left_at: Option<String>",
            departure_type AS "departure_type: Option<String>"
        FROM stays
        WHERE guild_id = ?
        ORDER BY id DESC
        LIMIT ?
        "#,
        gid,
        cap
    )
    .fetch_all(&db.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RejoinTimes {
            user_id: r.user_id,
            joined_at: r.joined_at,
            left_at: r.left_at.flatten(),
            departure_type: r
                .departure_type
                .flatten()
                .as_deref()
                .map(DepartureType::from_db),
        })
        .collect())
}
