# `droppen session-proxy` (docs/session-proxy.md): the session proxy, for
# Railway or anything that runs a container. Its configuration is the
# environment (DROPPEN_SESSION_PROXY_TOKEN, DROPPEN_HUB, DROPPEN_ENROLL_CODE
# once, PORT); its identity and verified device chain live in /data, which
# must be a persistent volume.
#
# Two ways to build, one image:
# - from source, as Railway or `docker build` does — context: the repository
#   root:
#     docker build -f crates/droppen-session-proxy/Dockerfile .
# - from a release's Linux binary, as .github/workflows/release.yml does
#   (`session-proxy-image`): BINARY=release, and a context holding
#   bin/<amd64|arm64>/droppen with LICENSE and THIRD_PARTY_NOTICES under doc/
#   — the very binary the release's tarball carries, nothing compiled.
#
# It runs as an unprivileged user (10001) that owns /data. Give it 10 s
# after SIGTERM. The image has no shell tools; `droppen session-proxy
# --health` is its health check.

ARG BINARY=source

# The workspace, built with the Rust the release builds with
# (scripts/build-linux.sh), as a server-only binary: the proxy serves no web
# app.
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS source
WORKDIR /src
COPY Cargo.toml Cargo.lock LICENSE ./
COPY crates/ ./crates/
ENV DROPPEN_ALLOW_MISSING_WEB_DIST=1
RUN cargo build --release --locked -p droppen-server \
 && mkdir -p /out/doc \
 && cp target/release/droppen-server /out/droppen \
 && cp LICENSE /out/doc/LICENSE

# A release's binary, for the platform being built.
FROM scratch AS release
ARG TARGETARCH
COPY bin/${TARGETARCH}/droppen /out/droppen
COPY doc/ /out/doc/

FROM ${BINARY} AS binary

FROM debian:bookworm-slim@sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e
# The proxy's own user, and its data directory: a volume mounted there
# starts out owned by it (Docker copies the ownership in; on Railway see
# docs/session-proxy.md). TLS roots are the binary's own (web PKI, bundled):
# no CA package.
RUN groupadd --gid 10001 droppen \
 && useradd --uid 10001 --gid droppen --home-dir /data --no-create-home --shell /usr/sbin/nologin droppen \
 && mkdir -p /data \
 && chown droppen:droppen /data \
 && chmod 700 /data
COPY --from=binary /out/droppen /usr/local/bin/droppen
COPY --from=binary /out/doc/ /usr/share/doc/droppen/
USER 10001:10001
ENV DROPPEN_DATA_DIR=/data \
    PORT=8080 \
    RUST_LOG=info
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=20s --retries=3 CMD ["droppen", "session-proxy", "--health"]
ENTRYPOINT ["droppen", "session-proxy"]
