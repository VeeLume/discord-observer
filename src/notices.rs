use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::all::{ChannelId, CreateEmbed, CreateMessage, GuildId};
use serenity::http::Http;

use crate::db::Db;
use crate::repos::{GuildNoticesRepo, GuildSettings};

/// Context available to each notice's condition check.
pub struct NoticeContext {
    pub guild_id: GuildId,
    pub settings: GuildSettings,
}

struct NoticeDefinition {
    key: &'static str,
    check: fn(ctx: &NoticeContext) -> Option<CreateEmbed>,
}

/// Registry of all one-time notices.
const NOTICES: &[NoticeDefinition] = &[NoticeDefinition {
    key: "update-v0.2.0",
    check: check_update_v0_2,
}];

fn invite_url() -> Option<String> {
    std::env::var("CLIENT_ID")
        .ok()
        .map(|id| format!("https://discord.com/oauth2/authorize?client_id={id}"))
}

fn check_update_v0_2(_ctx: &NoticeContext) -> Option<CreateEmbed> {
    let action_needed = match invite_url() {
        Some(url) => format!(
            "\n\n**Action needed:** Invite tracking requires the **Manage Server** permission. \
             To grant it, kick the bot and [re-invite with updated permissions]({url}). \
             Without it, invite info will show as \"Unknown\" in join embeds."
        ),
        None => String::new(),
    };

    Some(
        CreateEmbed::new()
            .color(0x3498db)
            .title("Observer Update")
            .description(format!(
                "**What's new:**\n\
                 • **Invite tracking** — join logs now show which invite link was used and who created it\n\
                 • **Richer join embeds** — account age, new-account warnings, rejoin detection\n\
                 • **Richer leave embeds** — membership duration, join date, stay history\n\
                 • **Improved stats** — retention percentages, colors, faster responses\n\
                 • **Improved user info** — account creation date, compact stay history with invite codes\
                 {action_needed}"
            )),
    )
}

/// Send any pending one-time notices for a guild.
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
        None => return Ok(()), // no channel configured, skip
    };

    let notices_repo = GuildNoticesRepo::new(db);

    for notice in NOTICES {
        if notices_repo.has_been_sent(ctx.guild_id, notice.key).await? {
            continue;
        }
        if let Some(embed) = (notice.check)(ctx) {
            if send_to_channel(http, channel, embed).await {
                notices_repo.mark_sent(ctx.guild_id, notice.key).await?;
            }
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
