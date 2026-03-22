//! Mention editing when users depart.
//!
//! When a user leaves, embeds that reference them via `<@user_id>` are edited
//! to replace the broken mention with their display name. Only specific parts
//! are targeted based on the embed type.

use std::sync::Arc;

use serenity::all::{ChannelId, CreateEmbed, EditMessage, Embed, MessageId};
use serenity::http::Http;

use crate::db::Db;
use crate::services::messages::{self, MessageType};

/// Edit all embeds that reference a departing user.
///
/// - **Join embeds**: replace `<@id>` in description and Invite field
/// - **Departure embeds**: replace `<@id>` in Invite and By fields only
///
/// Spawns a background task so the graduated delays (up to 2s per embed)
/// don't block the caller. Edits are best-effort — errors are logged.
pub fn edit_departed_mentions(
    http: &Arc<Http>,
    db: &Db,
    guild_id: serenity::all::GuildId,
    user_id: &str,
    display_name: &str,
) {
    let http = Arc::clone(http);
    let db = db.clone();
    let user_id = user_id.to_string();
    let display_name = display_name.to_string();

    tokio::spawn(async move {
        if let Err(e) =
            edit_departed_mentions_inner(&http, &db, guild_id, &user_id, &display_name).await
        {
            tracing::warn!(guild_id = %guild_id, user_id, "Mention editing failed: {e:#}");
        }
    });
}

async fn edit_departed_mentions_inner(
    http: &Http,
    db: &Db,
    guild_id: serenity::all::GuildId,
    user_id: &str,
    display_name: &str,
) -> anyhow::Result<()> {
    // Returned newest-first from the query.
    let referencing = messages::find_referencing_user(db, guild_id, user_id).await?;

    let pattern = format!("<@{user_id}>");
    let replacement = format!("*{display_name}*");

    for (i, embed_ref) in referencing.iter().enumerate() {
        let Ok(ch_id) = embed_ref.channel_id.parse::<u64>() else {
            continue;
        };
        let Ok(msg_id) = embed_ref.message_id.parse::<u64>() else {
            continue;
        };
        let channel_id = ChannelId::new(ch_id);
        let message_id = MessageId::new(msg_id);

        let embed = if embed_ref.message_type == MessageType::Join {
            edit_join_embed(http, channel_id, message_id, &pattern, &replacement).await
        } else {
            edit_departure_embed(http, channel_id, message_id, &pattern, &replacement).await
        };

        if let Some(embed) = embed {
            channel_id
                .edit_message(
                    http,
                    message_id,
                    EditMessage::new().embed(CreateEmbed::from(embed)),
                )
                .await
                .map_err(|e| {
                    tracing::error!(
                        channel_id = %channel_id,
                        message_id = %message_id,
                        error = ?e,
                        "Failed to edit embed for departed user"
                    )
                })
                .ok();

            // Graduated delay: 300ms for the first few, scaling up for older embeds.
            let delay_ms = match i {
                0..=4 => 300,
                5..=14 => 800,
                _ => 2000,
            };
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
    }

    Ok(())
}

/// Join embeds: replace mention in description and Invite field.
async fn edit_join_embed(
    http: &Http,
    channel_id: ChannelId,
    message_id: MessageId,
    pattern: &str,
    replacement: &str,
) -> Option<Embed> {
    let Ok(original) = channel_id.message(http, message_id).await else {
        return None;
    };
    let Some(mut embed) = original.embeds.first().cloned() else {
        return None;
    };

    if let Some(ref mut d) = embed.description {
        *d = d.replace(pattern, replacement);
    }
    for field in &mut embed.fields {
        if field.name == "Invite" {
            field.value = field.value.replace(pattern, replacement);
        }
    }

    Some(embed)
}

/// Departure embeds: replace mention in Invite and By fields only.
async fn edit_departure_embed(
    http: &Http,
    channel_id: ChannelId,
    message_id: MessageId,
    pattern: &str,
    replacement: &str,
) -> Option<Embed> {
    let Ok(original) = channel_id.message(http, message_id).await else {
        return None;
    };
    let Some(mut embed) = original.embeds.first().cloned() else {
        return None;
    };

    for field in &mut embed.fields {
        if field.name == "Invite" || field.name == "By" {
            field.value = field.value.replace(pattern, replacement);
        }
    }

    Some(embed)
}
