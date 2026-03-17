use anyhow::Result;

use crate::state::Ctx;

/// Owner-only prefix command to register or delete application commands.
///
/// Spawns interactive buttons to register/delete globally or in the current guild.
/// Usage: `~register` or `@bot register`
#[poise::command(prefix_command, owners_only, hide_in_help)]
pub async fn register(ctx: Ctx<'_>) -> Result<()> {
    poise::builtins::register_application_commands_buttons(ctx).await?;
    Ok(())
}
