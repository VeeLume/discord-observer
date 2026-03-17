use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use dashmap::DashMap;
use poise::serenity_prelude as serenity;
use serenity::all::{GuildId, UserId};
use serenity::http::Http;

use crate::db::Db;
use crate::invites::{self, InviteSnapshot, TrackedInvite};

pub type Ctx<'a> = poise::Context<'a, std::sync::Arc<AppState>, anyhow::Error>;

/// AppState: holds Db and all in-memory caches.
/// No SQL here; only quick state helpers.
pub struct AppState {
    pub db: Db,

    /// Monotonic clock reference for comparing event timestamps.
    pub boot: Instant,

    /// invite_cache[guild_id][code] = tracked invite with lifecycle timestamps
    pub invite_cache: DashMap<GuildId, HashMap<String, TrackedInvite>>,

    /// Recent bans for leave classification
    pub recent_bans: DashMap<GuildId, DashMap<UserId, i64>>,
}

impl AppState {
    pub async fn new(db_url: &str) -> Result<Arc<Self>, anyhow::Error> {
        let db = crate::db::Db::connect(db_url).await?;
        Ok(Arc::new(Self {
            db,
            boot: Instant::now(),
            invite_cache: DashMap::new(),
            recent_bans: DashMap::new(),
        }))
    }

    /// Detect which invite was used for a join.
    ///
    /// Primary: find an invite deleted within `window` of now (timestamp-based).
    /// Fallback: fetch invites from API and compare uses counts against cache.
    pub async fn detect_invite_used(
        &self,
        http: &Http,
        guild_id: GuildId,
    ) -> Option<(String, InviteSnapshot)> {
        let now = Instant::now();
        let window = Duration::from_secs(5);

        // ── Primary: timestamp-based matching on recently deleted invites ──
        if let Some(cache) = self.invite_cache.get(&guild_id) {
            // Find invites deleted within the window
            let mut candidates: Vec<_> = cache
                .iter()
                .filter_map(|(code, tracked)| {
                    tracked.deleted_at.and_then(|del| {
                        let age = now.duration_since(del);
                        if age <= window {
                            Some((code.clone(), tracked.snapshot.clone(), age))
                        } else {
                            None
                        }
                    })
                })
                .collect();

            // Sort by closest to now (smallest age)
            candidates.sort_by_key(|(_, _, age)| *age);

            for (code, snap, age) in &candidates {
                tracing::trace!(
                    code,
                    age_ms = age.as_millis() as u64,
                    inviter = ?snap.inviter_name,
                    "Recently deleted invite candidate"
                );
            }

            if !candidates.is_empty() {
                let count = candidates.len();
                // Already sorted by closest — pick first
                let (code, snap, age) = candidates.into_iter().next().unwrap();
                tracing::trace!(
                    code = %code,
                    age_ms = age.as_millis() as u64,
                    total_candidates = count,
                    "Primary match: recently deleted invite"
                );
                return Some((code, snap));
            }
        }

        tracing::trace!(guild_id = %guild_id, "No recently deleted invite — trying API fallback");

        // ── Fallback: API fetch + uses comparison (for unlimited invites) ──
        let new_map = match invites::fetch_invites_map(http, guild_id).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(guild_id = %guild_id, "Failed to fetch invites: {e}");
                return None;
            }
        };

        let result = if let Some(old_map) = self.invite_cache.get(&guild_id) {
            let found = new_map
                .iter()
                .find(|(code, tracked)| {
                    if let Some(old) = old_map.get(code.as_str()) {
                        let matched = tracked.snapshot.uses == old.snapshot.uses + 1;
                        if matched {
                            tracing::trace!(
                                code,
                                old_uses = old.snapshot.uses,
                                new_uses = tracked.snapshot.uses,
                                "Fallback match: uses incremented"
                            );
                        }
                        matched
                    } else {
                        false
                    }
                })
                .map(|(code, tracked)| (code.clone(), tracked.snapshot.clone()));

            if found.is_none() {
                tracing::trace!(guild_id = %guild_id, "No invite match found");
            }
            found
        } else {
            tracing::trace!(guild_id = %guild_id, "No cached invites — first join since boot");
            None
        };

        // Merge fresh API data into cache (update existing, add new, keep deleted entries)
        let mut cache = self.invite_cache.entry(guild_id).or_default();
        for (code, tracked) in new_map {
            cache
                .entry(code)
                .and_modify(|existing| {
                    // Update snapshot but preserve deleted_at
                    existing.snapshot = tracked.snapshot.clone();
                })
                .or_insert(tracked);
        }

        result
    }

    /// Prune deleted invites older than `max_age` from all guilds.
    pub fn prune_deleted_invites(&self, max_age: Duration) {
        let now = Instant::now();
        for mut guild_cache in self.invite_cache.iter_mut() {
            guild_cache.value_mut().retain(|_code, tracked| {
                match tracked.deleted_at {
                    Some(del) => now.duration_since(del) < max_age,
                    None => true, // keep active invites
                }
            });
        }
    }

    pub fn mark_recent_ban(&self, guild_id: GuildId, user_id: UserId) {
        let now = unix_now();
        let m = self
            .recent_bans
            .entry(guild_id)
            .or_insert_with(DashMap::new);
        m.insert(user_id, now);
    }

    pub fn was_recently_banned(
        &self,
        guild_id: GuildId,
        user_id: UserId,
        window_secs: i64,
    ) -> bool {
        if let Some(map) = self.recent_bans.get(&guild_id) {
            if let Some(ts) = map.get(&user_id) {
                return unix_now() - *ts <= window_secs;
            }
        }
        false
    }

    pub fn prune_recent_bans(&self, max_age_secs: i64) {
        let now = unix_now();
        for gmap in self.recent_bans.iter_mut() {
            let to_remove: Vec<UserId> = gmap
                .iter()
                .filter_map(|kv| {
                    let (uid, ts) = (kv.key().to_owned(), *kv.value());
                    if now - ts > max_age_secs {
                        Some(uid)
                    } else {
                        None
                    }
                })
                .collect();
            for uid in to_remove {
                gmap.remove(&uid);
            }
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
