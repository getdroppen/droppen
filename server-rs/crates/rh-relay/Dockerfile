# droppen-relay, for Railway, Fly.io or anything that runs a container.
#
# Build context: the repository root —
#   docker build -f server-rs/crates/rh-relay/Dockerfile .
# The relay needs no database, no web app and no files: one static process.
# Give it ≥ 35 s after SIGTERM so `goaway` can drain its clients
# (DROPPEN_RELAY_DRAIN_SECS, default 30).
#
# The binary is the `droppen-relay` package (crates/droppen-relay); this file
# stays beside the library because the deployment configuration outside this
# repository and the services' settings name this path.

FROM rust:1-slim-bookworm AS build
WORKDIR /src/server-rs
COPY server-rs/ ./
RUN cargo build --release --locked -p droppen-relay \
 && cp target/release/droppen-relay /usr/local/bin/droppen-relay

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/droppen-relay /usr/local/bin/droppen-relay
# Nobody: the relay keeps nothing on disk unless DROPPEN_RELAY_STATE_DIR names a volume.
USER 65534:65534
ENV RUST_LOG=info
EXPOSE 8080
CMD ["droppen-relay", "serve"]
