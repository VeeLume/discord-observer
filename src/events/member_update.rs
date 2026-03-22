use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::services::{membership, search};
use crate::state::AppState;

pub async fn on_member_update(
    state: &AppState,
    event: &serenity::GuildMemberUpdateEvent,
) -> Result<()> {
    let changed = membership::record_name_change(
        &state.db,
        event.guild_id,
        event.user.id,
        &event.user.name,
        event.user.global_name.as_deref(),
        event.nick.as_deref(),
    )
    .await?;

    // FTS update happens inside the transaction for name changes,
    // but if names didn't change we still refresh FTS in case it's stale
    if !changed {
        search::upsert_fts_row(&state.db, event.guild_id, &event.user.id.to_string()).await?;
    }

    Ok(())
}
