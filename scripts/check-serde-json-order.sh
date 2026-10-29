#!/bin/sh
# serde_json's `preserve_order` is on for the whole server-rs workspace
# (`[workspace.dependencies]` in server-rs/Cargo.toml says why). Cargo resolves
# features for the packages a build selects, so every member whose dependency
# graph reaches serde_json has to turn the feature on itself; otherwise
# `cargo test -p <member>` writes JSON keys in another order than a workspace
# build does. A member that names serde_json takes it from the workspace, so
# the feature does not hang on what else that member happens to depend on.
# The targets are the ones this workspace is built for: macOS (releases,
# development) and Linux (CI, relays); `--target all` would also count
# reqwest's wasm-only use of serde_json.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
manifest="$root/server-rs/Cargo.toml"
targets="--target x86_64-unknown-linux-gnu --target aarch64-unknown-linux-gnu --target aarch64-apple-darwin --target x86_64-apple-darwin"
# One `<name> <dir>` per member.
members=$(cargo tree --manifest-path "$manifest" --workspace --depth 0 --prefix none -e normal | sed -n 's/^\([^ ]*\) v[^ ]* (\(.*\))$/\1 \2/p')
on=""
without=""
missing=""
own=""
while read -r member dir; do
  own="$own$(awk -v file="${dir#$root/}/Cargo.toml" '
    /^\[/ { section = $0 }
    /^serde_json[ .=]/ && section != "[workspace.dependencies]" && $0 !~ /workspace *= *true/ { printf "\n  %s: %s", file, $0 }
  ' "$dir/Cargo.toml")"
  graph=$(cargo tree --manifest-path "$manifest" -p "$member" $targets -e normal,dev,build --prefix none)
  if ! printf '%s\n' "$graph" | grep -q '^serde_json v'; then
    without="$without $member"
    continue
  fi
  features=$(cargo tree --manifest-path "$manifest" -p "$member" $targets -e features,normal,dev,build -i serde_json --depth 1 --prefix none)
  if printf '%s\n' "$features" | grep -qx 'serde_json feature "preserve_order"'; then
    on="$on $member"
  else
    missing="$missing $member"
  fi
done <<EOF
$members
EOF
if [ -n "$missing" ] || [ -n "$own" ]; then
  [ -z "$missing" ] || echo "serde_json builds without preserve_order for:$missing" >&2
  [ -z "$own" ] || echo "serde_json named outside the workspace dependency:$own" >&2
  echo "take it from the workspace: serde_json = { workspace = true }" >&2
  exit 1
fi
echo "serde_json preserve_order on for:$on; not in the graph of:$without"
