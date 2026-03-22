use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::permissions;
use crate::services::{notices as notices_svc, settings as settings_svc};
use crate::state::Ctx;

/// `/settings` parent command.
#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    default_member_permissions = "MANAGE_GUILD",
    subcommands(
        "settings_join_log",
        "settings_leave_log",
        "settings_mod_log",
        "settings_notices",
        "settings_notices_history",
        "settings_show",
        "settings_features"
    )
)]
pub async fn settings(_: Ctx<'_>) -> Result<()> {
    Ok(())
}

#[poise::command(slash_command, guild_only, ephemeral, rename = "join-log")]
pub async fn settings_join_log(
    ctx: Ctx<'_>,
    #[description = "Channel to use for join logs (defaults to this channel)"] channel: Option<
        serenity::ChannelId,
    >,
    #[description = "Clear the join log channel instead of setting it"] clear: Option<bool>,
) -> Result<()> {
    let gid = match ctx.guild_id() {
        Some(g) => g,
        None => {
            ctx.say("Use this command in a server channel.").await?;
            return Ok(());
        }
    };

    let db = &ctx.data().db;
    settings_svc::ensure_settings_row(db, gid).await?;

    if clear.unwrap_or(false) {
        settings_svc::set_column(db, gid, "join_log_channel_id", None).await?;
        ctx.say("Cleared **join log** channel.").await?;
    } else {
        let ch = channel.unwrap_or_else(|| ctx.channel_id());
        settings_svc::set_column(db, gid, "join_log_channel_id", Some(ch)).await?;
        ctx.say(format!("**Join log** channel set to <#{}>.", ch.get()))
            .await?;
    }

    Ok(())
}

#[poise::command(slash_command, guild_only, ephemeral, rename = "leave-log")]
pub async fn settings_leave_log(
    ctx: Ctx<'_>,
    #[description = "Channel to use for leave logs (defaults to this channel)"] channel: Option<
        serenity::ChannelId,
    >,
    #[description = "Clear the leave log channel instead of setting it"] clear: Option<bool>,
) -> Result<()> {
    let gid = match ctx.guild_id() {
        Some(g) => g,
        None => {
            ctx.say("Use this command in a server channel.").await?;
            return Ok(());
        }
    };

    let db = &ctx.data().db;
    settings_svc::ensure_settings_row(db, gid).await?;

    if clear.unwrap_or(false) {
        settings_svc::set_column(db, gid, "leave_log_channel_id", None).await?;
        ctx.say("Cleared **leave log** channel.").await?;
    } else {
        let ch = channel.unwrap_or_else(|| ctx.channel_id());
        settings_svc::set_column(db, gid, "leave_log_channel_id", Some(ch)).await?;
        ctx.say(format!("**Leave log** channel set to <#{}>.", ch.get()))
            .await?;
    }

    Ok(())
}

#[poise::command(slash_command, guild_only, ephemeral, rename = "mod-log")]
pub async fn settings_mod_log(
    ctx: Ctx<'_>,
    #[description = "Channel to use for moderation logs (defaults to this channel)"]
    channel: Option<serenity::ChannelId>,
    #[description = "Clear the moderation log channel instead of setting it"] clear: Option<bool>,
) -> Result<()> {
    let gid = match ctx.guild_id() {
        Some(g) => g,
        None => {
            ctx.say("Use this command in a server channel.").await?;
            return Ok(());
        }
    };

    let db = &ctx.data().db;
    settings_svc::ensure_settings_row(db, gid).await?;

    if clear.unwrap_or(false) {
        settings_svc::set_column(db, gid, "mod_log_channel_id", None).await?;
        ctx.say("Cleared **moderation log** channel.").await?;
    } else {
        let ch = channel.unwrap_or_else(|| ctx.channel_id());
        settings_svc::set_column(db, gid, "mod_log_channel_id", Some(ch)).await?;
        ctx.say(format!(
            "**Moderation log** channel set to <#{}>.",
            ch.get()
        ))
        .await?;
    }

    Ok(())
}

#[poise::command(slash_command, guild_only, ephemeral, rename = "show")]
pub async fn settings_show(ctx: Ctx<'_>) -> Result<()> {
    let gid = match ctx.guild_id() {
        Some(g) => g,
        None => {
            ctx.say("Use this command in a server channel.").await?;
            return Ok(());
        }
    };

    let current = settings_svc::get_settings(&ctx.data().db, gid).await?;

    let fmt = |ch: Option<serenity::ChannelId>| {
        ch.map(|c| format!("<#{}>", c.get()))
            .unwrap_or_else(|| "— not set —".to_string())
    };

    let notices = if current.notices_enabled {
        "Enabled"
    } else {
        "Disabled"
    };

    let msg = format!(
        "**Current settings for this server**\n\
         • **Join log:** {}\n\
         • **Leave log:** {}\n\
         • **Moderation log:** {}\n\
         • **Notices:** {notices}",
        fmt(current.join_log),
        fmt(current.leave_log),
        fmt(current.mod_log),
    );

    ctx.say(msg).await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, ephemeral, rename = "notices")]
pub async fn settings_notices(
    ctx: Ctx<'_>,
    #[description = "Enable or disable notices"] enabled: bool,
) -> Result<()> {
    let gid = ctx.guild_id().expect("guild_only");
    let db = &ctx.data().db;
    settings_svc::ensure_settings_row(db, gid).await?;
    settings_svc::set_notices_enabled(db, gid, enabled).await?;

    let state = if enabled { "enabled" } else { "disabled" };
    ctx.say(format!("Notices are now **{state}**.")).await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, ephemeral, rename = "notices-history")]
pub async fn settings_notices_history(ctx: Ctx<'_>) -> Result<()> {
    let gid = ctx.guild_id().expect("guild_only");
    let history = notices_svc::get_history(&ctx.data().db, gid).await?;

    if history.is_empty() {
        ctx.say("No notices have been sent to this server yet.")
            .await?;
        return Ok(());
    }

    let lines: Vec<String> = history
        .iter()
        .map(|n| {
            let ts = chrono::DateTime::parse_from_rfc2822(&n.sent_at)
                .map(|dt| format!("<t:{}:R>", dt.timestamp()))
                .unwrap_or_else(|_| n.sent_at.clone());
            format!("- `{}` sent {}", n.notice_key, ts)
        })
        .collect();

    let embed = serenity::CreateEmbed::new()
        .color(0x3498db)
        .title("Notice History")
        .description(lines.join("\n"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, ephemeral, rename = "features")]
pub async fn settings_features(ctx: Ctx<'_>) -> Result<()> {
    let guild_id = ctx.guild_id().expect("guild_only");
    let bot_id = ctx.framework().bot_id;

    let perms = match permissions::bot_permissions_cached(ctx.serenity_context(), guild_id, bot_id)
    {
        Some(p) => p,
        None => {
            ctx.say("Could not determine bot permissions (guild not cached).")
                .await?;
            return Ok(());
        }
    };

    let features = permissions::check_features(perms);
    let mut embeds = Vec::with_capacity(features.len());

    for f in &features {
        let color = if f.available { 0x2ecc71 } else { 0xe74c3c };
        let perm_lines: Vec<String> = f
            .permissions
            .iter()
            .map(|p| {
                let icon = if p.granted { "\u{2705}" } else { "\u{274c}" };
                format!("{icon} {}", p.label)
            })
            .collect();

        embeds.push(
            serenity::CreateEmbed::new()
                .color(color)
                .title(f.name)
                .description(format!("{}\n\n{}", f.description, perm_lines.join("\n"))),
        );
    }

    let reply = poise::CreateReply {
        embeds,
        ..Default::default()
    };
    ctx.send(reply).await?;
    Ok(())
}
