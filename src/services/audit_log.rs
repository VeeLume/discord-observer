//! Audit log queries for classifying member departures.
//!
//! Consolidates the two near-duplicate audit log functions from events.rs
//! into a unified module.

use std::collections::{HashMap, HashSet};

use serenity::all::{GuildId, Timestamp, User, UserId};
use serenity::http::Http;

use crate::state::AppState;

// ── Types ────────────────────────────────────────────────────────────────────

/// Why a member left the server.
pub enum RemovalReason {
    Left,
    Kicked {
        moderator_id: Option<UserId>,
        reason: Option<String>,
    },
    Banned {
        moderator_id: Option<UserId>,
        reason: Option<String>,
    },
}

// ── Individual leave detection ───────────────────────────────────────────────

/// Determine why a user left: check in-memory ban cache, then audit log.
pub async fn detect_leave_reason(
    state: &AppState,
    http: &Http,
    guild_id: GuildId,
    user: &User,
) -> RemovalReason {
    if state.was_recently_banned(guild_id, user.id, 15) {
        // GuildBanAddition already fired — check audit log for moderator info
        check_audit_log(http, guild_id, user)
            .await
            .unwrap_or(RemovalReason::Banned {
                moderator_id: None,
                reason: None,
            })
    } else {
        // Check audit log for kicks (no gateway event for kicks)
        check_audit_log(http, guild_id, user)
            .await
            .unwrap_or(RemovalReason::Left)
    }
}

/// Check the audit log for a recent kick or ban targeting this user.
async fn check_audit_log(http: &Http, guild_id: GuildId, user: &User) -> Option<RemovalReason> {
    use serenity::all::audit_log::{Action, MemberAction};

    for action_type in [
        Action::Member(MemberAction::BanAdd),
        Action::Member(MemberAction::Kick),
    ] {
        let logs = match guild_id
            .audit_logs(http, Some(action_type), None, None, Some(5))
            .await
        {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(guild_id = %guild_id, "Failed to fetch audit logs: {e}");
                return None;
            }
        };

        for entry in &logs.entries {
            let target_matches = entry
                .target_id
                .map(|t| t.get() == user.id.get())
                .unwrap_or(false);
            if !target_matches {
                continue;
            }

            let entry_ts = entry.id.created_at().unix_timestamp();
            let now_ts = Timestamp::now().unix_timestamp();
            if (now_ts - entry_ts).abs() > 30 {
                continue;
            }

            let reason = entry.reason.clone();
            let moderator_id = Some(entry.user_id);

            match entry.action {
                Action::Member(MemberAction::BanAdd) => {
                    return Some(RemovalReason::Banned {
                        moderator_id,
                        reason,
                    });
                }
                Action::Member(MemberAction::Kick) => {
                    return Some(RemovalReason::Kicked {
                        moderator_id,
                        reason,
                    });
                }
                _ => continue,
            }
        }
    }
    None
}

// ── Batch classification (startup sync) ──────────────────────────────────────

/// Check audit log for recent bans/kicks targeting any of the stale user IDs.
/// Returns a map of user_id → (banned, kicked) for those found.
/// Only considers entries from the last 6 hours to avoid mis-classifying
/// users who simply left during extended downtime.
pub async fn classify_stale_removals(
    http: &Http,
    guild_id: GuildId,
    stale_user_ids: &[String],
) -> HashMap<String, (bool, bool)> {
    use serenity::all::audit_log::{Action, MemberAction};

    let stale_set: HashSet<&str> = stale_user_ids.iter().map(|s| s.as_str()).collect();
    let mut results = HashMap::new();
    let now_ts = Timestamp::now().unix_timestamp();

    for (action_type, is_ban) in [
        (Action::Member(MemberAction::BanAdd), true),
        (Action::Member(MemberAction::Kick), false),
    ] {
        let logs = match guild_id
            .audit_logs(http, Some(action_type), None, None, Some(50))
            .await
        {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(guild_id = %guild_id, "Failed to fetch audit logs for startup sync: {e}");
                continue;
            }
        };

        for entry in &logs.entries {
            // Skip entries older than 6 hours
            let entry_ts = entry.id.created_at().unix_timestamp();
            if (now_ts - entry_ts).abs() > 6 * 3600 {
                continue;
            }

            let target_id = match entry.target_id {
                Some(t) => t.get().to_string(),
                None => continue,
            };

            if !stale_set.contains(target_id.as_str()) {
                continue;
            }

            results
                .entry(target_id)
                .or_insert_with(|| (is_ban, !is_ban));
        }
    }

    results
}
