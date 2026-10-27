#!/bin/sh
# droppen-plugin-sdk is the open contract between Droppen and its plugins
# (docs/plugins/runtime.md): its normal dependency tree may reach neither the
# host (gotcha, axum, tokio) nor the hosted service (rh-hub, rh-proto). The
# TypeScript half of the same rule is scripts/check-plugin-boundary.mjs.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
tree=$(cargo tree --manifest-path "$root/server-rs/Cargo.toml" -p droppen-plugin-sdk -e normal --prefix none)
# Each line is `<crate> v<version> …`; a forbidden name also covers its
# satellites (`gotcha_core`, `tokio-util`, `axum-core`).
leaks=$(printf '%s\n' "$tree" | grep -E '^(rh-hub|rh-proto|gotcha|axum|tokio)([-_][a-z0-9_-]+)? v' | sort -u || true)
if [ -n "$leaks" ]; then
  echo "droppen-plugin-sdk may not depend on the host or the hosted service, but its tree has:" >&2
  printf '%s\n' "$leaks" >&2
  exit 1
fi
count=$(printf '%s\n' "$tree" | sort -u | wc -l | tr -d ' ')
echo "droppen-plugin-sdk: $count crates in its tree, none of rh-hub, rh-proto, gotcha, axum or tokio"
