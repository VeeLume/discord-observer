use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::permissions;
use crate::state::Ctx;

/// `/botstats` parent command — owner-only, anonymized cross-guild statistics.
#[poise::command(
    slash_command,
    ephemeral,
    owners_only,
    subcommands("botstats_features")
)]
pub async fn botstats(_: Ctx<'_>) -> Result<()> {
    Ok(())
}

/// Anonymized feature availability across all guilds.
#[poise::command(slash_command, ephemeral, owners_only, rename = "features")]
pub async fn botstats_features(ctx: Ctx<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let bot_id = ctx.framework().bot_id;
    let cache = &ctx.serenity_context().cache;

    // Collect permissions from all cached guilds
    let mut guild_count = 0u64;
    let mut feature_counts: Vec<(&'static str, u64)> = Vec::new();

    // We need to iterate all guilds, so collect guild IDs first to avoid holding cache lock
    let guild_ids: Vec<serenity::GuildId> = cache.guilds();
    let total_guilds = guild_ids.len() as u64;

    // Initialize feature counters from the first check
    let mut initialized = false;

    for gid in &guild_ids {
        let perms = match permissions::bot_permissions_cached(ctx.serenity_context(), *gid, bot_id)
        {
            Some(p) => p,
            None => continue,
        };

        guild_count += 1;
        let features = permissions::check_features(perms);

        if !initialized {
            feature_counts = features
                .iter()
                .map(|f| (f.name, if f.available { 1 } else { 0 }))
                .collect();
            initialized = true;
        } else {
            for (i, f) in features.iter().enumerate() {
                if f.available {
                    feature_counts[i].1 += 1;
                }
            }
        }
    }

    if guild_count == 0 {
        ctx.say("No guilds with cached permissions found.").await?;
        return Ok(());
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "**Guilds analyzed:** {guild_count} / {total_guilds} total\n"
    ));

    for (name, count) in &feature_counts {
        let pct = (*count as f64 / guild_count as f64) * 100.0;
        let bar = progress_bar(*count, guild_count);
        lines.push(format!(
            "**{name}**\n{bar} {count}/{guild_count} ({pct:.0}%)"
        ));
    }

    let embed = serenity::CreateEmbed::new()
        .color(0x3498db)
        .title("Feature Availability (anonymized)")
        .description(lines.join("\n\n"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Simple text progress bar.
fn progress_bar(value: u64, max: u64) -> String {
    let width = 10;
    let filled = if max > 0 {
        ((value as f64 / max as f64) * width as f64).round() as usize
    } else {
        0
    };
    let empty = width - filled;
    format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(empty))
}
