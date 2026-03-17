use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::notices::{self, NoticeContext};
use crate::repos::{GuildNoticesRepo, GuildSettingsRepo};
use crate::state::Ctx;

/// Manage bot-wide notices (owner-only).
#[poise::command(
    slash_command,
    owners_only,
    ephemeral,
    subcommands("notice_create", "notice_send", "notice_list", "notice_delete")
)]
pub async fn notice(_: Ctx<'_>) -> Result<()> {
    Ok(())
}

/// Create a new notice to send to guilds.
///
/// Attach a `.md` or `.txt` file whose contents become the embed body.
#[poise::command(slash_command, owners_only, ephemeral, rename = "create")]
pub async fn notice_create(
    ctx: Ctx<'_>,
    #[description = "Short unique key (e.g. \"v0.2.1\")"] key: String,
    #[description = "Notice title"] title: String,
    #[description = "Markdown file with the notice body"] file: serenity::Attachment,
    #[description = "Embed color as decimal (default: 3447003 / blue)"] color: Option<i64>,
    #[description = "Only send to guilds currently using the bot (default: true)"]
    current_only: Option<bool>,
) -> Result<()> {
    let color = color.unwrap_or(0x3498db);
    let current_only = current_only.unwrap_or(true);

    let body = file
        .download()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to download attachment: {e}"))?;
    let body =
        String::from_utf8(body).map_err(|_| anyhow::anyhow!("Attachment is not valid UTF-8"))?;

    let db = &ctx.data().db;
    let repo = GuildNoticesRepo::new(db);

    // Check for duplicate key
    if repo.get_notice_by_key(&key).await?.is_some() {
        ctx.say(format!("A notice with key `{key}` already exists."))
            .await?;
        return Ok(());
    }

    repo.create_notice(&key, &title, &body, color, current_only)
        .await?;

    let mode = if current_only {
        "current guilds only"
    } else {
        "all guilds (including future)"
    };
    ctx.say(format!(
        "Created notice `{key}` ({mode}).\nUse `/notice send` to distribute it now, or it will be sent on next restart."
    ))
    .await?;
    Ok(())
}

/// Send all pending notices to all connected guilds now.
#[poise::command(slash_command, owners_only, ephemeral, rename = "send")]
pub async fn notice_send(ctx: Ctx<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let db = &ctx.data().db;
    let grepo = GuildSettingsRepo::new(db);
    let http = &ctx.serenity_context().http;

    let mut sent = 0u32;
    let mut skipped = 0u32;

    // Iterate all guilds the bot can see
    for guild_ref in ctx.serenity_context().cache.guilds() {
        grepo.ensure_row(&guild_ref).await.ok();
        let settings = grepo.get(&guild_ref).await.unwrap_or_default();
        let notice_ctx = NoticeContext {
            guild_id: guild_ref,
            settings,
        };
        match notices::send_pending_notices(http, db, &notice_ctx).await {
            Ok(()) => sent += 1,
            Err(e) => {
                tracing::warn!(guild_id = %guild_ref, "Failed to send notices: {e}");
                skipped += 1;
            }
        }
    }

    ctx.say(format!("Processed {sent} guilds ({skipped} failed)."))
        .await?;
    Ok(())
}

/// List all notice definitions.
#[poise::command(slash_command, owners_only, ephemeral, rename = "list")]
pub async fn notice_list(ctx: Ctx<'_>) -> Result<()> {
    let db = &ctx.data().db;
    let repo = GuildNoticesRepo::new(db);
    let notices = repo.get_all_notices().await?;

    if notices.is_empty() {
        ctx.say("No notices defined.").await?;
        return Ok(());
    }

    let mut lines = Vec::new();
    for n in &notices {
        let sent_count = repo.count_sent(&n.key).await.unwrap_or(0);
        let mode = if n.current_only != 0 {
            "current only"
        } else {
            "all guilds"
        };
        let ts = chrono::DateTime::parse_from_rfc2822(&n.created_at)
            .map(|dt| format!("<t:{}:R>", dt.timestamp()))
            .unwrap_or_else(|_| n.created_at.clone());
        lines.push(format!(
            "- `{}` — **{}** ({mode}, sent to {sent_count} guilds, created {ts})",
            n.key, n.title
        ));
    }

    let embed = serenity::CreateEmbed::new()
        .color(0x3498db)
        .title("Notices")
        .description(lines.join("\n"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Delete a notice definition.
#[poise::command(slash_command, owners_only, ephemeral, rename = "delete")]
pub async fn notice_delete(
    ctx: Ctx<'_>,
    #[description = "Notice key to delete"] key: String,
) -> Result<()> {
    let db = &ctx.data().db;
    let repo = GuildNoticesRepo::new(db);

    if repo.delete_notice(&key).await? {
        ctx.say(format!("Deleted notice `{key}`.")).await?;
    } else {
        ctx.say(format!("No notice found with key `{key}`."))
            .await?;
    }
    Ok(())
}
