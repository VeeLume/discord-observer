use std::sync::Arc;

use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::all::GuildMemberFlags;
use serenity::http::Http;
use serenity::prelude::Context;

use crate::db::Db;
use crate::services::{embeds, membership, settings};
use crate::state::AppState;

pub async fn on_join(
    ctx: &Context,
    state: &Arc<AppState>,
    member: &serenity::all::Member,
) -> Result<()> {
    let guild_id = member.guild_id;

    // Voice-only guests are transient — don't track them
    if member.flags.contains(GuildMemberFlags::IS_GUEST) {
        tracing::trace!(guild_id = %guild_id, user_id = %member.user.id, "Skipping guest join");
        return Ok(());
    }

    // Record the join IMMEDIATELY (no invite info yet — filled in by enrichment)
    let stay_id = membership::record_join(&state.db, guild_id, member, None, None).await?;

    // Spawn background task: detect invite + update stay + post embed
    let http = Arc::clone(&ctx.http);
    let db = state.db.clone();
    let state = Arc::clone(state);
    let member = member.clone();

    tokio::spawn(async move {
        if let Err(e) = enrich_join(&http, &db, &state, &member, stay_id).await {
            tracing::warn!(
                guild_id = %guild_id,
                user_id = %member.user.id,
                "Join enrichment failed: {e:#}"
            );
        }
    });

    Ok(())
}

/// Background enrichment: wait for invite events to settle, detect which
/// invite was used, update the stay, and post the join embed.
async fn enrich_join(
    http: &Http,
    db: &Db,
    state: &AppState,
    member: &serenity::all::Member,
    stay_id: i64,
) -> Result<()> {
    // Wait for InviteCreate/InviteDelete events to settle
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let guild_id = member.guild_id;
    let invite_used = state.detect_invite_used(http, guild_id).await;

    // Update stay with invite info if detected
    if let Some((ref code, ref snap)) = invite_used {
        membership::update_stay_invite(db, stay_id, Some(code), snap.inviter_id.as_deref()).await?;

        tracing::trace!(
            guild_id = %guild_id,
            user_id = %member.user.id,
            invite_code = %code,
            "Detected invite for join"
        );
    }

    let gs = settings::get_settings(db, guild_id).await?;
    embeds::post_join(
        http,
        db,
        guild_id,
        member,
        stay_id,
        &invite_used,
        gs.join_log,
    )
    .await?;

    Ok(())
}
