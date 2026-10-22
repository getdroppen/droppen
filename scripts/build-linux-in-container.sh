#!/usr/bin/env bash
# The Linux server build itself, run inside IMAGE of scripts/build-linux.sh
# (rust:1.98.1-bookworm, pinned by digest): x86_64 and aarch64 linked against
# glibc 2.28 with zig and cargo-zigbuild, both pinned (zig's tarball checked
# against its sha256). scripts/build-linux.sh runs it in a throwaway pod;
# .github/workflows/release.yml in the same image with `docker run`. Both put
# the source in the same place, so the binaries name the same paths (remapped).
#
#   /work/src     the source tree, with web/dist built (the web app is
#                 embedded at compile time)
#   /work/target  cargo's target directory
#   /work/out     receives <arch>/remote-hand-server
#
# Environment:
#   DROPPEN_BUILD_REV  the short commit (required; passed on as RH_BUILD_REV
#                      too, for a commit from before the rename to Droppen)
#   ARCHES             which to build (default "x86_64 aarch64")
set -euo pipefail

ZIG_VERSION=0.13.0
ZIG_SHA256=d45312e61ebcc48032b77bc4cf7fd6915c11fa16e4aad116b66c9468211230ea
ZIGBUILD_VERSION=0.23.4
GLIBC=2.28
: "${DROPPEN_BUILD_REV:?set DROPPEN_BUILD_REV to the short commit}"
export RH_BUILD_REV=$DROPPEN_BUILD_REV
ARCHES=${ARCHES:-x86_64 aarch64}
[ -f /work/src/web/dist/index.html ] || { echo "build-linux-in-container: /work/src/web/dist is missing; build the web app first" >&2; exit 2; }

cd /tmp
curl -fsSL -o zig.tar.xz "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-x86_64-${ZIG_VERSION}.tar.xz"
echo "${ZIG_SHA256}  zig.tar.xz" | sha256sum -c -
tar -xJf zig.tar.xz -C /opt
export PATH="/opt/zig-linux-x86_64-${ZIG_VERSION}:$PATH"
zig version
export CARGO_TARGET_DIR=/work/target
cargo install --locked --quiet cargo-zigbuild --version "${ZIGBUILD_VERSION}"
for arch in $ARCHES; do
  rustup target add "${arch}-unknown-linux-gnu"
done
cd /work/src/server-rs
for arch in $ARCHES; do
  echo "== ${arch}-unknown-linux-gnu.${GLIBC}"
  RUSTFLAGS="--remap-path-prefix=/work/src=/droppen --remap-path-prefix=${CARGO_HOME:-/usr/local/cargo}=/cargo" \
    cargo zigbuild --release --locked --target "${arch}-unknown-linux-gnu.${GLIBC}" -p remote-hand-server
  mkdir -p "/work/out/${arch}"
  cp "/work/target/${arch}-unknown-linux-gnu/release/remote-hand-server" "/work/out/${arch}/"
done
ls -l /work/out/*
