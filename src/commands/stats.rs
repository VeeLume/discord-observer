use anyhow::Result;
use poise::serenity_prelude as serenity;

use crate::commands::{format_member_label, rfc2822_to_unix, send_chunked_embeds};
use crate::repos::MembershipsRepo;
use crate::state::Ctx;

/// `/stats` parent command. All real work happens in subcommands.
#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    subcommands(
        "stats_current",
        "stats_rejoiners",
        "stats_exits",
        "stats_member_balance"
    ),
    rename = "stats"
)]
pub async fn stats(_: Ctx<'_>) -> Result<()> {
    Ok(())
}

/// Top users who rejoined
#[poise::command(slash_command, guild_only, ephemeral, rename = "rejoins")]
pub async fn stats_rejoiners(
    ctx: Ctx<'_>,
    #[description = "Minimum joins (default 2)"] min_joins: Option<i64>,
    #[description = "Max users to show (default 15)"] limit: Option<i64>,
) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let gid = ctx
        .guild_id()
        .expect("guild_only command should always have a guild_id");

    let min_rejoins = min_joins.unwrap_or(2).max(2);
    let limit = limit.unwrap_or(15).clamp(1, 100);

    let repo = MembershipsRepo::new(&ctx.data().db);
    let rows = repo.rejoiners(gid, min_rejoins, limit).await?;

    let min_rejoins_display = min_rejoins.saturating_sub(1);

    if rows.is_empty() {
        ctx.say(format!("No users with ≥{min_rejoins_display} rejoins."))
            .await?;
        return Ok(());
    }

    let mut lines = Vec::with_capacity(rows.len());
    for r in rows {
        let label = format_member_label(
            &r.user_id,
            &r.account_username,
            &r.display_name,
            &r.server_nickname,
        );
        let rejoins = r.rejoin_count.saturating_sub(1);
        lines.push(format!(
            "• {label} — {rejoins} rejoins ({} exits)",
            r.times_left
        ));
    }

    let base_title = format!("Rejoiners (≥{min_rejoins_display} rejoins)");
    let base_title_cont = base_title.clone();

    send_chunked_embeds(
        ctx,
        lines,
        move |desc| {
            serenity::CreateEmbed::new()
                .color(0xe67e22)
                .title(base_title.clone())
                .description(desc)
        },
        move |idx, desc| {
            serenity::CreateEmbed::new()
                .color(0xe67e22)
                .title(format!("{base_title_cont} — cont. #{idx}"))
                .description(desc)
        },
    )
    .await?;

    Ok(())
}

/// Recent exits with left vs banned split.
#[poise::command(slash_command, guild_only, ephemeral, rename = "exits")]
pub async fn stats_exits(
    ctx: Ctx<'_>,
    #[description = "Look back this many days (default 30)"] days: Option<i64>,
    #[description = "Max rows shown (default 20)"] show: Option<i64>,
) -> Result<()> {
    ctx.defer_ephemeral().await?;

    use chrono::{Duration, Utc};

    let gid = ctx
        .guild_id()
        .expect("guild_only command should always have a guild_id");

    let days = days.unwrap_or(30).clamp(1, 365);
    let show = show.unwrap_or(20).clamp(1, 100);

    let repo = MembershipsRepo::new(&ctx.data().db);
    let rows = repo.all_exits(gid, 2000).await?;

    let cutoff = (Utc::now() - Duration::days(days)).timestamp();

    // Parse once, filter, and keep the unix timestamp for display
    struct FilteredExit {
        unix_ts: i64,
        user_id: String,
        banned: bool,
        account_username: Option<String>,
        display_name: Option<String>,
        server_nickname: Option<String>,
    }

    let mut filtered = Vec::new();
    let mut left_count = 0usize;
    let mut banned_count = 0usize;

    for r in rows {
        if let Some(ts) = rfc2822_to_unix(&r.left_at) {
            if ts >= cutoff {
                if r.banned {
                    banned_count += 1;
                } else {
                    left_count += 1;
                }
                filtered.push(FilteredExit {
                    unix_ts: ts,
                    user_id: r.user_id,
                    banned: r.banned,
                    account_username: r.account_username,
                    display_name: r.display_name,
                    server_nickname: r.server_nickname,
                });
            }
        }
    }

    if filtered.is_empty() {
        ctx.say(format!("No exits in the last {} days.", days))
            .await?;
        return Ok(());
    }

    // Sort newest first
    filtered.sort_by(|a, b| b.unix_ts.cmp(&a.unix_ts));

    let total = left_count + banned_count;
    let mut lines = Vec::new();
    lines.push(format!(
        "**Total:** {} (left: {}, banned: {})",
        total, left_count, banned_count
    ));
    lines.push("".into());

    for r in filtered.iter().take(show as usize) {
        let label = format_member_label(
            &r.user_id,
            &r.account_username,
            &r.display_name,
            &r.server_nickname,
        );
        let kind = if r.banned { "**banned**" } else { "left" };
        lines.push(format!("• {label} — {kind} — <t:{}:R>", r.unix_ts));
    }

    let base_title = format!("Exits in last {} days", days);
    let base_title_cont = base_title.clone();

    send_chunked_embeds(
        ctx,
        lines,
        move |desc| {
            serenity::CreateEmbed::new()
                .color(0x95a5a6)
                .title(base_title.clone())
                .description(desc)
        },
        move |idx, desc| {
            serenity::CreateEmbed::new()
                .color(0x95a5a6)
                .title(format!("{base_title_cont} — cont. #{idx}"))
                .description(desc)
        },
    )
    .await?;

    Ok(())
}

/// Snapshot counts: current members, lifetime uniques, exits, bans, server stays.
#[poise::command(slash_command, guild_only, ephemeral, rename = "current")]
pub async fn stats_current(ctx: Ctx<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let gid = ctx
        .guild_id()
        .expect("guild_only command should always have a guild_id");

    let repo = MembershipsRepo::new(&ctx.data().db);
    let s = repo.stats_current(gid).await?;

    let retention = if s.unique_ever > 0 {
        format!(
            "**{}** / {} ({:.1}%)",
            s.current_members,
            s.unique_ever,
            s.current_members as f64 / s.unique_ever as f64 * 100.0
        )
    } else {
        format!("**{}**", s.current_members)
    };

    let banned_text = if s.total_exits > 0 {
        format!(
            "{} ({:.1}%)",
            s.total_banned,
            s.total_banned as f64 / s.total_exits as f64 * 100.0
        )
    } else {
        format!("{}", s.total_banned)
    };

    let embed = serenity::CreateEmbed::new()
        .color(0x3498db)
        .title("Current stats")
        .description("Based on recorded activity only — members who joined or left before the bot was added are not included.")
        .field("Members (of recorded)", retention, true)
        .field("Total stays", format!("{}", s.total_rejoins), true)
        .field("Total exits", format!("{}", s.total_exits), true)
        .field("Banned (of exits)", banned_text, true)
        .field(
            "Left (of exits)",
            format!("{}", s.total_exits.saturating_sub(s.total_banned)),
            true,
        );

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Daily net member delta (joins - leaves) with totals and unique users.
#[poise::command(slash_command, guild_only, ephemeral, rename = "delta")]
pub async fn stats_member_balance(
    ctx: Ctx<'_>,
    #[description = "Days to look back (default 30)"] days: Option<i64>,
) -> Result<()> {
    ctx.defer_ephemeral().await?;

    use chrono::{DateTime, Duration, NaiveDate, Utc};
    use std::collections::{BTreeMap, BTreeSet};

    let gid = ctx
        .guild_id()
        .expect("guild_only command should always have a guild_id");

    let days = days.unwrap_or(30).clamp(1, 365);

    let repo = MembershipsRepo::new(&ctx.data().db);
    let raw = repo.recent_rejoins_raw(gid, 10_000).await?;

    let cutoff = Utc::now() - Duration::days(days);

    // Per-day tallies
    struct Tallies {
        total: i64,
        uniq: BTreeSet<String>,
    }
    impl Default for Tallies {
        fn default() -> Self {
            Self {
                total: 0,
                uniq: BTreeSet::new(),
            }
        }
    }

    let mut joins: BTreeMap<NaiveDate, Tallies> = BTreeMap::new();
    let mut leaves: BTreeMap<NaiveDate, Tallies> = BTreeMap::new();

    for item in raw {
        if let Ok(jdt) = DateTime::parse_from_rfc2822(&item.joined_at) {
            let jutc = jdt.with_timezone(&Utc);
            if jutc >= cutoff {
                let d = jutc.date_naive();
                let e = joins.entry(d).or_default();
                e.total += 1;
                e.uniq.insert(item.user_id.clone());
            }
        }
        if let Some(left) = &item.left_at {
            if let Ok(ldt) = DateTime::parse_from_rfc2822(left) {
                let lutc = ldt.with_timezone(&Utc);
                if lutc >= cutoff {
                    let d = lutc.date_naive();
                    let e = leaves.entry(d).or_default();
                    e.total += 1;
                    e.uniq.insert(item.user_id.clone());
                }
            }
        }
    }

    let all_days: BTreeSet<_> = joins.keys().chain(leaves.keys()).copied().collect();
    if all_days.is_empty() {
        ctx.say(format!("No join/leave activity in the last {} days.", days))
            .await?;
        return Ok(());
    }

    // Window-wide totals (keep unique counts at summary level)
    let (mut j_total, mut j_uniq_all) = (0i64, BTreeSet::<String>::new());
    let (mut l_total, mut l_uniq_all) = (0i64, BTreeSet::<String>::new());

    for (_d, t) in &joins {
        j_total += t.total;
        j_uniq_all.extend(t.uniq.iter().cloned());
    }
    for (_d, t) in &leaves {
        l_total += t.total;
        l_uniq_all.extend(t.uniq.iter().cloned());
    }

    let net_total = j_total - l_total;

    let mut lines = Vec::new();
    lines.push(format!(
        "**{} days:**  net {:+}  |  joins: {} ({} unique)  |  leaves: {} ({} unique)",
        days,
        net_total,
        j_total,
        j_uniq_all.len(),
        l_total,
        l_uniq_all.len()
    ));
    lines.push("".into());

    for d in all_days {
        let jt = joins.get(&d).map(|x| x.total).unwrap_or(0);
        let lt = leaves.get(&d).map(|x| x.total).unwrap_or(0);
        let net = jt - lt;

        lines.push(format!("{d}  +{jt} / -{lt}  (net: {net:+})"));
    }

    let base_title = format!("Member balance (last {} days)", days);
    let base_title_cont = base_title.clone();

    send_chunked_embeds(
        ctx,
        lines,
        move |desc| {
            serenity::CreateEmbed::new()
                .color(0x3498db)
                .title(base_title.clone())
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
