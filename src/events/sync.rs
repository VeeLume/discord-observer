use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::all::GuildId;
use serenity::futures::StreamExt;
use serenity::prelude::Context;

use crate::invites;
use crate::services::{audit_log, membership, notices, search, settings};
use crate::state::AppState;
use serenity::all::GuildMemberFlags;

pub async fn handle_ready(
    ctx: &Context,
    state: &Arc<AppState>,
    ready: &serenity::Ready,
) -> Result<()> {
    tracing::info!("Connected as {}", ready.user.name);

    for guild in &ready.guilds {
        tracing::info!("Connected to guild: {}", guild.id);
        setup_guild(ctx, state, guild.id).await;
    }

    // Maintenance loop: prune old bans and deleted invites (spawn once only)
    if state.maintenance_started.set(()).is_ok() {
        let state_clone = state.clone();
        tokio::spawn(async move {
            let every_min = std::time::Duration::from_secs(60);
            loop {
                tokio::time::sleep(every_min).await;
                state_clone.prune_recent_bans(60);
                state_clone.prune_deleted_invites(std::time::Duration::from_secs(60));
            }
        });
    }

    Ok(())
}

/// Per-guild bring-up: ensure DB rows, sync members, cache invites, send notices.
/// Called from both `Ready` (existing guilds at startup) and `GuildCreate` (newly joined).
pub async fn setup_guild(ctx: &Context, state: &Arc<AppState>, guild_id: GuildId) {
    settings::ensure_guild(&state.db, guild_id).await.ok();
    settings::ensure_settings_row(&state.db, guild_id)
        .await
        .ok();

    if let Err(e) = sync_members(ctx, state, guild_id).await {
        tracing::warn!("Failed to sync members for guild {}: {}", guild_id, e);
    }

    search::rebuild_fts(&state.db, guild_id)
        .await
        .map_err(|e| tracing::warn!("Failed to rebuild FTS for guild {}: {}", guild_id, e))
        .ok();

    match invites::fetch_invites_map(&ctx.http, guild_id).await {
        Ok(map) => {
            tracing::trace!(guild_id = %guild_id, count = map.len(), "Cached invites");
            state.invite_cache.insert(guild_id, map);
        }
        Err(e) => {
            tracing::warn!("Failed to cache invites for {}: {}", guild_id, e);
        }
    }

    let gs = settings::get_settings(&state.db, guild_id)
        .await
        .unwrap_or_default();
    let first_seen_at = settings::get_first_seen_at(&state.db, guild_id)
        .await
        .ok()
        .flatten();
    let notice_ctx = notices::NoticeContext {
        guild_id,
        settings: gs,
        first_seen_at,
    };
    if let Err(e) = notices::send_pending_notices(&ctx.http, &state.db, &notice_ctx).await {
        tracing::warn!("Failed to send notices for guild {}: {}", guild_id, e);
    }
}

/// Reconcile DB state with reality after the bot was offline.
async fn sync_members(ctx: &Context, state: &AppState, guild_id: GuildId) -> Result<()> {
    let open_ids: HashSet<String> = search::present_user_ids(&state.db, guild_id)
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
                // Voice-only guests are transient — don't track them
                if member.flags.contains(GuildMemberFlags::IS_GUEST) {
                    continue;
                }

                let uid = member.user.id.to_string();
                current_members.insert(uid.clone());

                if open_ids.contains(&uid) {
                    membership::update_names(
                        &state.db,
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
                    membership::backfill_join(
                        &state.db,
                        guild_id,
                        member.user.id,
                        member.joined_at,
                        &member.user.name,
                        member.user.global_name.as_deref(),
                        member.nick.as_deref(),
                        member.flags.bits(),
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

    // Close stale stays
    let stale: Vec<String> = open_ids
        .into_iter()
        .filter(|uid| !current_members.contains(uid))
        .collect();

    let closed = if stale.is_empty() {
        0
    } else {
        let reasons = audit_log::classify_stale_removals(&ctx.http, guild_id, &stale).await;
        membership::close_stale(&state.db, guild_id, &stale, &reasons).await?
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
