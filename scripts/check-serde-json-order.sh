#!/bin/sh
# serde_json's `preserve_order` is on for the whole server-rs workspace
# (`[workspace.dependencies]` in server-rs/Cargo.toml says why). Cargo resolves
# features for the packages a build selects, so every member whose dependency
# graph reaches serde_json has to turn the feature on itself; otherwise
# `cargo test -p <member>` writes JSON keys in another order than a workspace
# build does. So two checks, for every member but the exempt ones below:
# - each serde_json it names, in whatever form (`serde_json = …`, a
#   `[dependencies.serde_json]` table, a renamed `json = { package =
#   "serde_json" }`), carries the feature: `serde_json = { workspace = true }`;
# - when its graph reaches serde_json through other crates only, a build of it
#   alone still has the feature.
# The targets are the ones this workspace is built for: macOS (releases,
# development) and Linux (CI, relays); `--target all` would also count
# reqwest's wasm-only use of serde_json.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
manifest="$root/server-rs/Cargo.toml"
targets="--target x86_64-unknown-linux-gnu --target aarch64-unknown-linux-gnu --target aarch64-apple-darwin --target x86_64-apple-darwin"

# Members that keep serde_json as it comes, each with why:
# - droppen-plugin-sdk: plugin authors build against it, and the workspace's
#   serde_json would turn `preserve_order` on in their builds too. Its tests
#   compare `Value`s, which ignore key order.
exempt="droppen-plugin-sdk"
is_exempt() {
  case " $exempt " in *" $1 "*) return 0 ;; esac
  return 1
}

# Every serde_json a member names without the feature: `<member> <name>`.
# `cargo metadata` gives the package each dependency is, whatever the manifest
# calls it, and the features it has after workspace inheritance.
declared=$(cargo metadata --manifest-path "$manifest" --no-deps --format-version 1 | node -e '
let text = "";
process.stdin.on("data", (chunk) => (text += chunk)).on("end", () => {
  const meta = JSON.parse(text);
  const members = new Set(meta.workspace_members);
  for (const pkg of meta.packages.filter((p) => members.has(p.id))) {
    for (const dep of pkg.dependencies) {
      if (dep.name === "serde_json" && !dep.features.includes("preserve_order")) {
        console.log(`${pkg.name} ${dep.rename ?? dep.name}`);
      }
    }
  }
});')
named=""
while read -r member name; do
  [ -n "$member" ] || continue
  is_exempt "$member" || named="$named $member ($name)"
done <<EOF
$declared
EOF

members=$(cargo tree --manifest-path "$manifest" --workspace --depth 0 --prefix none -e normal | sed -n 's/^\([^ ]*\) v.*/\1/p')
on=""
without=""
missing=""
for member in $members; do
  if is_exempt "$member"; then
    continue
  fi
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
done
if [ -n "$named" ] || [ -n "$missing" ]; then
  [ -z "$named" ] || echo "serde_json named without preserve_order by:$named" >&2
  [ -z "$missing" ] || echo "serde_json builds without preserve_order for:$missing" >&2
  echo "take it from the workspace: serde_json = { workspace = true }" >&2
  exit 1
fi
echo "serde_json preserve_order on for:$on; not in the graph of:$without; exempt: $exempt"
