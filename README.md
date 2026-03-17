# Observer — Discord Member Tracking Bot

A Discord bot that tracks member joins, leaves, bans, and kicks with rich embeds and invite attribution. Built with Rust using [serenity](https://github.com/serenity-rs/serenity) and [poise](https://github.com/serenity-rs/poise).

## Features

- **Invite tracking** — Detects which invite was used for each join, including right-click "Invite to Server" friend invites. Uses timestamp-based matching for single-use invites and API fallback for unlimited invites.
- **Rich join embeds** — Account age, new-account warnings, rejoin detection, invite code and inviter.
- **Rich leave embeds** — Membership duration, join date, stay history.
- **Ban & kick detection** — Classifies leaves as voluntary, kicked, or banned using gateway events and audit log. Shows the moderator and reason.
- **Configurable log channels** — Per-guild join log, leave log, and moderation log.
- **Member history & stats** — Slash commands for retention stats, rejoin tracking, and member search.
- **Notice system** — One-time patch notes with per-guild enable/disable and history.

## Required Permissions

The bot needs the following Discord permissions to function. Each permission maps to a specific feature:

| Permission | What it enables |
|---|---|
| **Send Messages** | Posting join/leave/ban/kick embeds to your log channels |
| **Manage Server** | Reading the server's invite list to track which invite each new member used |
| **Manage Channels** | Receiving real-time `InviteCreate` / `InviteDelete` events from Discord's gateway — without this, the bot can only fall back to periodic API polling for invite detection |
| **Ban Members** | Receiving `GuildBanAddition` events so the bot can distinguish bans from voluntary leaves and show who issued the ban |
| **View Audit Log** | Looking up who kicked or banned a member and the reason they provided — this is the only way to detect kicks, since Discord has no gateway event for them |

### Required Gateway Intents

In the [Developer Portal](https://discord.com/developers/applications), enable **Server Members Intent** under *Bot → Privileged Gateway Intents*. This is required to receive join and leave events. All other intents the bot needs (`GUILD_MODERATION`, `GUILD_INVITES`, etc.) are non-privileged and enabled automatically.

## Setup

1. Create a Discord bot at the [Developer Portal](https://discord.com/developers/applications).

2. Grant the permissions listed above when inviting the bot to your server.

3. Enable **Server Members Intent** in the Developer Portal (see above).

4. Copy `.env.example` to `.env` and fill in your values:
   ```sh
   cp .env.example .env
   ```

5. Run the bot:
   ```sh
   cargo run
   ```

6. **First run:** Send `@bot register` in a server channel to register slash commands. This spawns interactive buttons to register globally or in the current guild.

7. Use `/settings join-log`, `/settings leave-log`, and `/settings mod-log` to configure which channels receive embeds.

## Docker

```sh
docker compose up -d
```

The included `Dockerfile` builds a statically linked musl binary with mimalloc. The `docker-compose.yml` mounts a `data/` volume for the SQLite database.

## Project Structure

```
src/
├── main.rs              # Entry point
├── app.rs               # Framework setup, intents, prefix config
├── events.rs            # Gateway event handlers (join, leave, ban, invite)
├── state.rs             # AppState (DB, invite cache, ban tracking)
├── notices.rs           # One-time notice system (patch notes)
├── invites.rs           # InviteSnapshot, TrackedInvite, API fetch
├── db/
│   └── mod.rs           # SQLite connection pool (sqlx)
├── repos/
│   ├── memberships_repo.rs    # Join/leave/ban history
│   ├── guild_settings_repo.rs # Per-guild log channel config
│   └── guild_notices_repo.rs  # Notice send tracking
├── commands/
│   ├── register.rs      # ~register (owner-only, registers slash commands)
│   ├── settings.rs      # /settings (log channels, notices toggle)
│   ├── userinfo.rs      # /userinfo (member history)
│   ├── member.rs        # /member search
│   └── stats.rs         # /stats (retention, rejoins, balance)
└── migrations/          # SQLite schema migrations
```

## Commands

| Command | Description |
|---------|-------------|
| `@bot register` | Register slash commands (owner-only prefix command) |
| `/settings join-log` | Set the join log channel |
| `/settings leave-log` | Set the leave log channel |
| `/settings mod-log` | Set the moderation log channel |
| `/settings notices` | Enable or disable automatic notices |
| `/settings notices-history` | Show sent notice history |
| `/settings show` | Show current settings |
| `/notice create` | Create a notice to send to guilds (owner-only) |
| `/notice send` | Send pending notices to all guilds now (owner-only) |
| `/notice list` | List all notice definitions (owner-only) |
| `/notice delete` | Delete a notice (owner-only) |
| `/userinfo` | View a member's join/leave history |
| `/member search` | Search members by name |
| `/stats` | Retention stats, rejoins, exits, balance |

## Architecture

See [CLAUDE.md](CLAUDE.md) for detailed architecture documentation, coding conventions, and agent guidelines.

## Development

**Prerequisites:** Rust (stable).

```sh
cargo fmt --all
cargo clippy --all-targets
cargo check
```

This project uses [Conventional Commits](https://www.conventionalcommits.org/) enforced by a `commit-msg` git hook in `.githooks/`. Configure with:

```sh
git config core.hooksPath .githooks
```

## License

MIT OR Apache-2.0
