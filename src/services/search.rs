//! Member search, autocomplete, and FTS maintenance.

use anyhow::Result;
use serenity::all::GuildId;
use sqlx::FromRow;

use crate::db::Db;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, FromRow)]
pub struct MemberRow {
    pub user_id: String,
    pub account_username: Option<String>,
    pub display_name: Option<String>,
    pub server_nickname: Option<String>,
    pub first_seen_at: String,
    pub is_present: bool,
}

// ── Search ───────────────────────────────────────────────────────────────────

/// FTS-backed search for autocomplete. Falls back to LIKE if FTS5 is missing.
pub async fn search_prefix(
    db: &Db,
    guild_id: GuildId,
    partial: &str,
    limit: i64,
) -> Result<Vec<MemberRow>> {
    let gid = guild_id.to_string();
    let trimmed = partial.trim();

    if trimmed.is_empty() {
        return recent_members(db, guild_id, limit).await;
    }

    // Sanitize input: keep only alphanumeric, whitespace, underscores.
    // FTS5 operators like *, (, ), :, -, +, ^, ~ would cause malformed MATCH errors.
    let sanitized: String = trimmed
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '_')
        .collect();

    if sanitized.is_empty() {
        return recent_members(db, guild_id, limit).await;
    }

    // Try FTS5 first
    let match_expr = format!(
        "label_norm:{q}* OR account_username:{q}* OR server_nickname:{q}*",
        q = sanitized
    );

    let fts_rows = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT m.user_id, m.account_username, m.display_name, m.server_nickname,
               m.first_seen_at, m.is_present AS "is_present: bool"
        FROM usernames_fts f
        JOIN members m ON m.guild_id = f.guild_id AND m.user_id = f.user_id
        WHERE f.guild_id = ? AND usernames_fts MATCH ?
        ORDER BY bm25(usernames_fts)
        LIMIT ?
        "#,
    )
    .bind(&gid)
    .bind(&match_expr)
    .bind(limit)
    .fetch_all(&db.pool)
    .await;

    match fts_rows {
        Ok(rows) => return Ok(rows),
        Err(e) => {
            let msg = e.to_string();
            if !msg.contains("no such module: fts5") && !msg.contains("malformed MATCH") {
                return Err(e.into());
            }
        }
    }

    // Fallback to LIKE
    let like = format!("%{trimmed}%");
    let rows = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT user_id, account_username, display_name, server_nickname,
               first_seen_at, is_present AS "is_present: bool"
        FROM members
        WHERE guild_id = ?
          AND ((account_username IS NOT NULL AND account_username LIKE ?)
            OR (display_name IS NOT NULL AND display_name LIKE ?)
            OR (server_nickname IS NOT NULL AND server_nickname LIKE ?))
        ORDER BY rowid DESC
        LIMIT ?
        "#,
    )
    .bind(&gid)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(limit)
    .fetch_all(&db.pool)
    .await?;

    Ok(rows)
}

/// Recent members (latest activity first). Used for autocomplete with empty input.
pub async fn recent_members(db: &Db, guild_id: GuildId, limit: i64) -> Result<Vec<MemberRow>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT user_id, account_username, display_name, server_nickname,
               first_seen_at, is_present AS "is_present: bool"
        FROM members
        WHERE guild_id = ?
        ORDER BY rowid DESC
        LIMIT ?
        "#,
    )
    .bind(gid)
    .bind(limit)
    .fetch_all(&db.pool)
    .await?;
    Ok(rows)
}

/// Plain best display name from DB: server_nickname > display_name > account_username.
/// Returns `None` if no member record exists.
pub async fn best_display_name(db: &Db, guild_id: GuildId, user_id: &str) -> Option<String> {
    let gid = guild_id.to_string();
    let row = sqlx::query!(
        r#"
        SELECT account_username, display_name, server_nickname
        FROM members
        WHERE guild_id = ? AND user_id = ?
        "#,
        gid,
        user_id
    )
    .fetch_optional(&db.pool)
    .await
    .ok()
    .flatten()?;

    row.server_nickname
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| row.display_name.as_deref().filter(|s| !s.is_empty()))
        .or(row.account_username.as_deref())
        .map(|s| s.to_string())
}

/// Best reference for a user: `<@id>` if present, `*name*` if departed.
pub async fn best_user_ref(db: &Db, guild_id: GuildId, user_id: &str) -> String {
    let gid = guild_id.to_string();
    let is_present = sqlx::query!(
        r#"SELECT is_present AS "is_present: bool" FROM members WHERE guild_id = ? AND user_id = ?"#,
        gid,
        user_id
    )
    .fetch_optional(&db.pool)
    .await
    .ok()
    .flatten()
    .map(|r| r.is_present)
    .unwrap_or(false);

    if is_present {
        return format!("<@{user_id}>");
    }

    match best_display_name(db, guild_id, user_id).await {
        Some(name) => format!("*{name}*"),
        None => user_id.to_string(),
    }
}

/// Get all user IDs currently present in a guild.
pub async fn present_user_ids(db: &Db, guild_id: GuildId) -> Result<Vec<String>> {
    let gid = guild_id.to_string();
    let rows = sqlx::query!(
        r#"SELECT user_id AS "user_id: String" FROM members WHERE guild_id = ? AND is_present = 1"#,
        gid
    )
    .fetch_all(&db.pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.user_id).collect())
}

// ── FTS maintenance ──────────────────────────────────────────────────────────

/// Rebuild FTS rows for a guild from the members table.
pub async fn rebuild_fts(db: &Db, guild_id: GuildId) -> Result<()> {
    let gid = guild_id.to_string();

    sqlx::query!("DELETE FROM usernames_fts WHERE guild_id = ?", gid)
        .execute(&db.pool)
        .await?;

    sqlx::query!(
        r#"
        INSERT INTO usernames_fts (guild_id, user_id, account_username, server_nickname, label, label_norm)
        SELECT
            guild_id,
            user_id,
            account_username,
            server_nickname,
            COALESCE(NULLIF(server_nickname, ''), NULLIF(display_name, ''), account_username, 'User ' || user_id),
            LOWER(COALESCE(NULLIF(server_nickname, ''), NULLIF(display_name, ''), account_username, user_id))
        FROM members
        WHERE guild_id = ?
        "#,
        gid
    )
    .execute(&db.pool)
    .await?;

    Ok(())
}

/// Upsert a single user into FTS.
pub async fn upsert_fts_row(db: &Db, guild_id: GuildId, user_id: &str) -> Result<()> {
    let gid = guild_id.to_string();

    let row = sqlx::query!(
        r#"
        SELECT account_username, display_name, server_nickname
        FROM members
        WHERE guild_id = ? AND user_id = ?
        "#,
        gid,
        user_id
    )
    .fetch_optional(&db.pool)
    .await?;

    sqlx::query!(
        "DELETE FROM usernames_fts WHERE guild_id = ? AND user_id = ?",
        gid,
        user_id
    )
    .execute(&db.pool)
    .await?;

    if let Some(r) = row {
        let label = r
            .server_nickname
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| r.display_name.as_deref().filter(|s| !s.is_empty()))
            .map(|s| s.to_string())
            .or(r.account_username.clone())
            .unwrap_or_else(|| format!("User {user_id}"));

        let label_norm = label.to_lowercase();

        sqlx::query!(
            r#"
            INSERT INTO usernames_fts (guild_id, user_id, account_username, server_nickname, label, label_norm)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
            gid,
            user_id,
            r.account_username,
            r.server_nickname,
            label,
            label_norm
        )
        .execute(&db.pool)
        .await?;
    }

    Ok(())
}

// ── Counts ───────────────────────────────────────────────────────────────────

/// Count of current members (is_present = 1).
pub async fn count_present(db: &Db, guild_id: GuildId) -> Result<i64> {
    let gid = guild_id.to_string();
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "cnt!: i64" FROM members WHERE guild_id = ? AND is_present = 1"#,
        gid
    )
    .fetch_one(&db.pool)
    .await?;
    Ok(row.cnt)
}

/// Count of all unique members ever seen.
pub async fn count_unique_ever(db: &Db, guild_id: GuildId) -> Result<i64> {
    let gid = guild_id.to_string();
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "cnt!: i64" FROM members WHERE guild_id = ?"#,
        gid
    )
    .fetch_one(&db.pool)
    .await?;
    Ok(row.cnt)
}
