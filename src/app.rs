use anyhow::{Context as AnyhowContext, Result};
use poise::Framework;
use serenity::all::{ClientBuilder, GatewayIntents};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::commands::{member, register, settings, stats, userinfo};
use crate::events::event_handler;
use crate::state::AppState;

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let token = std::env::var("DISCORD_TOKEN").context("Set DISCORD_TOKEN in env")?;
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://bot.db".into());

    let token_tail = token
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    info!("Starting bot with DB: {db_url}");
    info!("Discord token: ...{token_tail} (len={})", token.len());

    // GUILD_INVITES is already in non_privileged(), but the bot also needs the
    // Manage Channels permission in Discord for InviteCreate/InviteDelete events
    // to actually be dispatched by the gateway.
    let intents = GatewayIntents::GUILD_MEMBERS | GatewayIntents::non_privileged();

    let framework = Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                register::register(),
                userinfo::userinfo(),
                settings::settings(),
                member::member(),
                stats::stats(),
            ],
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some("~".into()),
                ..Default::default()
            },
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(move |_ctx, _ready, _framework| {
            Box::pin(async move { AppState::new(&db_url).await })
        })
        .build();

    let mut client = ClientBuilder::new(token, intents)
        .framework(framework)
        .await
        .context("Building serenity client failed")?;

    info!("Connecting to Discord gateway…");
    if let Err(e) = client.start().await {
        return Err(anyhow::anyhow!("Discord client error: {e:#}"));
    }

    info!("Discord client disconnected gracefully.");
    Ok(())
}
