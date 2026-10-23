#!/usr/bin/env bash
# The browser key move on two real hosts (see run.mjs): build the pages,
# make a throwaway certificate for the three host names, start the server,
# run the browsers. From anywhere:
#
#   web/e2e/move/run.sh [chromium,firefox,webkit]
#
# Needs: droppen.test, my.droppen.test and evil.test resolving to 127.0.0.1
# (/etc/hosts), `pnpm install` done in the workspace, openssl, and Playwright
# 1.64.1 with its browsers (`npx playwright@1.64.1 install --with-deps`);
# PLAYWRIGHT_MODULE=<…/node_modules/playwright/index.mjs> when it is not
# installed in web/. MOVE_PORT: the port to serve on (8443).
set -euo pipefail
cd "$(dirname "$0")/../.."
port=${MOVE_PORT:-8443}
tmp=$(mktemp -d)
server=
cleanup() {
  # The server's whole process group (pnpm, tsx and the node it starts).
  [ -n "$server" ] && kill -- "-$server" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=droppen.test \
  -addext "subjectAltName=DNS:droppen.test,DNS:my.droppen.test,DNS:evil.test" \
  -keyout "$tmp/key.pem" -out "$tmp/cert.pem" 2>/dev/null
pnpm exec vite build --config e2e/move/vite.config.ts --logLevel warn
setsid pnpm exec tsx e2e/move/server.ts "$port" "$tmp/cert.pem" "$tmp/key.pem" >"$tmp/server.log" 2>&1 &
server=$!
for _ in $(seq 1 100); do
  curl -sfk --resolve "droppen.test:$port:127.0.0.1" "https://droppen.test:$port/__test/state" >/dev/null && break
  sleep 0.2
done
MOVE_PORT=$port node e2e/move/run.mjs "${1:-chromium,firefox,webkit}"
