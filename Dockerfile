# syntax=docker/dockerfile:1.7

############################
# Build & dependency cache #
############################
ARG RUST_VERSION=1.88
ARG BIN_NAME=observer

FROM rust:${RUST_VERSION}-alpine AS chef
WORKDIR /app

# Tooling needed to compile native deps for musl (incl. OpenSSL) + cargo-chef
RUN apk add --no-cache \
      musl-dev build-base pkgconf \
      openssl-dev openssl-libs-static \
      git curl ca-certificates

# SQLX offline mode (expects sqlx-data.json in repo)
ENV SQLX_OFFLINE=true

# Build for a fully-static musl target
RUN rustup target add x86_64-unknown-linux-musl

# Make pkg-config work in cross builds and prefer static OpenSSL
ENV PKG_CONFIG_ALLOW_CROSS=1 \
    OPENSSL_STATIC=1

# Install cargo-chef for dependency caching
RUN cargo install cargo-chef --locked

###########################
# Stage 1: plan the deps  #
###########################
FROM chef AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

########################################
# Stage 2: cook (compile) dependencies #
########################################
FROM chef AS builder
WORKDIR /app

# re-declare args in this stage so ${BIN_NAME} is available
ARG BIN_NAME=observer

COPY --from=planner /app/recipe.json /app/recipe.json

# Cache registry + git between builds (BuildKit)
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

# Now build the actual application
COPY . .

RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    cargo build --release --target x86_64-unknown-linux-musl --bin "${BIN_NAME}" \
 && strip "target/x86_64-unknown-linux-musl/release/${BIN_NAME}"

# Prepare an owned data dir we can copy into scratch
RUN mkdir -p /data-owned

################
# Runtime stage
################
FROM scratch

# re-declare so COPY can use it here too (optional, but clearer)
ARG BIN_NAME=observer

# TLS certs for HTTPS
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/

# App binary
COPY --from=builder "/app/target/x86_64-unknown-linux-musl/release/${BIN_NAME}" /app

# Pre-create /data owned by non-root
COPY --from=builder --chown=10001:10001 /data-owned /data

# Run as non-root
USER 10001:10001

# Defaults (UTC, sqlite in /data)
ENV RUST_LOG=info \
    DATABASE_URL=sqlite:///data/bot.db \
    TZ=UTC

VOLUME ["/data"]

# Use --init in run/compose for clean PID1 handling
ENTRYPOINT ["/app"]
