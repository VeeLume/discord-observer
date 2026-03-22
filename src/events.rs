use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use poise::FrameworkContext;
use poise::serenity_prelude as serenity;
use serenity::all::{
    ChannelId, CreateEmbed, CreateMessage, EditMessage, GuildId, MessageId, Timestamp, User, UserId,
};
use serenity::futures::StreamExt;
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
        GuildMemberUpdate { event, .. } => {
            tracing::trace!(
                t,
                guild_id = %event.guild_id,
                user_id = %event.user.id,
                nick = ?event.nick,
                "GuildMemberUpdate"
            );
            on_member_update(state, event).await?;
        }
        GuildMemberRemoval { guild_id, user, .. } => {
            tracing::trace!(
                t,
                guild_id = %guild_id,
                user_id = %user.id,
                username = %user.name,
                "GuildMemberRemoval"
            );
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

/// Post an embed to a channel and return (channel_id, message_id) if successful.
async fn post_embed(
    http: &serenity::http::Http,
    channel: Option<ChannelId>,
    embed: CreateEmbed,
) -> Option<(ChannelId, serenity::all::MessageId)> {
    let ch = channel?;
    match ch
        .send_message(http, CreateMessage::new().embed(embed))
        .await
    {
        Ok(msg) => Some((ch, msg.id)),
        Err(_) => None,
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
        // Sync members: update names that changed while offline, close stale stays
        if let Err(e) = sync_members(ctx, &mrepo, guild.id).await {
            tracing::warn!("Failed to sync members for guild {}: {}", guild.id, e);
        }

        // Rebuild FTS after sync so it reflects the freshest names
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

        // Ensure guild row exists and first_seen_at is set
        let grepo = GuildSettingsRepo::new(&state.db);
        grepo.ensure_row(&guild.id).await.ok();

        // Send any pending one-time notices
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

// ── Startup Sync ─────────────────────────────────────────────────────────────

/// Reconcile DB state with reality after the bot was offline (or on first connect).
/// - Backfills stays for members not yet in the DB (joined while offline, or existing
///   members when the bot first joins a server).
/// - Updates account_username, display_name, and server_nickname for current members.
/// - Closes open stays for users who left while the bot was offline, using the audit
///   log to classify bans/kicks where possible.
async fn sync_members(ctx: &Context, mrepo: &MembershipsRepo<'_>, guild_id: GuildId) -> Result<()> {
    // Snapshot of who currently has an open stay — used to decide update vs backfill
    let open_ids: HashSet<String> = mrepo
        .open_stay_user_ids(guild_id)
        .await?
        .into_iter()
        .collect();

    let mut current_members: HashSet<String> = HashSet::new();
    let mut stream = guild_id.members_iter(&ctx.http).boxed();
    let mut updated = 0u64;
    let mut backfilled = 0u64;

    while let Some(result) = stream.next().await {
        match result {
            Ok(member) => {
                let uid = member.user.id.to_string();
                current_members.insert(uid.clone());

                if open_ids.contains(&uid) {
                    // Existing stay — refresh names
                    mrepo
                        .update_names(
                            guild_id,
                            member.user.id,
                            &member.user.name,
                            member.user.global_name.as_deref(),
                            member.nick.as_deref(),
                        )
                        .await
                        .ok();
                    updated += 1;
                } else {
                    // No open stay — backfill using Discord's actual join timestamp
                    mrepo
                        .record_backfill_join(
                            guild_id,
                            member.user.id,
                            member.joined_at,
                            &member.user.name,
                            member.user.global_name.as_deref(),
                            member.nick.as_deref(),
                        )
                        .await
                        .ok();
                    backfilled += 1;
                }
            }
            Err(e) => {
                tracing::warn!(guild_id = %guild_id, "Error streaming members: {e}");
                break;
            }
        }
    }

    // Close open stays for users no longer in the guild
    let stale: Vec<String> = open_ids
        .into_iter()
        .filter(|uid| !current_members.contains(uid))
        .collect();

    let closed = if stale.is_empty() {
        0
    } else {
        // Check audit log for bans/kicks before closing
        let reasons = check_audit_log_for_stale_removals(&ctx.http, guild_id, &stale).await;
        mrepo
            .close_stale_stays_classified(guild_id, &stale, &reasons)
            .await?
    };

    tracing::info!(
        guild_id = %guild_id,
        updated,
        backfilled,
        closed,
        "Startup member sync complete"
    );
    Ok(())
}

/// Check the audit log for recent bans/kicks targeting any of the stale user IDs.
/// Returns a map of user_id → (banned, kicked) for those found in the log.
/// Users not in the returned map are treated as voluntary leaves.
async fn check_audit_log_for_stale_removals(
    http: &serenity::http::Http,
    guild_id: GuildId,
    stale_user_ids: &[String],
) -> std::collections::HashMap<String, (bool, bool)> {
    use serenity::all::audit_log::{Action, MemberAction};

    let stale_set: HashSet<&str> = stale_user_ids.iter().map(|s| s.as_str()).collect();
    let mut results = std::collections::HashMap::new();

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
            let target_id = match entry.target_id {
                Some(t) => t.get().to_string(),
                None => continue,
            };

            if !stale_set.contains(target_id.as_str()) {
                continue;
            }

            // Only record the first (most recent) action per user
            results
                .entry(target_id)
                .or_insert_with(|| (is_ban, !is_ban));
        }
    }

    results
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

    let inviter_id = invite_used
        .as_ref()
        .and_then(|(_, snap)| snap.inviter_id.as_deref());
    let inviter_name = invite_used
        .as_ref()
        .and_then(|(_, snap)| snap.inviter_name.as_deref());

    let mrepo = MembershipsRepo::new(&state.db);
    mrepo
        .record_join(guild_id, member, invite_code, inviter_id, inviter_name)
        .await?;
    mrepo
        .upsert_usernames_fts_row(guild_id, &user_id.to_string())
        .await?;

    // Fetch history for rejoin detection and previous embed link
    let history = mrepo.history_for_user(guild_id, user_id).await?;
    let stay_count = history.len();

    // Find the previous stay's leave embed (second-to-last entry, which should have left_at set)
    let prev_embed_link = if history.len() >= 2 {
        let prev = &history[history.len() - 2];
        match (&prev.embed_channel_id, &prev.embed_message_id) {
            (Some(ch), Some(msg)) => Some(format!(
                "https://discord.com/channels/{}/{}/{}",
                guild_id.get(),
                ch,
                msg
            )),
            _ => None,
        }
    } else {
        None
    };

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
        .field("Username", &member.user.name, true)
        .field("Account Created", format!("<t:{}:R>", created_unix), true)
        .timestamp(Timestamp::now());

    // Invite info — use @mention for the inviter (clickable while they're on the server)
    let invite_text = match &invite_used {
        Some((code, snap)) => match snap.inviter_id.as_deref() {
            Some(id) => format!("`{code}` by <@{id}>"),
            None => {
                let inviter = snap.inviter_name.as_deref().unwrap_or("Unknown");
                format!("`{code}` by *{inviter}*")
            }
        },
        None => "Unknown".to_string(),
    };
    embed = embed.field("Invite", invite_text, true);

    // Rejoin indicator
    if stay_count > 1 {
        embed = embed.field("Rejoin", format!("Join #{stay_count}"), true);
    }

    // Link to previous leave embed
    if let Some(link) = prev_embed_link {
        embed = embed.field("Previous", format!("[Leave embed]({link})"), true);
    }

    // New account warning
    if is_new_account {
        embed = embed.field("⚠ New Account", "Created less than 7 days ago", false);
    }

    // Post embed and store the message reference
    if let Some((ch, msg)) = post_embed(&ctx.http, settings.join_log, embed).await {
        mrepo
            .set_embed_ref(
                guild_id,
                user_id,
                &ch.get().to_string(),
                &msg.get().to_string(),
            )
            .await
            .ok();
    }

    Ok(())
}

// ── Member Update ────────────────────────────────────────────────────────────

async fn on_member_update(
    state: &AppState,
    event: &serenity::GuildMemberUpdateEvent,
) -> Result<()> {
    let mrepo = MembershipsRepo::new(&state.db);
    mrepo
        .update_names(
            event.guild_id,
            event.user.id,
            &event.user.name,
            event.user.global_name.as_deref(),
            event.nick.as_deref(),
        )
        .await?;
    mrepo
        .upsert_usernames_fts_row(event.guild_id, &event.user.id.to_string())
        .await?;
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
    // Delay leave processing to give GuildBanAddition time to arrive first,
    // so we can correctly classify bans vs voluntary leaves.
    // GuildBanAddition typically arrives ~65ms before GuildMemberRemoval;
    // 3s gives plenty of margin.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

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
    let moderator_id = match &leave_reason {
        LeaveReason::Banned { moderator_id, .. } | LeaveReason::Kicked { moderator_id, .. } => {
            Some(moderator_id.get().to_string())
        }
        LeaveReason::Left => None,
    };

    tracing::trace!(
        guild_id = %guild_id,
        user_id = %user.id,
        banned,
        kicked,
        "Processing leave"
    );

    let mrepo = MembershipsRepo::new(&state.db);
    mrepo
        .record_leave(*guild_id, user.id, banned, kicked, moderator_id.as_deref())
        .await?;

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

    // Use the most recognizable name: server nickname > display name > username
    let best_name = closed_stay
        .and_then(|s| s.server_nickname.as_deref())
        .or_else(|| closed_stay.and_then(|s| s.display_name.as_deref()))
        .or_else(|| user.global_name.as_deref())
        .unwrap_or(&user.name);

    let mut embed = CreateEmbed::new()
        .color(color)
        .title(title)
        .description(format!("**{best_name}** {action}"))
        .thumbnail(user.face())
        .field("User ID", format!("<@{}>", user.id.get()), true)
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

    // Invite info from the closed stay
    if let Some(stay) = closed_stay {
        let invite_text = match (&stay.invite_code, &stay.inviter_name) {
            (Some(code), Some(inviter)) => format!("`{code}` by *{inviter}*"),
            (Some(code), None) => format!("`{code}`"),
            _ => "Unknown".to_string(),
        };
        embed = embed.field("Invite", invite_text, true);
    }

    // Joined timestamp from the closed stay
    if let Some(stay) = closed_stay {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(&stay.joined_at) {
            embed = embed.field("Joined", format!("<t:{}:R>", dt.timestamp()), true);
        }
    }

    // Link to join embed
    if let Some(stay) = closed_stay {
        if let (Some(ch), Some(msg)) = (&stay.embed_channel_id, &stay.embed_message_id) {
            let link = format!(
                "https://discord.com/channels/{}/{}/{}",
                guild_id.get(),
                ch,
                msg
            );
            embed = embed.field("Previous", format!("[Join Embed]({link})"), true);
        }
    }

    // Total stays
    if stay_count > 1 {
        embed = embed.field("Total Stays", format!("{stay_count}"), true);
    }

    // Account created
    let created_unix = account_created_unix(user);
    embed = embed.field("Account Created", format!("<t:{created_unix}:R>"), true);

    // Post embed and store the message reference
    if let Some((ch, msg)) = post_embed(http, target, embed).await {
        mrepo
            .set_embed_ref(
                *guild_id,
                user.id,
                &ch.get().to_string(),
                &msg.get().to_string(),
            )
            .await
            .ok();
    }

    // Edit the original join embed: replace the broken @mention with the best name
    if let Some(stay) = closed_stay {
        if let (Some(ch), Some(msg)) = (&stay.embed_channel_id, &stay.embed_message_id) {
            if let (Ok(ch_id), Ok(msg_id)) = (ch.parse::<u64>(), msg.parse::<u64>()) {
                let channel_id = ChannelId::new(ch_id);
                let message_id = MessageId::new(msg_id);

                // Fetch the original message to preserve existing embeds
                if let Ok(original) = channel_id.message(http, message_id).await {
                    if let Some(original_embed) = original.embeds.first() {
                        // Rebuild the embed with the name instead of the mention
                        let mut updated = CreateEmbed::new()
                            .title(original_embed.title.as_deref().unwrap_or("Member joined"))
                            .color(
                                original_embed
                                    .colour
                                    .unwrap_or(serenity::all::Colour::new(0x2ecc71)),
                            )
                            .description(format!("**{best_name}** joined the server"))
                            .field("User ID", format!("<@{}>", user.id.get()), true);

                        // Preserve existing fields, replacing inviter @mention with name
                        for field in &original_embed.fields {
                            let value = if field.name == "Invite" {
                                // Replace <@inviter_id> with *inviter_name*
                                if let Some(stay) = closed_stay {
                                    match (&stay.invite_code, &stay.inviter_id, &stay.inviter_name)
                                    {
                                        (Some(code), Some(_), Some(name)) => {
                                            format!("`{code}` by *{name}*")
                                        }
                                        _ => field.value.clone(),
                                    }
                                } else {
                                    field.value.clone()
                                }
                            } else {
                                field.value.clone()
                            };
                            updated = updated.field(&field.name, value, field.inline);
                        }

                        // Preserve thumbnail and timestamp
                        if let Some(thumb) = &original_embed.thumbnail {
                            updated = updated.thumbnail(&thumb.url);
                        }
                        if let Some(ts) = original_embed.timestamp {
                            updated = updated.timestamp(ts);
                        }

                        channel_id
                            .edit_message(http, message_id, EditMessage::new().embed(updated))
                            .await
                            .ok();
                    }
                }
            }
        }
    }

    // Edit leave embeds where this user was the moderator (ban/kick actions).
    // When a mod leaves, their <@id> mentions break — replace with display name.
    let mod_user_id = user.id.to_string();
    let mod_embeds = mrepo
        .embeds_by_moderator(*guild_id, &mod_user_id)
        .await
        .unwrap_or_default();
    if !mod_embeds.is_empty() {
        let mod_display = user.global_name.as_deref().unwrap_or(&user.name);
        for (ch, msg) in mod_embeds {
            if let (Ok(ch_id), Ok(msg_id)) = (ch.parse::<u64>(), msg.parse::<u64>()) {
                let channel_id = ChannelId::new(ch_id);
                let message_id = MessageId::new(msg_id);
                if let Ok(original) = channel_id.message(http, message_id).await {
                    if let Some(original_embed) = original.embeds.first() {
                        let mut updated = CreateEmbed::new().color(
                            original_embed
                                .colour
                                .unwrap_or(serenity::all::Colour::new(0xe74c3c)),
                        );
                        if let Some(t) = &original_embed.title {
                            updated = updated.title(t);
                        }
                        if let Some(d) = &original_embed.description {
                            updated = updated.description(d);
                        }
                        if let Some(thumb) = &original_embed.thumbnail {
                            updated = updated.thumbnail(&thumb.url);
                        }
                        if let Some(ts) = original_embed.timestamp {
                            updated = updated.timestamp(ts);
                        }

                        // Preserve fields, replacing "By" with display name
                        for field in &original_embed.fields {
                            let value = if field.name == "By" {
                                format!("*{mod_display}*")
                            } else {
                                field.value.clone()
                            };
                            updated = updated.field(&field.name, value, field.inline);
                        }

                        channel_id
                            .edit_message(http, message_id, EditMessage::new().embed(updated))
                            .await
                            .ok();

                        // Small delay between edits to avoid rate limits
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        }
    }

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
            inviter_id: data.inviter.as_ref().map(|u| u.id.to_string()),
            inviter_name: data.inviter.as_ref().map(|u| u.display_name().to_string()),
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
