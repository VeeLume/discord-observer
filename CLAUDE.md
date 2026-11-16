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
   events/             ← Per-event handlers (join, leave, invites, member_update, sync)
                         dispatched from events/mod.rs::event_handler
        │
        ▼
   state.rs            ← AppState: invite cache (DashMap), recent_bans, OnceLock guard
        │
        ▼
   services/           ← Domain services: SQL + business logic, grouped by concern
                         (membership writes are transactional; reads live in stats/search)
        │
        ▼
   SQLite (sqlx)       ← Migrations in migrations/, compile-time checked queries
```

### Key Files

| File | Role |
|------|------|
| `src/app.rs` | Framework setup, intents (`GUILD_MEMBERS` + non_privileged), prefix config. No command registration on startup — uses `@bot register` prefix command instead. |
| `src/events/mod.rs` | `event_handler` — the single `match event` block that dispatches to per-event modules. Adds `t = ms_since(boot)` to every `tracing` call. |
| `src/events/{join,leave,invites,member_update,sync}.rs` | One file per event group. `leave.rs` spawns a background enrichment task that delays 3s, checks audit log, then writes departure type + posts the embed. |
| `src/state.rs` | `AppState` struct: `db`, `boot` instant, `invite_cache` (DashMap), `recent_bans`, `maintenance_started: OnceLock<()>`. **No SQL here** — only in-memory caches and the detection helpers that consult them. |
| `src/invites.rs` | `InviteSnapshot` (uses, max_uses, inviter) and `TrackedInvite` (adds created_at, deleted_at lifecycle timestamps). |
| `src/db/mod.rs` | `Db` wrapper around `SqlitePool`. Runs migrations on connect. |
| `src/permissions.rs` | Per-command permission gating helpers. |

### Invite Detection

Two-phase detection in `AppState::detect_invite_used()`:

1. **Primary (timestamp-based):** When a limited invite is consumed, Discord fires `InviteDelete`. The handler sets `tracked.deleted_at = Some(Instant::now())`. After a 3-second delay, detection finds invites with `deleted_at` within a 5-second window and picks the closest one.

2. **Fallback (API comparison):** For unlimited invites (`max_uses=0`) that don't fire `InviteDelete` when used, fetches fresh invites from the Discord API and compares use counts against the cached values.

The invite cache (`DashMap<GuildId, HashMap<String, TrackedInvite>>`) is populated on startup via API fetch, updated by `InviteCreate`/`InviteDelete` gateway events, and pruned every 60 seconds (deleted entries older than 60s).

**Race-condition fix:** `detect_invite_used()` takes an exclusive shard lock via `DashMap::get_mut()` and removes the matched invite atomically. Two concurrent joins can no longer double-match the same just-deleted invite. Use `Instant::checked_duration_since()` for the timestamp window comparison — saturating arithmetic on `Instant` is not safe across reboots-of-monotonic-clock scenarios.

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

SQLite via sqlx with compile-time checked queries (`sqlx::query!`, `sqlx::query_as!`). Migrations live in `migrations/` and run automatically on connect.

**IMPORTANT:** Never apply schema changes directly to the database (e.g. `ALTER TABLE` via sqlite3). Always use the sqlx CLI:
- `sqlx migrate add <name>` — create a new migration file in `migrations/`
- `sqlx migrate run` — apply pending migrations
- `cargo sqlx prepare` — regenerate the offline query cache in `.sqlx/` (required after adding or changing `query!` macros)

sqlx tracks migrations in `_sqlx_migrations` with SHA-384 checksums — manual schema changes cause checksum mismatches or "duplicate column" errors on next run.

### Tables (post-`0011_schema_v2`)

The old monolithic `memberships` table was normalized in migration `0011_schema_v2.sql` (commit `b4c11f7`, 2026-05-01) into:

- `members` — Identity per (guild, user): account_username, display_name, server_nickname, `is_present` flag, `first_seen_at`.
- `stays` — One row per join→leave lifecycle. Holds `departure_type` (enum: `ban` / `kick` / `leave`), `moderator_id`, `reason`, `invite_code`, `inviter_id`, `member_flags`.
- `name_changes` — Append-only log of name changes for the three name fields. Mirrors observer-side history that `View user`-style commands display.
- `guilds` — Per-guild lifecycle (`first_seen_at`). Decoupled from `guild_settings` so a guild row exists even before settings are configured.
- `guild_settings` — Per-guild log channel IDs, `notices_enabled` flag.
- `guild_notices` — Tracks which one-time notices have been sent per guild.
- `bot_message_mentions` — Junction table mapping bot embed messages → user IDs they reference, for cross-link edits when a user is later actioned.
- `usernames_fts` — FTS5 virtual table for fast member name search.

The `banned`/`kicked` boolean columns on the old `memberships` table no longer exist — use `stays.departure_type` instead.

### Services Pattern (replaces the old `src/repos/`)

SQL + business logic live in `src/services/`, grouped by concern rather than by table:

- `membership` — **Transactional writes** for join / leave / name-change. Wraps coupled writes across `members` + `stays` + `name_changes` + FTS in a single transaction so we never end up with half-updated state.
- `stats` — Read-side history queries, retention stats, `DepartureType` enum.
- `search` — FTS5-backed name search.
- `settings` — `GuildSettings` struct, per-guild log-channel config, notices toggle. Includes an `ALLOWED_SETTING_COLUMNS` allowlist used by the dynamic-column update helper (SQL-injection guard for the column-name interpolation).
- `notices` — DB-driven notice system. Reads notice definitions from the `notices` table, respects `current_only` flag and `first_seen_at` per guild. Gated on `notices_enabled` guild setting.
- `audit_log` — Audit-log query helpers (moderator + reason lookup for kicks/bans).
- `embeds` — Embed construction for join/leave/ban/kick log messages.
- `messages` — Bot-message persistence (which embed went where, used by `mentions` for cross-link edits).
- `mentions` — Reads `bot_message_mentions` to find prior embeds that mentioned a user, and edits them in place when that user is later actioned.

Services are free functions taking `&Db` (no repo structs): `services::membership::record_join(&state.db, …)`.

## Commands

Slash commands use poise 0.6. Parent commands have `subcommands(...)` and empty bodies. Each subcommand is a separate function with `rename = "discord-name"`.

The `@bot register` prefix command (owner-only) uses `poise::builtins::register_application_commands_buttons` to interactively register or delete commands globally or per-guild.

### Adding a New Slash Command

1. Create `src/commands/my_command.rs`
2. Add `pub mod my_command;` to `src/commands/mod.rs`
3. Add `my_command::my_command()` to the commands vec in `src/app.rs`
4. Use `@bot register` to re-register

### Adding a Subcommand to /settings

1. Add the function with `#[poise::command(slash_command, guild_only, ephemeral, rename = "name")]`
2. Add `"settings_my_subcommand"` to the parent's `subcommands(...)` list
3. Use `@bot register` to re-register

## Adding a New Setting

1. **Migration:** `sqlx migrate add <name>` → `ALTER TABLE guild_settings ADD COLUMN ...` in the new file.
2. **Service:** Update the `GuildSettings` struct and `get()` query in `src/services/settings.rs`. If the setting is updatable via the dynamic-column helper, add the column name to `ALLOWED_SETTING_COLUMNS`.
3. **Command:** Add a subcommand to `/settings` in `src/commands/settings.rs`.
4. **Display:** Update `settings_show` to include the new setting.
5. **Offline cache:** `cargo sqlx prepare` so the pre-commit/pre-push hooks pass.

## Adding a New Event Handler

Two cases:

**New gateway event type** — add an arm to the `match event` block in `src/events/mod.rs::event_handler`:

```rust
SomeEvent { data } => {
    tracing::trace!(t, /* fields */, "SomeEvent");
    on_some_event(state, data).await?;
}
```

Always include the `t` (milliseconds since boot) field for timing analysis.

**Net-new event group** (rare) — create `src/events/<group>.rs`, declare it in `src/events/mod.rs`, and route to it from the dispatcher. Background work belongs in `tokio::spawn`'d enrichment tasks like `leave.rs::enrich_leave`, not in the synchronous event-handler path — Discord drops events that take too long to ack.

## Conventions

- **Commits:** [Conventional Commits](https://www.conventionalcommits.org/) enforced by `.githooks/commit-msg`.
- **Hooks:** `.githooks/pre-commit` runs `cargo fmt` and **`cargo sqlx prepare`**, then `git add -A` (so it stages *everything* in the working tree — stash unrelated changes before committing). `.githooks/pre-push` runs `cargo fmt --check`, `cargo clippy`, `cargo check`, and **`cargo sqlx prepare --check`**. The Docker build uses the offline cache in `.sqlx/`, so the cache must be committed alongside any change to a `query!`/`query_as!` macro — otherwise the image build fails on the VPS.
- **Errors:** `anyhow::Result` throughout.
- **Logging:** `tracing` macros (`info!`, `warn!`, `trace!`). Use `trace!` for event timing, `info!` for startup, `warn!` for recoverable errors.
- **Concurrent state:** `DashMap` for multi-key concurrent access.
- **Formatting:** `cargo fmt --all`.
- **Linting:** `cargo clippy --all-targets`.

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
| `RUST_LOG` | No | Log level filter (e.g. `observer=trace`) |

## Deployment

Production runs on the VPS and is declared in the separate infra repo (`D:/infra`, stack `discord-observer`), not by this repo's `docker-compose.yml` (that one is a standalone example). The VPS checks out `main` of this repo and builds the image from its `Dockerfile`; state is the named volume `discord-observer_bot_data`.

```sh
# from D:/infra
just deploy discord-observer   # pulls main on the VPS, rebuilds, up -d
just logs discord-observer
```

Push to `main` first — the VPS builds from GitHub, not from your working tree. After changing a command's permissions or signature, run `@bot register` in Discord so the registered commands pick it up.
