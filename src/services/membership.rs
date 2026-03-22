//! Transactional member lifecycle writes.
//!
//! Each public function wraps coupled DB writes in a transaction.
//! When a member joins, leaves, or changes names, multiple tables must
//! be updated atomically (members + stays + name_changes + FTS).

use anyhow::Result;
use serenity::all::{GuildId, Member, Timestamp, UserId};

use crate::db::Db;
use crate::services::stats::DepartureType;

// ── Join ─────────────────────────────────────────────────────────────────────

/// Record a member join: upsert member + create stay + update FTS.
/// Returns the new stay ID.
pub async fn record_join(
    db: &Db,
    guild_id: GuildId,
    member: &Member,
    invite_code: Option<&str>,
    inviter_id: Option<&str>,
) -> Result<i64> {
    let mut tx = db.pool.begin().await?;

    let gid = guild_id.to_string();
    let uid = member.user.id.to_string();
    let now = Timestamp::now().to_rfc2822();
    let account_username = member.user.name.clone();
    let display_name = member.user.global_name.clone();
    let server_nickname = member.nick.clone();
    let flags_bits = member.flags.bits() as i64;

    // 1. Upsert member (create or update names + set present)
    sqlx::query!(
        r#"
        INSERT INTO members (guild_id, user_id, account_username, display_name, server_nickname, first_seen_at, is_present)
        VALUES (?, ?, ?, ?, ?, ?, 1)
        ON CONFLICT(guild_id, user_id) DO UPDATE SET
            account_username = excluded.account_username,
            display_name     = excluded.display_name,
            server_nickname  = excluded.server_nickname,
            is_present       = 1
        "#,
        gid, uid, account_username, display_name, server_nickname, now
    )
    .execute(&mut *tx)
    .await?;

    // 2. Create stay
    let result = sqlx::query!(
        r#"
        INSERT INTO stays (guild_id, user_id, joined_at, invite_code, inviter_id, member_flags)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
        gid,
        uid,
        now,
        invite_code,
        inviter_id,
        flags_bits
    )
    .execute(&mut *tx)
    .await?;
    let stay_id = result.last_insert_rowid();

    // 3. Update FTS
    upsert_fts_tx(&mut tx, &gid, &uid).await?;

    tx.commit().await?;
    Ok(stay_id)
}

// ── Leave ────────────────────────────────────────────────────────────────────

/// Record a member departure immediately: close stay + mark not present.
/// Does NOT set departure_type or moderator_id — those are filled in later
/// by `enrich_departure` after the audit log check.
pub async fn record_leave_immediate(db: &Db, guild_id: GuildId, user_id: UserId) -> Result<()> {
    let mut tx = db.pool.begin().await?;

    let gid = guild_id.to_string();
    let uid = user_id.to_string();
    let left_at = Timestamp::now().to_rfc2822();

    // 1. Close the open stay (may affect 0 rows if mark_banned already closed it)
    sqlx::query!(
        "UPDATE stays SET left_at = ? WHERE guild_id = ? AND user_id = ? AND left_at IS NULL",
        left_at,
        gid,
        uid
    )
    .execute(&mut *tx)
    .await?;

    // 2. Always mark member as not present
    sqlx::query!(
        "UPDATE members SET is_present = 0 WHERE guild_id = ? AND user_id = ?",
        gid,
        uid
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Enrich the most recent stay with departure reason and moderator info.
/// Called after a delay to allow audit log / ban detection to complete.
pub async fn enrich_departure(
    db: &Db,
    guild_id: GuildId,
    user_id: UserId,
    departure_type: DepartureType,
    moderator_id: Option<&str>,
) -> Result<()> {
    let gid = guild_id.to_string();
    let uid = user_id.to_string();
    let departure_str = departure_type.as_str();

    sqlx::query!(
        r#"
        UPDATE stays SET departure_type = ?, moderator_id = ?
        WHERE guild_id = ? AND user_id = ?
          AND id = (SELECT MAX(id) FROM stays WHERE guild_id = ? AND user_id = ?)
        "#,
        departure_str,
        moderator_id,
        gid,
        uid,
        gid,
        uid
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Update a stay's invite info after detection completes.
pub async fn update_stay_invite(
    db: &Db,
    stay_id: i64,
    invite_code: Option<&str>,
    inviter_id: Option<&str>,
) -> Result<()> {
    sqlx::query!(
        "UPDATE stays SET invite_code = ?, inviter_id = ? WHERE id = ?",
        invite_code,
        inviter_id,
        stay_id
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

// ── Name Change ──────────────────────────────────────────────────────────────

/// Update member names + log change + update FTS.
/// Returns true if any name actually changed.
pub async fn record_name_change(
    db: &Db,
    guild_id: GuildId,
    user_id: UserId,
    account_username: &str,
    display_name: Option<&str>,
    server_nickname: Option<&str>,
) -> Result<bool> {
    let gid = guild_id.to_string();
    let uid = user_id.to_string();

    // Read inside the transaction so concurrent GuildMemberUpdate events
    // are serialized (SQLite serializes writers), preventing duplicate
    // name_changes rows from the same logical change.
    let mut tx = db.pool.begin().await?;

    let current = sqlx::query!(
        r#"
        SELECT account_username, display_name, server_nickname
        FROM members WHERE guild_id = ? AND user_id = ?
        "#,
        gid,
        uid
    )
    .fetch_optional(&mut *tx)
    .await?;

    let changed = match &current {
        Some(r) => {
            r.account_username.as_deref() != Some(account_username)
                || r.display_name.as_deref() != display_name
                || r.server_nickname.as_deref() != server_nickname
        }
        None => true,
    };

    if !changed {
        tx.commit().await?;
        return Ok(false);
    }

    let changed_at = Timestamp::now().to_rfc2822();

    // 1. Update member names
    sqlx::query!(
        r#"
        UPDATE members
        SET account_username = ?, display_name = ?, server_nickname = ?
        WHERE guild_id = ? AND user_id = ?
        "#,
        account_username,
        display_name,
        server_nickname,
        gid,
        uid
    )
    .execute(&mut *tx)
    .await?;

    // 2. Log the name change
    sqlx::query!(
        r#"
        INSERT INTO name_changes (guild_id, user_id, changed_at, account_username, display_name, server_nickname)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
        gid, uid, changed_at, account_username, display_name, server_nickname
    )
    .execute(&mut *tx)
    .await?;

    // 3. Update FTS
    upsert_fts_tx(&mut tx, &gid, &uid).await?;

    tx.commit().await?;
    Ok(true)
}

// ── Ban ──────────────────────────────────────────────────────────────────────

/// Mark the most recent stay as banned and member as not present.
/// Wraps both updates in a transaction so the member state stays consistent
/// even if GuildMemberRemoval is missed.
pub async fn mark_banned(db: &Db, guild_id: GuildId, user_id: UserId) -> Result<()> {
    let mut tx = db.pool.begin().await?;

    let gid = guild_id.to_string();
    let uid = user_id.to_string();
    let left_at = Timestamp::now().to_rfc2822();

    // 1. Close the most recent stay as a ban
    sqlx::query!(
        r#"
        UPDATE stays
        SET departure_type = 'ban', left_at = COALESCE(left_at, ?)
        WHERE guild_id = ? AND user_id = ?
          AND id = (SELECT MAX(id) FROM stays WHERE guild_id = ? AND user_id = ?)
        "#,
        left_at,
        gid,
        uid,
        gid,
        uid
    )
    .execute(&mut *tx)
    .await?;

    // 2. Mark member as not present
    sqlx::query!(
        "UPDATE members SET is_present = 0 WHERE guild_id = ? AND user_id = ?",
        gid,
        uid
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

// ── Startup Sync ─────────────────────────────────────────────────────────────

/// Backfill a member discovered at startup: upsert member + create stay.
#[allow(clippy::too_many_arguments)]
pub async fn backfill_join(
    db: &Db,
    guild_id: GuildId,
    user_id: UserId,
    joined_at: Option<Timestamp>,
    account_username: &str,
    display_name: Option<&str>,
    server_nickname: Option<&str>,
    member_flags: u32,
) -> Result<()> {
    let mut tx = db.pool.begin().await?;

    let gid = guild_id.to_string();
    let uid = user_id.to_string();
    let first_seen = joined_at.unwrap_or_else(Timestamp::now).to_rfc2822();

    // 1. Upsert member
    sqlx::query!(
        r#"
        INSERT INTO members (guild_id, user_id, account_username, display_name, server_nickname, first_seen_at, is_present)
        VALUES (?, ?, ?, ?, ?, ?, 1)
        ON CONFLICT(guild_id, user_id) DO UPDATE SET
            account_username = excluded.account_username,
            display_name     = excluded.display_name,
            server_nickname  = excluded.server_nickname,
            is_present       = 1
        "#,
        gid, uid, account_username, display_name, server_nickname, first_seen
    )
    .execute(&mut *tx)
    .await?;

    // 2. Create stay (only if no open stay already exists)
    let flags_bits = member_flags as i64;
    sqlx::query!(
        r#"
        INSERT INTO stays (guild_id, user_id, joined_at, member_flags)
        SELECT ?, ?, ?, ?
        WHERE NOT EXISTS (
            SELECT 1 FROM stays WHERE guild_id = ? AND user_id = ? AND left_at IS NULL
        )
        "#,
        gid,
        uid,
        first_seen,
        flags_bits,
        gid,
        uid
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Update names on an existing member (startup sync, no name change log).
pub async fn update_names(
    db: &Db,
    guild_id: GuildId,
    user_id: UserId,
    account_username: &str,
    display_name: Option<&str>,
    server_nickname: Option<&str>,
) -> Result<()> {
    let gid = guild_id.to_string();
    let uid = user_id.to_string();
    sqlx::query!(
        r#"
        UPDATE members
        SET account_username = ?, display_name = ?, server_nickname = ?
        WHERE guild_id = ? AND user_id = ?
        "#,
        account_username,
        display_name,
        server_nickname,
        gid,
        uid
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Close stale stays + mark members not present (startup sync).
pub async fn close_stale(
    db: &Db,
    guild_id: GuildId,
    stale_user_ids: &[String],
    reasons: &std::collections::HashMap<String, (bool, bool)>,
) -> Result<u64> {
    if stale_user_ids.is_empty() {
        return Ok(0);
    }

    let mut tx = db.pool.begin().await?;
    let gid = guild_id.to_string();
    let left_at = Timestamp::now().to_rfc2822();
    let mut closed = 0u64;

    for uid in stale_user_ids {
        let &(banned, kicked) = reasons.get(uid).unwrap_or(&(false, false));
        let departure = if banned {
            DepartureType::Ban
        } else if kicked {
            DepartureType::Kick
        } else {
            DepartureType::Leave
        };
        let departure_str = departure.as_str();

        // Close stay
        let result = sqlx::query!(
            r#"
            UPDATE stays SET left_at = ?, departure_type = ?
            WHERE guild_id = ? AND user_id = ? AND left_at IS NULL
            "#,
            left_at,
            departure_str,
            gid,
            uid
        )
        .execute(&mut *tx)
        .await?;
        closed += result.rows_affected();

        // Mark not present
        sqlx::query!(
            "UPDATE members SET is_present = 0 WHERE guild_id = ? AND user_id = ?",
            gid,
            uid
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(closed)
}

// ── Internal: FTS upsert within a transaction ────────────────────────────────

async fn upsert_fts_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    guild_id: &str,
    user_id: &str,
) -> Result<()> {
    // Read current names from members (within the same transaction)
    let row = sqlx::query!(
        r#"
        SELECT account_username, display_name, server_nickname
        FROM members WHERE guild_id = ? AND user_id = ?
        "#,
        guild_id,
        user_id
    )
    .fetch_optional(&mut **tx)
    .await?;

    // Delete old FTS row
    sqlx::query!(
        "DELETE FROM usernames_fts WHERE guild_id = ? AND user_id = ?",
        guild_id,
        user_id
    )
    .execute(&mut **tx)
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
            guild_id, user_id, r.account_username, r.server_nickname, label, label_norm
        )
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}
