# OmaCal web build. Multi-stage: the Svelte bundle (web flag), the
# Tauri-free `omacal-server` binary, then a slim runtime.

FROM node:22-bookworm-slim AS ui
WORKDIR /app
COPY ui/package.json ui/package-lock.json ./ui/
RUN npm --prefix ui ci
COPY ui ./ui
# VITE_TARGET=web installs the HTTP shim that repoints `invoke`/`listen` at
# omacal-server. Unset (the desktop build) the shim is dropped.
RUN VITE_TARGET=web npm --prefix ui run build

FROM rust:1-bookworm AS server
WORKDIR /app
COPY . .
RUN cargo build --release -p omacal-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 10001 omacal \
 && mkdir -p /data && chown omacal:omacal /data

WORKDIR /app
COPY --from=server /app/target/release/omacal-server /usr/local/bin/omacal-server
COPY --from=ui /app/ui/dist ./ui/dist

ENV XDG_DATA_HOME=/data \
    TZ=Europe/Bucharest \
    OMACAL_BIND=0.0.0.0:8080
EXPOSE 8080
VOLUME ["/data"]

USER omacal
CMD ["omacal-server"]
