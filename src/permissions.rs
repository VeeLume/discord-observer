use poise::serenity_prelude as serenity;
use serenity::all::Permissions;

/// A single permission requirement and whether the bot has it.
pub struct PermissionCheck {
    pub label: &'static str,
    pub granted: bool,
}

/// A bot feature and its permission requirements.
pub struct Feature {
    pub name: &'static str,
    pub description: &'static str,
    pub available: bool,
    pub permissions: Vec<PermissionCheck>,
}

/// Check all bot features against the given permission set.
pub fn check_features(perms: Permissions) -> Vec<Feature> {
    vec![
        check_feature(
            "Join & Leave Logging",
            "Log member joins and leaves to configured channels",
            perms,
            &[(Permissions::SEND_MESSAGES, "Send Messages")],
        ),
        check_feature(
            "Invite Detection",
            "Detect which invite a new member used to join",
            perms,
            &[
                (Permissions::MANAGE_GUILD, "Manage Server"),
                (Permissions::MANAGE_CHANNELS, "Manage Channels"),
            ],
        ),
        check_feature(
            "Ban Detection",
            "Detect bans and identify the moderator",
            perms,
            &[
                (Permissions::BAN_MEMBERS, "Ban Members"),
                (Permissions::VIEW_AUDIT_LOG, "View Audit Log"),
            ],
        ),
        check_feature(
            "Kick Detection",
            "Detect kicks and identify the moderator",
            perms,
            &[(Permissions::VIEW_AUDIT_LOG, "View Audit Log")],
        ),
    ]
}

fn check_feature(
    name: &'static str,
    description: &'static str,
    perms: Permissions,
    required: &[(Permissions, &'static str)],
) -> Feature {
    let checks: Vec<PermissionCheck> = required
        .iter()
        .map(|(perm, label)| PermissionCheck {
            label,
            granted: perms.contains(*perm),
        })
        .collect();

    let available = checks.iter().all(|c| c.granted);

    Feature {
        name,
        description,
        available,
        permissions: checks,
    }
}

/// Get the bot's permissions in a guild from the cache.
///
/// Returns `None` if the guild or bot member isn't cached.
pub fn bot_permissions_cached(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    bot_id: serenity::UserId,
) -> Option<Permissions> {
    let guild = ctx.cache.guild(guild_id)?;
    let member = guild.members.get(&bot_id)?;
    // We intentionally use the deprecated guild-level method here because we want
    // server-wide permissions, not channel-specific overrides.
    #[allow(deprecated)]
    Some(guild.member_permissions(member))
}
