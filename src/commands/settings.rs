use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::repos::{GuildNoticesRepo, GuildSettings, GuildSettingsRepo};
use crate::state::Ctx;

/// `/settings` parent command, like in your other bot.
/// All real work happens in the subcommands.
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
        "settings_show"
    )
)]
pub async fn settings(_: Ctx<'_>) -> Result<()> {
    // Parent does nothing by itself.
    Ok(())
}

/// Set or clear the **join log** channel.
///
/// Usage examples:
/// - `/settings join-log` (no args) → uses the current channel
/// - `/settings join-log channel:#some-channel`
/// - `/settings join-log clear:true`
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
    let repo = GuildSettingsRepo::new(db);
    repo.ensure_row(&gid).await?;

    if clear.unwrap_or(false) {
        repo.set_column(&gid, "join_log_channel_id", None).await?;
        ctx.say("✅ Cleared **join log** channel.").await?;
    } else {
        let ch = channel.unwrap_or_else(|| ctx.channel_id());
        repo.set_column(&gid, "join_log_channel_id", Some(ch))
            .await?;
        ctx.say(format!("✅ **Join log** channel set to <#{}>.", ch.get()))
            .await?;
    }

    Ok(())
}

/// Set or clear the **leave log** channel.
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
    let repo = GuildSettingsRepo::new(db);
    repo.ensure_row(&gid).await?;

    if clear.unwrap_or(false) {
        repo.set_column(&gid, "leave_log_channel_id", None).await?;
        ctx.say("✅ Cleared **leave log** channel.").await?;
    } else {
        let ch = channel.unwrap_or_else(|| ctx.channel_id());
        repo.set_column(&gid, "leave_log_channel_id", Some(ch))
            .await?;
        ctx.say(format!("✅ **Leave log** channel set to <#{}>.", ch.get()))
            .await?;
    }

    Ok(())
}

/// Set or clear the **moderation log** channel.
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
    let repo = GuildSettingsRepo::new(db);
    repo.ensure_row(&gid).await?;

    if clear.unwrap_or(false) {
        repo.set_column(&gid, "mod_log_channel_id", None).await?;
        ctx.say("✅ Cleared **moderation log** channel.").await?;
    } else {
        let ch = channel.unwrap_or_else(|| ctx.channel_id());
        repo.set_column(&gid, "mod_log_channel_id", Some(ch))
            .await?;
        ctx.say(format!(
            "✅ **Moderation log** channel set to <#{}>.",
            ch.get()
        ))
        .await?;
    }

    Ok(())
}

/// Show the current log channel configuration for this server.
#[poise::command(slash_command, guild_only, ephemeral, rename = "show")]
pub async fn settings_show(ctx: Ctx<'_>) -> Result<()> {
    let gid = match ctx.guild_id() {
        Some(g) => g,
        None => {
            ctx.say("Use this command in a server channel.").await?;
            return Ok(());
        }
    };

    let db = &ctx.data().db;
    let repo = GuildSettingsRepo::new(db);

    let current: GuildSettings = repo.get(&gid).await?;

    let fmt = |ch: Option<serenity::ChannelId>| {
        ch.map(|c| format!("<#{}>", c.get()))
            .unwrap_or_else(|| "— not set —".to_string())
    };

    let join = fmt(current.join_log);
    let leave = fmt(current.leave_log);
    let modu = fmt(current.mod_log);

    let notices = if current.notices_enabled {
        "Enabled"
    } else {
        "Disabled"
    };

    let msg = format!(
        "**Current settings for this server**\n\
         • **Join log:** {join}\n\
         • **Leave log:** {leave}\n\
         • **Moderation log:** {modu}\n\
         • **Notices:** {notices}"
    );

    ctx.say(msg).await?;
    Ok(())
}

/// Enable or disable automatic notices (patch notes, update alerts).
#[poise::command(slash_command, guild_only, ephemeral, rename = "notices")]
pub async fn settings_notices(
    ctx: Ctx<'_>,
    #[description = "Enable or disable notices"] enabled: bool,
) -> Result<()> {
    let gid = ctx.guild_id().expect("guild_only");
    let db = &ctx.data().db;
    let repo = GuildSettingsRepo::new(db);
    repo.ensure_row(&gid).await?;
    repo.set_notices_enabled(&gid, enabled).await?;

    let state = if enabled { "enabled" } else { "disabled" };
    ctx.say(format!("Notices are now **{state}**.")).await?;
    Ok(())
}

/// Show the history of notices sent to this server.
#[poise::command(slash_command, guild_only, ephemeral, rename = "notices-history")]
pub async fn settings_notices_history(ctx: Ctx<'_>) -> Result<()> {
    let gid = ctx.guild_id().expect("guild_only");
    let db = &ctx.data().db;
    let notices_repo = GuildNoticesRepo::new(db);
    let history = notices_repo.get_history(gid).await?;

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
