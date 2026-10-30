#!/bin/sh
# A member's `test-support` feature hands tests what production code must not
# have: the provider base's probes, a permit that takes no spawn gate, stand-in
# logins. Tests turn it on through `[dev-dependencies]`, which no release build
# reads. This checks that nothing turns one on any other way (a dependency's
# features, a default): a build of the whole server-rs workspace without its
# dev-dependencies, which is more than any release builds, has none on. The
# targets are the ones this workspace is built for, as in
# `check-serde-json-order.sh`.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
manifest="$root/server-rs/Cargo.toml"
targets="--target x86_64-unknown-linux-gnu --target aarch64-unknown-linux-gnu --target aarch64-apple-darwin --target x86_64-apple-darwin"

# Its own step, so a failing `cargo tree` fails the check rather than reading
# as an empty graph.
graph=$(cargo tree --manifest-path "$manifest" --workspace $targets -e features,normal,build --prefix none)
on=$(printf '%s\n' "$graph" | sed -n 's/^\([^ ]*\) feature "test-support".*/\1/p' | sort -u)
if [ -n "$on" ]; then
  echo "test-support is on without dev-dependencies for:" $on >&2
  echo "turn it on in [dev-dependencies] only" >&2
  exit 1
fi
echo "test-support is off in every build without dev-dependencies"
