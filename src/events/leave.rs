use std::sync::Arc;

use anyhow::Result;
use serenity::all::{GuildId, User};
use serenity::http::Http;

use crate::services::stats::DepartureType;
use crate::services::{audit_log, embeds, membership, mentions, settings};
use crate::state::AppState;

pub async fn on_leave(
    http: &Arc<Http>,
    state: &Arc<AppState>,
    guild_id: &GuildId,
    user: &User,
) -> Result<()> {
    // Record leave IMMEDIATELY — close the stay and mark not present.
    // Departure type and moderator info are filled in by the enrichment task.
    membership::record_leave_immediate(&state.db, *guild_id, user.id).await?;

    // Spawn background task: audit log check + enrich departure + embed + mention edits
    let http = Arc::clone(http);
    let state = Arc::clone(state);
    let guild_id = *guild_id;
    let user = user.clone();

    tokio::spawn(async move {
        if let Err(e) = enrich_leave(&http, &state, guild_id, &user).await {
            tracing::warn!(
                guild_id = %guild_id,
                user_id = %user.id,
                "Leave enrichment failed: {e:#}"
            );
        }
    });

    Ok(())
}

/// Background enrichment: wait for ban events to settle, check audit log,
/// enrich the departure record, and post the leave embed.
async fn enrich_leave(
    http: &Arc<Http>,
    state: &AppState,
    guild_id: GuildId,
    user: &User,
) -> Result<()> {
    // Delay to give GuildBanAddition time to arrive first
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let reason = audit_log::detect_leave_reason(state, http, guild_id, user).await;

    let (departure_type, moderator_id_str) = match &reason {
        audit_log::RemovalReason::Banned { moderator_id, .. } => (
            DepartureType::Ban,
            moderator_id.map(|id| id.get().to_string()),
        ),
        audit_log::RemovalReason::Kicked { moderator_id, .. } => (
            DepartureType::Kick,
            moderator_id.map(|id| id.get().to_string()),
        ),
        audit_log::RemovalReason::Left => (DepartureType::Leave, None),
    };

    tracing::trace!(
        guild_id = %guild_id,
        user_id = %user.id,
        departure_type = departure_type.as_str(),
        "Processing leave"
    );

    // Enrich the already-closed stay with departure reason + moderator
    membership::enrich_departure(
        &state.db,
        guild_id,
        user.id,
        departure_type,
        moderator_id_str.as_deref(),
    )
    .await?;

    let gs = settings::get_settings(&state.db, guild_id).await?;
    let banned = matches!(reason, audit_log::RemovalReason::Banned { .. });
    let kicked = matches!(reason, audit_log::RemovalReason::Kicked { .. });
    let target = if banned || kicked {
        gs.mod_log.or(gs.leave_log)
    } else {
        gs.leave_log
    };

    embeds::post_leave(http, &state.db, guild_id, user, &reason, target).await?;

    // Edit all embeds that reference this departing user (spawns its own background task)
    let display = user.global_name.as_deref().unwrap_or(&user.name);
    mentions::edit_departed_mentions(http, &state.db, guild_id, &user.id.to_string(), display);

    Ok(())
}
