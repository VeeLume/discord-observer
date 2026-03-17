use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::all::ChannelId;

use crate::db::Db;

const ALLOWED_SETTING_COLUMNS: &[&str] = &[
    "join_log_channel_id",
    "leave_log_channel_id",
    "mod_log_channel_id",
];

#[derive(Debug, Clone, Copy)]
pub struct GuildSettings {
    pub join_log: Option<ChannelId>,
    pub leave_log: Option<ChannelId>,
    pub mod_log: Option<ChannelId>,
    pub notices_enabled: bool,
}

impl Default for GuildSettings {
    fn default() -> Self {
        Self {
            join_log: None,
            leave_log: None,
            mod_log: None,
            notices_enabled: true,
        }
    }
}

#[derive(Clone)]
pub struct GuildSettingsRepo<'a> {
    db: &'a Db,
}

impl<'a> GuildSettingsRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub async fn get(&self, guild_id: &serenity::all::GuildId) -> Result<GuildSettings> {
        let guild = guild_id.to_string();
        let rec = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, i64)>(
            "SELECT join_log_channel_id, leave_log_channel_id, mod_log_channel_id, notices_enabled \
             FROM guild_settings WHERE guild_id = ?",
        )
        .bind(&guild)
        .fetch_optional(&self.db.pool)
        .await?;

        let parse_channel = |val: &Option<String>| {
            val.as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                .map(serenity::all::ChannelId::new)
        };

        match rec {
            Some((join, leave, mlog, notices)) => Ok(GuildSettings {
                join_log: parse_channel(&join),
                leave_log: parse_channel(&leave),
                mod_log: parse_channel(&mlog),
                notices_enabled: notices != 0,
            }),
            None => Ok(GuildSettings::default()),
        }
    }

    pub async fn upsert(
        &self,
        guild_id: &serenity::all::GuildId,
        join: Option<ChannelId>,
        leave: Option<ChannelId>,
        log_channel: Option<ChannelId>,
    ) -> Result<()> {
        let guild_id = guild_id.to_string();
        let join = join.map(|c| c.to_string());
        let leave = leave.map(|c| c.to_string());
        let modu = log_channel.map(|c| c.to_string());

        sqlx::query!(
            r#"
            INSERT INTO guild_settings (guild_id, join_log_channel_id, leave_log_channel_id, mod_log_channel_id)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(guild_id) DO UPDATE SET
              join_log_channel_id = COALESCE(excluded.join_log_channel_id, guild_settings.join_log_channel_id),
              leave_log_channel_id = COALESCE(excluded.leave_log_channel_id, guild_settings.leave_log_channel_id),
              mod_log_channel_id   = COALESCE(excluded.mod_log_channel_id,   guild_settings.mod_log_channel_id)
            "#,
            guild_id, join, leave, modu
        )
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Ensure row exists (used before column-wise updates).
    pub async fn ensure_row(&self, guild_id: &serenity::all::GuildId) -> Result<()> {
        let gid = guild_id.to_string();
        sqlx::query!(
            r#"INSERT INTO guild_settings (guild_id) VALUES (?) ON CONFLICT(guild_id) DO NOTHING"#,
            gid
        )
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn set_column(
        &self,
        guild_id: &serenity::all::GuildId,
        column: &str,
        value: Option<ChannelId>,
    ) -> Result<()> {
        anyhow::ensure!(
            ALLOWED_SETTING_COLUMNS.contains(&column),
            "Invalid column: {column}"
        );

        let gid = guild_id.to_string();
        if let Some(id) = value {
            let q = format!("UPDATE guild_settings SET {column} = ? WHERE guild_id = ?");
            sqlx::query(&q)
                .bind(id.get().to_string())
                .bind(gid)
                .execute(&self.db.pool)
                .await?;
        } else {
            let q = format!("UPDATE guild_settings SET {column} = NULL WHERE guild_id = ?");
            sqlx::query(&q).bind(gid).execute(&self.db.pool).await?;
        }
        Ok(())
    }

    /// Set notices enabled/disabled.
    pub async fn set_notices_enabled(
        &self,
        guild_id: &serenity::all::GuildId,
        enabled: bool,
    ) -> Result<()> {
        let gid = guild_id.to_string();
        let val = if enabled { 1_i64 } else { 0_i64 };
        sqlx::query("UPDATE guild_settings SET notices_enabled = ? WHERE guild_id = ?")
            .bind(val)
            .bind(gid)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// Convenience: get settings for this guild.
    pub async fn get_for_guild(&self, guild_id: &serenity::all::GuildId) -> Result<GuildSettings> {
        self.get(guild_id).await
    }

    /// Set join-log channel (or clear it if `None`).
    pub async fn set_join_log(
        &self,
        guild_id: &serenity::all::GuildId,
        channel: Option<ChannelId>,
    ) -> Result<()> {
        self.set_column(guild_id, "join_log_channel_id", channel)
            .await
    }

    /// Set leave-log channel (or clear it if `None`).
    pub async fn set_leave_log(
        &self,
        guild_id: &serenity::all::GuildId,
        channel: Option<ChannelId>,
    ) -> Result<()> {
        self.set_column(guild_id, "leave_log_channel_id", channel)
            .await
    }

    /// Set mod-log channel (or clear it if `None`).
    pub async fn set_mod_log(
        &self,
        guild_id: &serenity::all::GuildId,
        channel: Option<ChannelId>,
    ) -> Result<()> {
        self.set_column(guild_id, "mod_log_channel_id", channel)
            .await
    }
}
