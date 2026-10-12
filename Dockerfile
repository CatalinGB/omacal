# OmaCal web build. Three stages: the Svelte bundle (web flag), the
# Tauri-free `omacal-server` binary, then a distroless runtime.
#
# The binary is a normal glibc build, so the runtime must be the *same* Debian
# major: built on `rust:1-bookworm` (Debian 12), run on
# `gcr.io/distroless/cc-debian12` (Debian 12). A Debian 13 glibc binary would
# not start here, and vice versa. `distroless/cc` already carries the C runtime,
# `ca-certificates`, `tzdata` and a `nonroot` user (uid 65532).

# ---- 1. Svelte UI -> ui/dist ---------------------------------------------
FROM node:22-bookworm-slim AS ui
WORKDIR /app
COPY ui/package.json ui/package-lock.json ./ui/
RUN npm --prefix ui ci
COPY ui ./ui
# VITE_TARGET=web installs the HTTP shim that repoints `invoke`/`listen` at
# omacal-server. Unset (the desktop build) the shim is dropped.
RUN VITE_TARGET=web npm --prefix ui run build

# ---- 2. Rust server ------------------------------------------------------
FROM rust:1-bookworm AS server
WORKDIR /app
COPY . .
RUN cargo build --release -p omacal-server
# The volume mountpoint, with a marker so the runtime `COPY --chown` creates a
# non-root-owned `/data` for a fresh named volume (an empty source directory
# alone can be dropped by COPY).
RUN mkdir -p /data && touch /data/.keep

# ---- 3. Runtime (distroless, no shell) -----------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app
COPY --from=server /app/target/release/omacal-server /usr/local/bin/omacal-server
COPY --from=ui /app/ui/dist ./ui/dist
# An empty named volume inherits the mountpoint's ownership on first use, so
# non-root (65532) can write the DB and the credential file.
COPY --from=server --chown=65532:65532 /data /data

ENV XDG_DATA_HOME=/data \
    TZ=Europe/Bucharest \
    OMACAL_BIND=0.0.0.0:8080 \
    OMACAL_UI_DIR=/app/ui/dist
EXPOSE 8080
VOLUME ["/data"]

# No shell/curl in the image, so the probe is the binary's own exec-form
# subcommand: `GET /healthz`, exit 0/1.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/usr/local/bin/omacal-server", "healthcheck"]

USER nonroot
ENTRYPOINT ["/usr/local/bin/omacal-server"]
