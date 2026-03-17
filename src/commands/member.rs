use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::commands::{format_member_label, format_stay_lines, send_chunked_embeds};
use crate::repos::MembershipsRepo;
use crate::state::Ctx;

/// Autocomplete by nickname/account username; returns `AutocompleteChoice<label, value=user_id>`
pub async fn ac_member(ctx: Ctx<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    let Some(gid) = ctx.guild_id() else {
        return Vec::new();
    };

    let repo = MembershipsRepo::new(&ctx.data().db);
    let Ok(rows) = repo.search_user_summaries_prefix(gid, partial, 25).await else {
        return Vec::new();
    };

    rows.into_iter()
        .map(|r| {
            let label = format_member_label(&r.user_id, &r.account_username, &r.server_username);
            serenity::AutocompleteChoice::new(label, r.user_id)
        })
        .collect()
}

/// Parent command: `/member`
#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    subcommands("member_history"),
    rename = "member"
)]
pub async fn member(_: Ctx<'_>) -> Result<()> {
    Ok(())
}

/// Show the membership history for a user picked via autocomplete.
#[poise::command(slash_command, guild_only, ephemeral, rename = "history")]
pub async fn member_history(
    ctx: Ctx<'_>,
    #[description = "Pick a user by name"]
    #[autocomplete = "ac_member"]
    user_id: String,
) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let Some(guild_id) = ctx.guild_id() else {
        ctx.say("This command can only be used in a guild.").await?;
        return Ok(());
    };

    let uid = match user_id.parse::<u64>() {
        Ok(raw) => serenity::all::UserId::new(raw),
        Err(_) => {
            ctx.say("Couldn't parse that user id. Please pick from the autocomplete list.")
                .await?;
            return Ok(());
        }
    };

    let repo = MembershipsRepo::new(&ctx.data().db);
    let rows = repo.history_for_user(guild_id, uid).await?;

    // Try to fetch user for avatar (may fail for deleted accounts)
    let user_info = ctx.http().get_user(uid).await.ok();
    let thumb_url = user_info.as_ref().map(|u| u.face());
    let display_name = user_info
        .as_ref()
        .map(|u| u.tag())
        .unwrap_or_else(|| format!("{uid}"));

    let title = format!("History for {display_name}");

    if rows.is_empty() {
        let mut embed = serenity::CreateEmbed::new()
            .color(0x3498db)
            .title(title)
            .description("No membership history found for this user.");
        if let Some(url) = &thumb_url {
            embed = embed.thumbnail(url);
        }
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let lines = format_stay_lines(&rows);
    let stay_count = rows.len();
    let thumb_first = thumb_url.clone();
    let title_cont = title.clone();

    send_chunked_embeds(
        ctx,
        lines,
        move |first_desc| {
            let mut embed = serenity::CreateEmbed::new()
                .color(0x3498db)
                .title(title)
                .field("Server stays", stay_count.to_string(), true)
                .description(first_desc);
            if let Some(url) = &thumb_first {
                embed = embed.thumbnail(url);
            }
            embed
        },
        move |index, cont_desc| {
            serenity::CreateEmbed::new()
                .color(0x3498db)
                .title(format!("{title_cont} — cont. #{index}"))
                .description(cont_desc)
        },
    )
    .await?;

    Ok(())
}
