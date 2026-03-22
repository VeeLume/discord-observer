use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use serenity::all::{GuildId, RichInvite};
use serenity::http::Http;

#[derive(Debug, Clone)]
pub struct InviteSnapshot {
    pub uses: u64,
    pub max_uses: u64,
    pub inviter_id: Option<String>,
    pub inviter_name: Option<String>,
}

/// An invite tracked through its full lifecycle via gateway events.
#[derive(Debug, Clone)]
pub struct TrackedInvite {
    pub snapshot: InviteSnapshot,
    pub created_at: Instant,
    /// Set when an InviteDelete event is received. `None` while the invite is active.
    pub deleted_at: Option<Instant>,
}

/// Fetch all invites for a guild (requires Manage Guild + Manage Channels)
/// and return them as TrackedInvite entries with `created_at = now`.
pub async fn fetch_invites_map(
    http: &Http,
    guild_id: GuildId,
) -> Result<HashMap<String, TrackedInvite>> {
    let invites: Vec<RichInvite> = guild_id.invites(http).await?;
    let now = Instant::now();
    Ok(invites
        .into_iter()
        .map(|i| {
            let inviter_id = i.inviter.as_ref().map(|u| u.id.to_string());
            let inviter_name = i.inviter.as_ref().map(|u| u.display_name().to_string());
            (
                i.code,
                TrackedInvite {
                    snapshot: InviteSnapshot {
                        uses: i.uses,
                        max_uses: i.max_uses as u64,
                        inviter_id,
                        inviter_name,
                    },
                    created_at: now,
                    deleted_at: None,
                },
            )
        })
        .collect())
}
