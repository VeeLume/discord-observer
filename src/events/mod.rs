mod invites;
mod join;
mod leave;
mod member_update;
pub mod sync;

use std::sync::Arc;

use anyhow::Result;
use poise::FrameworkContext;
use poise::serenity_prelude as serenity;
use serenity::prelude::Context;

use crate::state::AppState;

/// Milliseconds since bot boot — for comparing event timing in logs.
fn ms_since(boot: std::time::Instant) -> u64 {
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
        Ready { data_about_bot, .. } => sync::handle_ready(ctx, state, data_about_bot).await?,

        GuildMemberAddition { new_member } => {
            tracing::trace!(
                t,
                guild_id = %new_member.guild_id,
                user_id = %new_member.user.id,
                username = %new_member.user.name,
                "GuildMemberAddition"
            );
            join::on_join(ctx, state, new_member).await?;
        }

        GuildMemberUpdate { event, .. } => {
            tracing::trace!(
                t,
                guild_id = %event.guild_id,
                user_id = %event.user.id,
                nick = ?event.nick,
                "GuildMemberUpdate"
            );
            member_update::on_member_update(state, event).await?;
        }

        GuildMemberRemoval { guild_id, user, .. } => {
            tracing::trace!(
                t,
                guild_id = %guild_id,
                user_id = %user.id,
                username = %user.name,
                "GuildMemberRemoval"
            );
            leave::on_leave(&ctx.http, state, guild_id, user).await?;
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
            invites::on_guild_ban_add(state, *guild_id, banned_user).await?;
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
            invites::on_invite_create(state, data);
        }

        InviteDelete { data } => {
            tracing::trace!(
                t,
                code = %data.code,
                guild_id = ?data.guild_id,
                "InviteDelete"
            );
            invites::on_invite_delete(state, data);
        }

        _ => {}
    }
    Ok(())
}
