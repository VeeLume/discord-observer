mod app;
mod commands;
mod db;
mod events;
mod invites;
mod permissions;
mod services;
mod state;

// Avoid musl's default allocator due to lackluster performance
// https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() {
    if let Err(e) = app::run().await {
        tracing::error!("Fatal error: {e:#}");
        std::process::exit(1);
    }
}
