# Deployment E2E

Black-box tests for the deployment shape of `api.droppen.app`: real TLS,
several hosts, a fake hub, and three browsers. They catch what unit tests
cannot — the wrong marker or API origin, a missing CORS header, a static
`_headers` policy that re-enables WebAuthn on `www`, a worker bound to the
wrong host, an old machine handshake the apex no longer accepts.

The companion without a browser is `scripts/e2e-deploy.mjs` (read-only checks
against real hosts; see the repository README's scripts or run
`node scripts/e2e-deploy.mjs --help`).

## What runs

| host | served | what the specs prove |
|---|---|---|
| `my.droppen.test` | the built web app (`web/dist`) | marker `wss://api.droppen.test`, cross-origin sign-up config, device session, `/v1/me`, `/v1/client` socket, the real service worker |
| `www.droppen.test` | the site fixture | no marker, no API in `connect-src`, `Permissions-Policy` denies WebAuthn, no keys/worker, app links absolute to `my.` |
| `droppen.test` | the apex interstitial | moved page, no keys/worker, a machine handshake naming the apex accepted by the hub (the temporary alias) |
| `api.droppen.test` | the fake hub | CORS allowlist, `/v1/signup/config`, `/v1/session/challenge`, `/v1/session`, `/v1/me`, `wss://…/v1/client`, `wss://…/v1/agent` |
| `evil.test` | an unrelated page | an authenticated response is not readable there |

`www.` and the apex are fixtures: the site role's real build does not exist
yet. They already carry the target headers and links; `siteFixture` in
`scripts/build-fixtures.mjs` is what a real site build replaces.

`my.` is the real build. When the tree has `web/build/cloud-mode.ts`, prepare
builds it with `DROPPEN_WEB_MODE=cloud` (the marker, `/.well-known/droppen-hub`
and `_headers` come from that build). Until that lands on `main`,
`build-fixtures.mjs` adds the same three things and a small fetch shim that
sends the app's same-origin `/v1/*` calls to `https://api.droppen.test` — the
cross-origin API base the cloud build derives from the marker.

## Docker (what CI runs)

From the repository root:

```sh
docker compose -f e2e/docker-compose.yml up --build --abort-on-container-exit --exit-code-from e2e
```

The runner is Playwright's own image (browsers included, no cache step). One
container is both the HTTPS listener and the browsers; `extra_hosts` maps every
host name to `127.0.0.1`, so no Chromium resolver flags and no `/etc/hosts`
edits are needed. Port 443 is bound inside the container, as root.

## Local (no Docker)

Add the names to `/etc/hosts`:

```
127.0.0.1 api.droppen.test my.droppen.test www.droppen.test droppen.test evil.test
```

Then, from `e2e/`:

```sh
npx pnpm@10.6.3 install --frozen-lockfile
# the HTTPS listener binds 443, so it needs root; the tests do not:
sudo node scripts/serve.mjs          # terminal A
E2E_EXTERNAL_SERVERS=1 pnpm exec playwright test   # terminal B
```

`sudo node scripts/serve.mjs` prepares the fixtures and the throwaway
certificate first. `E2E_SKIP_BUILD=1` reuses an existing `web/dist` (useful
while iterating); otherwise a normal or cloud build runs.

## Mutation acceptance

The suite must fail when the deployment is broken. With the mutation name in
the environment, the fixtures (or the fake hub's allowlist) are deliberately
wrong, and the named spec must go red:

```sh
node scripts/mutation.mjs                 # all four, chromium
node scripts/mutation.mjs --only cors
```

- `marker` — the app's marker names `wss://api.droppen.invalid` (`app.spec.ts`).
- `cors` — the hub's allowlist drops `https://my.droppen.test` (`app.spec.ts`).
- `site-policy` — `www.` sends `publickey-credentials-get=(self)` (`site.spec.ts`).
- `api-base` — the app's API base is `https://api.droppen.invalid` (`app.spec.ts`).

The same environment variable works with the Docker runner:
`E2E_MUTATION=cors docker compose -f e2e/docker-compose.yml up --build ...`.

## Logs and artifacts

- `.e2e/hub-log.jsonl` — one JSON line per fake-hub request and socket event;
  the specs poll it, so "the page called `api.`" is observed, not assumed.
- `test-results/`, `playwright-report/` — written by Playwright (gitignored).
- `.e2e/` — fixtures and the throwaway key/certificate (gitignored).
