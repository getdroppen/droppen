# The image desktop/build-linux.sh builds the Linux app in: Ubuntu 22.04, by
# digest, with the WebKitGTK stack Tauri needs from the live jammy archive,
# and Rust. Its glibc (2.35) is the oldest the app and its AppImage run on:
# Ubuntu 22.04, Debian 12, Fedora 36 and newer.
#
# The apt lists stay in the image, with the sources (deb-src) of the same
# archive beside the packages: the sources of the libraries the AppImage
# bundles are fetched against these very lists, so they are the versions
# installed here, and none other.
#
#   docker build --platform linux/<amd64|arm64> -f desktop/linux/Dockerfile desktop/linux
FROM ubuntu:22.04@sha256:74b12b11ffa550fba1929ad4d8d16533c34f6612728469671afba1b03b66881c

# The Rust every release build uses (RUST_TOOLCHAIN in
# .github/workflows/release.yml), installed by rustup-init of this version,
# checked by its sha256 (static.rust-lang.org's .sha256 beside it).
ARG RUST_TOOLCHAIN=1.98.1
ARG RUSTUP_VERSION=1.29.3
ARG RUSTUP_SHA256_X86_64=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
ARG RUSTUP_SHA256_AARCH64=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282

ENV DEBIAN_FRONTEND=noninteractive
# A deb-src line for every deb line, then one update for both. Tauri 2's
# Linux prerequisites (https://v2.tauri.app/start/prerequisites/#linux), as
# CI's desktop-app-linux job installs them. The downloaded packages go; the
# lists stay.
RUN sed -n 's/^deb /deb-src /p' /etc/apt/sources.list > /etc/apt/sources.list.d/deb-src.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl file pkg-config \
      libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
 && apt-get clean

ENV RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:$PATH
RUN arch=$(uname -m) \
 && case "$arch" in x86_64) sum=$RUSTUP_SHA256_X86_64 ;; aarch64) sum=$RUSTUP_SHA256_AARCH64 ;; *) echo "no rustup-init pinned for $arch" >&2; exit 1 ;; esac \
 && curl -fsSL --proto '=https' --tlsv1.2 -o /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/$arch-unknown-linux-gnu/rustup-init" \
 && echo "$sum  /tmp/rustup-init" | sha256sum -c - \
 && chmod 755 /tmp/rustup-init \
 && CARGO_HOME=/usr/local/cargo /tmp/rustup-init -y -q --no-modify-path --profile minimal --default-toolchain "$RUST_TOOLCHAIN" \
 && rm /tmp/rustup-init \
 && chmod -R a+rX /usr/local/rustup /usr/local/cargo \
 && rustc --version
