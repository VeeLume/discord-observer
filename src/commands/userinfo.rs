use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::commands::{format_stay_lines, send_chunked_embeds};
use crate::services::stats;
use crate::state::Ctx;

/// Slash + context menu for user info / history.
///
/// - Slash: `/userinfo user:<pick member>`
/// - Context menu: right click user → "User information"
#[poise::command(
    slash_command,
    context_menu_command = "User information",
    guild_only,
    ephemeral
)]
pub async fn userinfo(
    ctx: Ctx<'_>,
    #[description = "User to look up"] user: serenity::User,
) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let guild_id = match ctx.guild_id() {
        Some(gid) => gid,
        None => {
            ctx.say("This command can only be used in a guild.").await?;
            return Ok(());
        }
    };

    let rows = stats::history_for_user(&ctx.data().db, guild_id, user.id).await?;

    let title = format!("History for {}", user.tag());
    let thumb_url = user.face();

    if rows.is_empty() {
        let embed = serenity::CreateEmbed::new()
            .color(0x3498db)
            .title(title)
            .thumbnail(thumb_url)
            .description("No server stays recorded for this user.");

        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let lines = format_stay_lines(&rows);
    let stay_count = rows.len();
    let created_unix = user.created_at().unix_timestamp();

    let base_title = title.clone();
    let base_title_cont = base_title.clone();
    let thumb_first = thumb_url.clone();

    send_chunked_embeds(
        ctx,
        lines,
        move |desc| {
            serenity::CreateEmbed::new()
                .color(0x3498db)
                .title(base_title.clone())
                .thumbnail(thumb_first.clone())
                .field("Server stays", stay_count.to_string(), true)
                .field("Account Created", format!("<t:{created_unix}:R>"), true)
                .description(desc)
        },
        move |idx, desc| {
            serenity::CreateEmbed::new()
                .color(0x3498db)
                .title(format!("{base_title_cont} — cont. #{idx}"))
                .description(desc)
        },
    )
    .await?;

    Ok(())
}
