use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::services::{notices as notices_svc, settings as settings_svc};
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

    if notices_svc::get_notice_by_key(db, &key).await?.is_some() {
        ctx.say(format!("A notice with key `{key}` already exists."))
            .await?;
        return Ok(());
    }

    notices_svc::create_notice(db, &key, &title, &body, color, current_only).await?;

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

#[poise::command(slash_command, owners_only, ephemeral, rename = "send")]
pub async fn notice_send(ctx: Ctx<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let db = &ctx.data().db;
    let http = &ctx.serenity_context().http;

    let mut sent = 0usize;
    let mut skipped = 0usize;

    for guild_ref in ctx.serenity_context().cache.guilds() {
        settings_svc::ensure_settings_row(db, guild_ref).await.ok();
        settings_svc::ensure_guild(db, guild_ref).await.ok();
        let gs = settings_svc::get_settings(db, guild_ref)
            .await
            .unwrap_or_default();
        let first_seen_at = settings_svc::get_first_seen_at(db, guild_ref)
            .await
            .ok()
            .flatten();
        let notice_ctx = notices_svc::NoticeContext {
            guild_id: guild_ref,
            settings: gs,
            first_seen_at,
        };
        match notices_svc::send_pending_notices(http, db, &notice_ctx).await {
            Ok(n) => sent += n,
            Err(e) => {
                tracing::warn!(guild_id = %guild_ref, "Failed to send notices: {e}");
                skipped += 1;
            }
        }
    }

    ctx.say(format!("Sent {sent} notices ({skipped} guilds failed)."))
        .await?;
    Ok(())
}

#[poise::command(slash_command, owners_only, ephemeral, rename = "list")]
pub async fn notice_list(ctx: Ctx<'_>) -> Result<()> {
    let db = &ctx.data().db;
    let notices = notices_svc::get_all_notices(db).await?;

    if notices.is_empty() {
        ctx.say("No notices defined.").await?;
        return Ok(());
    }

    let mut lines = Vec::new();
    for n in &notices {
        let sent_count = notices_svc::count_sent(db, &n.key).await.unwrap_or(0);
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

#[poise::command(slash_command, owners_only, ephemeral, rename = "delete")]
pub async fn notice_delete(
    ctx: Ctx<'_>,
    #[description = "Notice key to delete"] key: String,
) -> Result<()> {
    let db = &ctx.data().db;

    if notices_svc::delete_notice(db, &key).await? {
        ctx.say(format!("Deleted notice `{key}`.")).await?;
    } else {
        ctx.say(format!("No notice found with key `{key}`."))
            .await?;
    }
    Ok(())
}
