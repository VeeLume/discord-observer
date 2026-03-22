//! Embed building, posting, and recording.
//!
//! Pure builder functions for constructing Discord embeds,
//! plus orchestration for posting to a channel and recording in bot_messages.

use anyhow::Result;
use serenity::all::{
    ChannelId, CreateEmbed, CreateMessage, GuildId, GuildMemberFlags, MessageId, Timestamp, User,
};
use serenity::http::Http;

use crate::db::Db;
use crate::invites::InviteSnapshot;
use crate::services::audit_log::RemovalReason;
use crate::services::messages::{self, MentionRole, MessageType};
use crate::services::search;
use crate::services::stats::{self, StayRow};

const SEVEN_DAYS_SECS: i64 = 7 * 24 * 60 * 60;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn account_created_unix(user: &User) -> i64 {
    user.created_at().unix_timestamp()
}

/// Post an embed to a channel, returning (channel_id, message_id) on success.
pub async fn post_embed(
    http: &Http,
    channel: Option<ChannelId>,
    embed: CreateEmbed,
) -> Option<(ChannelId, MessageId)> {
    let ch = channel?;
    match ch
        .send_message(http, CreateMessage::new().embed(embed))
        .await
    {
        Ok(msg) => Some((ch, msg.id)),
        Err(_) => None,
    }
}

// ── Join embed ───────────────────────────────────────────────────────────────

/// Build, post, and record a join embed.
#[allow(clippy::too_many_arguments)]
pub async fn post_join(
    http: &Http,
    db: &Db,
    guild_id: GuildId,
    member: &serenity::all::Member,
    stay_id: i64,
    invite_used: &Option<(String, InviteSnapshot)>,
    channel: Option<ChannelId>,
) -> Result<()> {
    let user_id = member.user.id;

    // Read data needed for the embed
    let history = stats::history_for_user(db, guild_id, user_id).await?;
    let stay_count = history.len();

    let prev_departure = messages::find_latest_departure(db, guild_id, &user_id.to_string())
        .await
        .ok()
        .flatten();
    let prev_embed_link = prev_departure.as_ref().map(|r| {
        format!(
            "https://discord.com/channels/{}/{}/{}",
            guild_id.get(),
            r.channel_id,
            r.message_id
        )
    });

    // Build embed
    let created_unix = account_created_unix(&member.user);
    let now_unix = Timestamp::now().unix_timestamp();
    let is_new_account = (now_unix - created_unix) < SEVEN_DAYS_SECS;
    let did_rejoin = member.flags.contains(GuildMemberFlags::DID_REJOIN);
    let is_returning = did_rejoin || stay_count > 1;

    let color = if is_new_account {
        0xe67e22 // orange — new account warning takes priority
    } else if is_returning {
        0x3498db // blue — returning member
    } else {
        0x2ecc71 // green — normal first join
    };

    let inviter_id = invite_used
        .as_ref()
        .and_then(|(_, snap)| snap.inviter_id.as_deref());

    let invite_text = match invite_used {
        Some((code, snap)) => match snap.inviter_id.as_deref() {
            Some(id) => format!("`{code}` by <@{id}>"),
            None => format!("`{code}`"),
        },
        None => "Unknown".to_string(),
    };

    let mut embed = CreateEmbed::new()
        .color(color)
        .title("Member joined")
        .description(format!("<@{}> joined the server", user_id.get()))
        .thumbnail(member.user.face())
        .field("Username", &member.user.name, true)
        .field("Account Created", format!("<t:{created_unix}:R>"), true)
        .field("Invite", invite_text, true)
        .timestamp(Timestamp::now());

    // History
    if stay_count > 1 {
        embed = embed.field("Stays", format!("Join #{stay_count}"), true);
    } else if did_rejoin {
        // Discord says they were here before, but we have no prior stays recorded
        embed = embed.field("Stays", "Returning member", true);
    }
    if let Some(link) = prev_embed_link {
        embed = embed.field("Previous", format!("[Leave embed]({link})"), true);
    }

    // Warnings
    if is_new_account {
        embed = embed.field("⚠ New Account", "Created less than 7 days ago", false);
    }

    // Post and record
    if let Some((ch, msg)) = post_embed(http, channel, embed).await {
        let ch_str = ch.get().to_string();
        let msg_str = msg.get().to_string();
        let uid_str = user_id.to_string();

        let mut mentions: Vec<(&str, MentionRole)> = vec![(uid_str.as_str(), MentionRole::Member)];
        if let Some(iid) = inviter_id {
            mentions.push((iid, MentionRole::Inviter));
        }

        messages::record(
            db,
            guild_id,
            &ch_str,
            &msg_str,
            MessageType::Join,
            Some(stay_id),
            prev_departure.as_ref().map(|r| r.message_id.as_str()),
            &mentions,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to record join embed in database for user {}: {:?}",
                uid_str,
                e
            )
        })
        .ok();
    }

    Ok(())
}

// ── Leave embed ──────────────────────────────────────────────────────────────

/// Build, post, and record a leave/kick/ban embed.
#[allow(clippy::too_many_arguments)]
pub async fn post_leave(
    http: &Http,
    db: &Db,
    guild_id: GuildId,
    user: &User,
    reason: &RemovalReason,
    channel: Option<ChannelId>,
) -> Result<()> {
    let user_id_str = user.id.to_string();

    let history = stats::history_for_user(db, guild_id, user.id).await?;
    let stay_count = history.len();
    let closed_stay = history.iter().rev().find(|r| r.left_at.is_some());

    let join_embed_row = messages::find_latest(db, guild_id, MessageType::Join, &user_id_str)
        .await
        .ok()
        .flatten();

    // Determine embed properties from reason
    let (color, action, title, banned, kicked) = match reason {
        RemovalReason::Banned { .. } => (0xe74c3c, "was **banned**", "Member Banned", true, false),
        RemovalReason::Kicked { .. } => (0xe67e22, "was **kicked**", "Member Kicked", false, true),
        RemovalReason::Left => (0x95a5a6, "left the server", "Member Left", false, false),
    };

    let best_name = search::best_display_name(db, guild_id, &user_id_str)
        .await
        .unwrap_or_else(|| {
            user.global_name
                .clone()
                .unwrap_or_else(|| user.name.clone())
        });

    let created_unix = account_created_unix(user);

    // Identity — same order as join: Username, Account Created, Invite
    let mut embed = CreateEmbed::new()
        .color(color)
        .title(title)
        .description(format!("**{best_name}** (<@{}>) {action}", user.id.get()))
        .thumbnail(user.face())
        .field("Username", &user.name, true)
        .field("Account Created", format!("<t:{created_unix}:R>"), true)
        .timestamp(Timestamp::now());

    // Invite info from closed stay
    if let Some(stay) = closed_stay {
        let inviter_ref = match &stay.inviter_id {
            Some(iid) => Some(search::best_user_ref(db, guild_id, iid).await),
            None => None,
        };
        let invite_text = match (&stay.invite_code, inviter_ref) {
            (Some(code), Some(inviter)) => format!("`{code}` by {inviter}"),
            (Some(code), None) => format!("`{code}`"),
            _ => "Unknown".to_string(),
        };
        embed = embed.field("Invite", invite_text, true);
    }

    // Departure context (leave-only)
    match reason {
        RemovalReason::Banned {
            moderator_id,
            reason: r,
        }
        | RemovalReason::Kicked {
            moderator_id,
            reason: r,
        } => {
            if let Some(mod_id) = moderator_id {
                embed = embed.field("By", format!("<@{}>", mod_id.get()), true);
            }
            if let Some(r) = r {
                embed = embed.field("Reason", r, false);
            }
        }
        RemovalReason::Left => {}
    }

    if let Some(stay) = closed_stay {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(&stay.joined_at) {
            embed = embed.field("Joined", format!("<t:{}:R>", dt.timestamp()), true);
        }
    }

    // History — same order as join: Stays, Previous
    if stay_count > 1 {
        embed = embed.field("Stays", format!("{stay_count} total"), true);
    }

    if let Some(ref row) = join_embed_row {
        let link = format!(
            "https://discord.com/channels/{}/{}/{}",
            guild_id.get(),
            row.channel_id,
            row.message_id
        );
        embed = embed.field("Previous", format!("[Join embed]({link})"), true);
    }

    // Post and record
    let msg_type = if banned {
        MessageType::Ban
    } else if kicked {
        MessageType::Kick
    } else {
        MessageType::Leave
    };

    let stay_id = closed_stay.map(|s| s.id);
    let moderator_id_str: Option<String> = match reason {
        RemovalReason::Banned {
            moderator_id: Some(id),
            ..
        }
        | RemovalReason::Kicked {
            moderator_id: Some(id),
            ..
        } => Some(id.get().to_string()),
        _ => None,
    };

    if let Some((ch, msg)) = post_embed(http, channel, embed).await {
        let ch_str = ch.get().to_string();
        let msg_str = msg.get().to_string();

        let mut mentions: Vec<(&str, MentionRole)> =
            vec![(user_id_str.as_str(), MentionRole::Member)];
        if let Some(ref mid) = moderator_id_str {
            mentions.push((mid.as_str(), MentionRole::Moderator));
        }

        messages::record(
            db,
            guild_id,
            &ch_str,
            &msg_str,
            msg_type,
            stay_id,
            join_embed_row.as_ref().map(|r| r.message_id.as_str()),
            &mentions,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to record leave embed in database for user {}: {:?}",
                user_id_str,
                e
            )
        })
        .ok();
    }

    Ok(())
}
