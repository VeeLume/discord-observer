use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::all::{ChannelId, CreateEmbed, CreateMessage, GuildId};
use serenity::http::Http;

use crate::db::Db;
use crate::repos::{GuildNoticesRepo, GuildSettings};

/// Context available when sending notices to a guild.
pub struct NoticeContext {
    pub guild_id: GuildId,
    pub settings: GuildSettings,
}

/// Send any pending notices for a guild.
///
/// Reads notice definitions from the `notices` table and skips:
/// - notices the guild has already received
/// - `current_only` notices created before the guild's `first_seen_at`
pub async fn send_pending_notices(http: &Http, db: &Db, ctx: &NoticeContext) -> Result<()> {
    if !ctx.settings.notices_enabled {
        return Ok(());
    }

    let target = ctx
        .settings
        .mod_log
        .or(ctx.settings.join_log)
        .or(ctx.settings.leave_log);

    let channel = match target {
        Some(ch) => ch,
        None => return Ok(()),
    };

    let repo = GuildNoticesRepo::new(db);
    let notices = repo.get_all_notices().await?;

    for notice in &notices {
        // Skip current_only notices for guilds that joined after the notice was created
        if notice.current_only != 0 {
            if let Some(first_seen) = &ctx.settings.first_seen_at {
                if notice.created_at < *first_seen {
                    continue;
                }
            }
        }

        if repo.has_been_sent(ctx.guild_id, &notice.key).await? {
            continue;
        }

        let embed = CreateEmbed::new()
            .color(notice.color as u32)
            .title(&notice.title)
            .description(&notice.body);

        if send_to_channel(http, channel, embed).await {
            repo.mark_sent(ctx.guild_id, &notice.key).await?;
        }
    }

    Ok(())
}

async fn send_to_channel(http: &Http, channel: ChannelId, embed: CreateEmbed) -> bool {
    channel
        .send_message(http, CreateMessage::new().embed(embed))
        .await
        .is_ok()
}
