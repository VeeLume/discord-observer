use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use poise::FrameworkContext;
use poise::serenity_prelude as serenity;
use serenity::all::{ChannelId, CreateEmbed, CreateMessage, GuildId, Timestamp, User, UserId};
use serenity::prelude::Context;

use crate::invites;
use crate::notices::{self, NoticeContext};
use crate::repos::{GuildSettingsRepo, MembershipsRepo};
use crate::state::AppState;

/// Milliseconds since bot boot — for comparing event timing in logs.
fn ms_since(boot: Instant) -> u64 {
    boot.elapsed().as_millis() as u64
}

pub async fn event_handler(
    ctx: &Context,
    event: &serenity::FullEvent,
    _framework: FrameworkContext<'_, Arc<AppState>, anyhow::Error>,
    state: &Arc<AppState>,
) -> Result<()> {
    use serenity::FullEvent::*;
    let t = ms_since(state.boot);
    match event {
        Ready { data_about_bot, .. } => handle_ready(ctx, state, data_about_bot).await?,
        GuildMemberAddition { new_member } => {
            tracing::trace!(
                t,
                guild_id = %new_member.guild_id,
                user_id = %new_member.user.id,
                username = %new_member.user.name,
                "GuildMemberAddition"
            );
            on_join(ctx, state, new_member).await?;
        }
        GuildMemberRemoval { guild_id, user, .. } => {
            tracing::trace!(
                t,
                guild_id = %guild_id,
                user_id = %user.id,
                username = %user.name,
                "GuildMemberRemoval"
            );
            // Delay leave processing to give GuildBanAddition time to arrive first,
            // so we can correctly classify bans vs voluntary leaves.
            // GuildBanAddition typically arrives ~65ms before GuildMemberRemoval;
            // 3s gives plenty of margin.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            on_leave(&ctx.http, state, guild_id, user).await?;
        }
        GuildBanAddition {
            guild_id,
            banned_user,
        } => {
            tracing::trace!(
                t,
                guild_id = %guild_id,
                user_id = %banned_user.id,
                username = %banned_user.name,
                "GuildBanAddition"
            );
            on_guild_ban_add(state, *guild_id, banned_user).await?;
        }
        InviteCreate { data } => {
            tracing::trace!(
                t,
                code = %data.code,
                uses = data.uses,
                max_uses = data.max_uses,
                inviter = ?data.inviter.as_ref().map(|u| &u.name),
                guild_id = ?data.guild_id,
                "InviteCreate"
            );
            on_invite_create(state, data);
        }
        InviteDelete { data } => {
            tracing::trace!(
                t,
                code = %data.code,
                guild_id = ?data.guild_id,
                "InviteDelete"
            );
            on_invite_delete(state, data);
        }
        _ => {}
    }
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

async fn post_embed(http: &serenity::http::Http, channel: Option<ChannelId>, embed: CreateEmbed) {
    if let Some(ch) = channel {
        let _ = ch
            .send_message(http, CreateMessage::new().embed(embed))
            .await;
    }
}

/// Seconds since Unix epoch for a serenity Timestamp.
fn ts_unix(ts: Timestamp) -> i64 {
    ts.unix_timestamp()
}

/// Seconds since Unix epoch for a user's account creation (derived from snowflake ID).
fn account_created_unix(user: &serenity::User) -> i64 {
    ts_unix(user.created_at())
}

const SEVEN_DAYS_SECS: i64 = 7 * 24 * 60 * 60;

// ── Ready ────────────────────────────────────────────────────────────────────

pub async fn handle_ready(
    ctx: &Context,
    state: &Arc<AppState>,
    ready: &serenity::Ready,
) -> Result<()> {
    tracing::info!("Connected as {}", ready.user.name);

    let mrepo = MembershipsRepo::new(&state.db);
    for guild in &ready.guilds {
        tracing::info!("Connected to guild: {}", guild.id);
        mrepo
            .rebuild_usernames_fts_for_guild(guild.id)
            .await
            .map_err(|e| {
                tracing::warn!(
                    "Failed to rebuild usernames FTS for guild {}: {}",
                    guild.id,
                    e
                )
            })
            .ok();

        // Populate invite cache
        match invites::fetch_invites_map(&ctx.http, guild.id).await {
            Ok(map) => {
                tracing::trace!(guild_id = %guild.id, count = map.len(), "Cached invites on startup");
                state.invite_cache.insert(guild.id, map);
            }
            Err(e) => {
                tracing::warn!("Failed to cache invites for {}: {}", guild.id, e);
            }
        }

        // Send any pending one-time notices
        let grepo = GuildSettingsRepo::new(&state.db);
        let settings = grepo.get(&guild.id).await.unwrap_or_default();
        let notice_ctx = NoticeContext {
            guild_id: guild.id,
            settings,
        };
        if let Err(e) = notices::send_pending_notices(&ctx.http, &state.db, &notice_ctx).await {
            tracing::warn!("Failed to send notices for guild {}: {}", guild.id, e);
        }
    }

    // Maintenance loop: prune old bans and deleted invites
    let state_clone = state.clone();
    tokio::spawn(async move {
        let every_min = std::time::Duration::from_secs(60);
        loop {
            tokio::time::sleep(every_min).await;
            state_clone.prune_recent_bans(60);
            state_clone.prune_deleted_invites(std::time::Duration::from_secs(60));
        }
    });

    Ok(())
}

// ── Join ─────────────────────────────────────────────────────────────────────

pub async fn on_join(
    ctx: &Context,
    state: &AppState,
    member: &serenity::all::Member,
) -> Result<()> {
    let guild_id = member.guild_id;
    let user_id = member.user.id;

    // Wait for InviteCreate/InviteDelete events to settle before detection.
    // InviteDelete typically arrives within ~2ms of GuildMemberAddition;
    // 3s gives plenty of margin.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Detect which invite was used (and update cache)
    let invite_used = state.detect_invite_used(&ctx.http, guild_id).await;
    let invite_code = invite_used.as_ref().map(|(code, _)| code.as_str());

    tracing::trace!(
        t = ms_since(state.boot),
        guild_id = %guild_id,
        user_id = %user_id,
        invite_code = ?invite_code,
        "Detected invite for join"
    );

    let mrepo = MembershipsRepo::new(&state.db);
    mrepo.record_join(guild_id, member, invite_code).await?;
    mrepo
        .upsert_usernames_fts_row(guild_id, &user_id.to_string())
        .await?;

    // Count stays for rejoin detection
    let history = mrepo.history_for_user(guild_id, user_id).await?;
    let stay_count = history.len();

    let grepo = GuildSettingsRepo::new(&state.db);
    let settings = grepo.get(&guild_id).await?;

    // Build rich embed
    let created_unix = account_created_unix(&member.user);
    let now_unix = ts_unix(Timestamp::now());
    let account_age_secs = now_unix - created_unix;
    let is_new_account = account_age_secs < SEVEN_DAYS_SECS;

    let color = if is_new_account { 0xe67e22 } else { 0x2ecc71 };

    let mut embed = CreateEmbed::new()
        .color(color)
        .title("Member joined")
        .description(format!("<@{}> joined the server", user_id.get()))
        .thumbnail(member.user.face())
        .field("Account Created", format!("<t:{}:R>", created_unix), true)
        .timestamp(Timestamp::now());

    // Invite info
    let invite_text = match &invite_used {
        Some((code, snap)) => {
            let inviter = snap.inviter_name.as_deref().unwrap_or("Unknown");
            format!("`{code}` by *{inviter}*")
        }
        None => "Unknown".to_string(),
    };
    embed = embed.field("Invite", invite_text, true);

    // Rejoin indicator
    if stay_count > 1 {
        embed = embed.field("Rejoin", format!("Join #{stay_count}"), true);
    }

    // New account warning
    if is_new_account {
        embed = embed.field("⚠ New Account", "Created less than 7 days ago", false);
    }

    post_embed(&ctx.http, settings.join_log, embed).await;

    Ok(())
}

// ── Leave ────────────────────────────────────────────────────────────────────

/// The reason a member left the server, determined from gateway events and audit log.
enum LeaveReason {
    /// Voluntarily left
    Left,
    /// Kicked by a moderator
    Kicked {
        moderator_id: UserId,
        reason: Option<String>,
    },
    /// Banned by a moderator
    Banned {
        moderator_id: UserId,
        reason: Option<String>,
    },
}

/// Check the audit log for a recent kick or ban targeting this user.
async fn check_audit_log_for_removal(
    http: &serenity::http::Http,
    guild_id: GuildId,
    user: &User,
) -> Option<LeaveReason> {
    use serenity::all::audit_log::{Action, MemberAction};

    // Check ban first (most specific), then kick
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
            // Match on target
            let target_matches = entry
                .target_id
                .map(|t| t.get() == user.id.get())
                .unwrap_or(false);
            if !target_matches {
                continue;
            }

            // Check entry is recent (within 30 seconds)
            let entry_ts = entry.id.created_at().unix_timestamp();
            let now_ts = Timestamp::now().unix_timestamp();
            if (now_ts - entry_ts).abs() > 30 {
                continue;
            }

            let reason = entry.reason.clone();
            let moderator_id = entry.user_id;

            return match entry.action {
                Action::Member(MemberAction::BanAdd) => Some(LeaveReason::Banned {
                    moderator_id,
                    reason,
                }),
                Action::Member(MemberAction::Kick) => Some(LeaveReason::Kicked {
                    moderator_id,
                    reason,
                }),
                _ => None,
            };
        }
    }
    None
}

pub async fn on_leave(
    http: &serenity::http::Http,
    state: &AppState,
    guild_id: &GuildId,
    user: &User,
) -> Result<()> {
    // Determine leave reason: check gateway event (fast) then audit log (detailed)
    let leave_reason = if state.was_recently_banned(*guild_id, user.id, 15) {
        // GuildBanAddition already fired — check audit log for moderator info
        check_audit_log_for_removal(http, *guild_id, user)
            .await
            .unwrap_or(LeaveReason::Banned {
                moderator_id: UserId::new(1), // placeholder if audit log fails
                reason: None,
            })
    } else {
        // Check audit log for kicks (no gateway event for kicks)
        check_audit_log_for_removal(http, *guild_id, user)
            .await
            .unwrap_or(LeaveReason::Left)
    };

    let banned = matches!(leave_reason, LeaveReason::Banned { .. });
    let kicked = matches!(leave_reason, LeaveReason::Kicked { .. });

    tracing::trace!(
        guild_id = %guild_id,
        user_id = %user.id,
        banned,
        kicked,
        "Processing leave"
    );

    let mrepo = MembershipsRepo::new(&state.db);
    mrepo.record_leave(*guild_id, user.id, banned).await?;

    // Fetch history to get join date and stay count
    let history = mrepo.history_for_user(*guild_id, user.id).await?;
    let stay_count = history.len();

    // Find the just-closed stay (most recent with left_at set)
    let closed_stay = history.iter().rev().find(|r| r.left_at.is_some());

    let grepo = GuildSettingsRepo::new(&state.db);
    let settings = grepo.get(guild_id).await?;
    let target = if banned || kicked {
        settings.mod_log.or(settings.leave_log)
    } else {
        settings.leave_log
    };

    let (color, action, title) = match &leave_reason {
        LeaveReason::Banned { .. } => (0xe74c3c, "was **banned**", "Member Banned"),
        LeaveReason::Kicked { .. } => (0xe67e22, "was **kicked**", "Member Kicked"),
        LeaveReason::Left => (0x95a5a6, "left the server", "Member Left"),
    };

    let mut embed = CreateEmbed::new()
        .color(color)
        .title(title)
        .description(format!("<@{}> {action}", user.id.get()))
        .thumbnail(user.face())
        .field("Username", &user.name, true)
        .timestamp(Timestamp::now());

    // Moderator info for bans and kicks
    match &leave_reason {
        LeaveReason::Banned {
            moderator_id,
            reason,
        }
        | LeaveReason::Kicked {
            moderator_id,
            reason,
        } => {
            embed = embed.field("By", format!("<@{}>", moderator_id.get()), true);
            if let Some(r) = reason {
                embed = embed.field("Reason", r, false);
            }
        }
        LeaveReason::Left => {}
    }

    // Joined timestamp from the closed stay
    if let Some(stay) = closed_stay {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(&stay.joined_at) {
            embed = embed.field("Joined", format!("<t:{}:R>", dt.timestamp()), true);
        }
    }

    // Total stays
    if stay_count > 1 {
        embed = embed.field("Total Stays", format!("{stay_count}"), true);
    }

    // Account created
    let created_unix = account_created_unix(user);
    embed = embed.field("Account Created", format!("<t:{created_unix}:R>"), true);

    post_embed(http, target, embed).await;

    Ok(())
}

// ── Bans ─────────────────────────────────────────────────────────────────────

async fn on_guild_ban_add(state: &AppState, guild_id: GuildId, banned_user: &User) -> Result<()> {
    state.mark_recent_ban(guild_id, banned_user.id);

    // Mark the most recent stay as banned. Uses COALESCE so it works whether
    // GuildMemberRemoval already fired (left_at set) or not (left_at NULL).
    let mrepo = MembershipsRepo::new(&state.db);
    let _ = mrepo.mark_banned(guild_id, banned_user.id).await;
    Ok(())
}

// ── Invite cache maintenance ─────────────────────────────────────────────────

fn on_invite_create(state: &AppState, data: &serenity::InviteCreateEvent) {
    let guild_id = match data.guild_id {
        Some(g) => g,
        None => return,
    };
    let tracked = crate::invites::TrackedInvite {
        snapshot: crate::invites::InviteSnapshot {
            uses: data.uses,
            max_uses: data.max_uses as u64,
            inviter_name: data.inviter.as_ref().map(|u| u.name.clone()),
        },
        created_at: Instant::now(),
        deleted_at: None,
    };

    state
        .invite_cache
        .entry(guild_id)
        .or_default()
        .insert(data.code.clone(), tracked);
}

fn on_invite_delete(state: &AppState, data: &serenity::InviteDeleteEvent) {
    let guild_id = match data.guild_id {
        Some(g) => g,
        None => return,
    };
    if let Some(mut map) = state.invite_cache.get_mut(&guild_id) {
        if let Some(tracked) = map.get_mut(&data.code) {
            tracked.deleted_at = Some(Instant::now());
        }
    }
}
