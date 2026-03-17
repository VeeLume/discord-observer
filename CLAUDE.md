# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) and other coding agents when working with this repository.

## Project Overview

**Observer** is a Rust Discord bot that tracks member joins, leaves, bans, kicks, and invite usage. It posts rich embeds to configurable log channels and stores membership history in SQLite.

**Stack:** Rust, serenity 0.12, poise 0.6, sqlx 0.7 (SQLite), tokio, dashmap, tracing.

## Architecture

### Data Flow

```
Discord Gateway Events
        │
        ▼
   events.rs          ← Event dispatch (join, leave, ban, invite create/delete)
        │
        ▼
   state.rs           ← AppState: invite cache (DashMap), ban tracking, detection logic
        │
        ▼
   repos/             ← Repository pattern: SQL queries isolated from business logic
        │
        ▼
   SQLite (sqlx)      ← Migrations in migrations/, runtime queries
```

### Key Files

| File | Role |
|------|------|
| `src/app.rs` | Framework setup, intents, prefix config. No command registration on startup — uses `~register` prefix command instead. |
| `src/events.rs` | All gateway event handlers. Uses `ms_since(boot)` timestamps for timing analysis. |
| `src/state.rs` | `AppState` struct: DB handle, `boot` instant, `invite_cache` (DashMap of TrackedInvite), `recent_bans`. Detection logic lives here. |
| `src/invites.rs` | `InviteSnapshot` (uses, max_uses, inviter) and `TrackedInvite` (adds created_at, deleted_at lifecycle timestamps). |
| `src/notices.rs` | One-time notice system. `NOTICES` const array of `NoticeDefinition` with `check` function pointers. Gated on `notices_enabled` guild setting. |

### Invite Detection

Two-phase detection in `AppState::detect_invite_used()`:

1. **Primary (timestamp-based):** When a limited invite is consumed, Discord fires `InviteDelete`. The handler sets `tracked.deleted_at = Some(Instant::now())`. After a 3-second delay, detection finds invites with `deleted_at` within a 5-second window and picks the closest one.

2. **Fallback (API comparison):** For unlimited invites (`max_uses=0`) that don't fire `InviteDelete` when used, fetches fresh invites from the Discord API and compares use counts against the cached values.

The invite cache (`DashMap<GuildId, HashMap<String, TrackedInvite>>`) is populated on startup via API fetch, updated by `InviteCreate`/`InviteDelete` gateway events, and pruned every 60 seconds (deleted entries older than 60s).

### Ban & Kick Detection

- **Bans:** `GuildBanAddition` gateway event fires and calls `mark_recent_ban()`. The `GuildMemberRemoval` handler delays 3 seconds, then checks `was_recently_banned()`. Audit log is also queried for the moderator info.
- **Kicks:** No gateway event exists. Detected solely via audit log (`MemberAction::Kick`) in `check_audit_log_for_removal()`.
- Both paths extract the moderator and reason from the audit log entry.

### Required Discord Permissions

Manage Server, Manage Channels, Ban Members, View Audit Log, Send Messages.

### Required Gateway Intents

`GUILD_MEMBERS` (privileged — must be enabled in Discord Developer Portal). All other needed intents are non-privileged and covered by `GatewayIntents::non_privileged()`:
- `GUILD_MODERATION` (bit 2) — for `GuildBanAddition` events
- `GUILD_INVITES` (bit 18) — for `InviteCreate`/`InviteDelete` events

**Important:** `InviteCreate`/`InviteDelete` events also require the bot to have the **Manage Channels** permission in Discord, even with the intent enabled.

## Database

SQLite via sqlx with runtime queries (`sqlx::query_as`, `sqlx::query`). Migrations live in `migrations/` and run automatically on connect.

### Tables

- `memberships` — Join/leave/ban history per user per guild
- `guild_settings` — Per-guild log channel IDs, notices_enabled flag
- `guild_notices` — Tracks which one-time notices have been sent
- `usernames_fts` — FTS5 virtual table for fast member name search

### Repository Pattern

SQL is isolated in `src/repos/`:
- `MembershipsRepo` — join/leave/ban records, history queries, stats
- `GuildSettingsRepo` — channel config, notices toggle
- `GuildNoticesRepo` — notice send tracking

Repos take `&Db` and are created per-call: `let repo = MembershipsRepo::new(&state.db);`

## Commands

Slash commands use poise 0.6. Parent commands have `subcommands(...)` and empty bodies. Each subcommand is a separate function with `rename = "discord-name"`.

The `~register` prefix command (owner-only) uses `poise::builtins::register_application_commands_buttons` to interactively register or delete commands globally or per-guild.

### Adding a New Slash Command

1. Create `src/commands/my_command.rs`
2. Add `pub mod my_command;` to `src/commands/mod.rs`
3. Add `my_command::my_command()` to the commands vec in `src/app.rs`
4. Use `~register` to re-register

### Adding a Subcommand to /settings

1. Add the function with `#[poise::command(slash_command, guild_only, ephemeral, rename = "name")]`
2. Add `"settings_my_subcommand"` to the parent's `subcommands(...)` list
3. Use `~register` to re-register

## Adding a New Setting

1. **Migration:** Create `migrations/N_description.sql` with `ALTER TABLE guild_settings ADD COLUMN ...`
2. **Repo:** Update `GuildSettings` struct and `get()` query in `guild_settings_repo.rs`
3. **Command:** Add a subcommand to `/settings` in `settings.rs`
4. **Display:** Update `settings_show` to include the new setting

## Adding a New Event Handler

In `src/events.rs`, add a new arm to the `match event` block:

```rust
SomeEvent { data } => {
    tracing::trace!(t, /* fields */, "SomeEvent");
    on_some_event(state, data).await?;
}
```

Always include the `t` (milliseconds since boot) field for timing analysis.

## Conventions

- **Commits:** [Conventional Commits](https://www.conventionalcommits.org/) enforced by `.githooks/commit-msg`
- **Errors:** `anyhow::Result` throughout
- **Logging:** `tracing` macros (`info!`, `warn!`, `trace!`). Use `trace!` for event timing, `info!` for startup, `warn!` for recoverable errors.
- **Concurrent state:** `DashMap` for multi-key concurrent access
- **Formatting:** `cargo fmt --all`
- **Linting:** `cargo clippy --all-targets`

## Development Commands

```sh
cargo run                          # Run the bot (reads .env)
cargo fmt --all                    # Format code
cargo clippy --all-targets         # Lint
cargo check                        # Type check without building
RUST_LOG=observer=trace cargo run  # Run with trace logging
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `DISCORD_TOKEN` | Yes | Bot token from Discord Developer Portal |
| `DATABASE_URL` | No | SQLite URL (default: `sqlite://bot.db`) |
| `CLIENT_ID` | No | Application client ID for invite URL in notices |
| `RUST_LOG` | No | Log level filter (e.g. `observer=trace`) |
