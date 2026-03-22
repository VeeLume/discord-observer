use std::time::Instant;

use anyhow::Result;
use serenity::all::{GuildId, User};

use crate::services::membership;
use crate::state::AppState;

pub async fn on_guild_ban_add(
    state: &AppState,
    guild_id: GuildId,
    banned_user: &User,
) -> Result<()> {
    state.mark_recent_ban(guild_id, banned_user.id);
    let _ = membership::mark_banned(&state.db, guild_id, banned_user.id).await;
    Ok(())
}

pub fn on_invite_create(state: &AppState, data: &serenity::all::InviteCreateEvent) {
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

pub fn on_invite_delete(state: &AppState, data: &serenity::all::InviteDeleteEvent) {
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
