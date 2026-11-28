# Plugins on droppen.app

**Status:** Implemented. Plugins opted in via `PUT /api/plugins/{id}/cloud` (Settings › Plugins or `plugin cloud <id> on`) run on `droppen.app` inside a sandboxed `/plugin-frame.html` iframe with all requests forwarded back to the plugin's machine over the end-to-end encrypted relay.

## Mental model

On `droppen.app`, the browser is a pure display for a remote machine's plugins:

- The plugin's code and data live on the user's machine and travel to the browser over the end-to-end encrypted relay — the hub never sees them.
- The cloud page renders each plugin view (page, Screen pane, settings) inside a sandboxed `<iframe sandbox="allow-scripts" src="/plugin-frame.html">` with an opaque origin and `connect-src 'none'`.
- Every action inside the frame — `host.storage`, `fetch()`, `XMLHttpRequest`, UI confirmations, file saves, clipboard writes, and navigation — crosses the frame's `MessageChannel` to the parent page, which forwards HTTP/storage calls over the E2E relay to **that plugin's machine** for execution.

## Why not the way a local page does it

A page the instance serves imports a plugin's code into itself ([runtime.md](runtime.md) §6). The plugin runs with that page's rights, which are the rights of the user on that one machine (§8).

A page on `droppen.app` holds much more:

- this browser's device keys (WebCrypto's non-extractable keys stop export, not use);
- the hub session;
- a channel to every machine of the account;
- on an admin device, the power to change the device chain.

A plugin imported directly into `my.droppen.app` could control every other machine of the account and add a device to it. The app's CSP allows only `'self'` scripts (`web/build/cloud-mode.ts`), and `scripts/verify-web.mjs` holds every page to that ([web-integrity.md](../web-integrity.md)). That does not change.

So `droppen.app` runs a plugin only inside an opaque-origin frame that holds none of those credentials, and stamps every request forwarded from that frame with `X-Rh-Plugin: <pluginId>` for the single machine it belongs to.

## 1. The switch: show on droppen.app

The switch is per plugin and per instance, and it is **off** until someone switches it on locally. Neither the hub nor `droppen.app` can change it.

- `PluginStatus.cloud` is the instance's decision. It takes effect only while the plugin is `enabled` and `compatible`.
- `PUT /api/plugins/{id}/cloud` with `{ cloud: boolean }` answers `200 PluginStatus`, or `404 plugin_not_found`. Only local callers may call it; the bridge refuses it as `not_from_cloud`.
- `remote-hand-server plugin cloud <id> on|off [--instance <name>]` sets it from the CLI, and `plugin list` shows it.
- Settings › Plugins on the machine's local page exposes a "Show on droppen.app" switch with a confirmation dialog explaining the trust model.
- The switch is kept in `state.db` next to `enabled` and outlives upgrades and uninstalls. `[plugins] enable/disable` pins on/off only, not `cloud`.

## 2. The frame

`droppen.app` draws each plugin page and Screen pane in an `<iframe sandbox="allow-scripts" src="/plugin-frame.html">`, a static page of the same reproducible build.

- The frame never gets `allow-same-origin`, `allow-popups`, `allow-top-navigation`, `allow-forms`, `allow-modals`, or `allow-downloads`, and gets no `allow="clipboard-write"`.
- Every frame has its own opaque origin (`null`), isolating it from `my.droppen.app` and from every other plugin frame.
- `/plugin-frame.html` is served with its own strict CSP (in `web/build/cloud-mode.ts` `_headers` and `hub/crates/rh-hub/src/web.rs`):
  `sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline' blob:; style-src 'unsafe-inline' blob:; img-src blob: data:; font-src blob: data:; connect-src 'none'; frame-ancestors 'self'`
- The app's own CSP allows `frame-src 'self'` (alongside Turnstile on `app` hosts), and nothing else.

## 3. Getting the code into the frame

- `GET /api/plugins/{id}/package` returns the installed package's files (`utf8` for text/JS/CSS/JSON/SVG, `base64` for binary assets, capped at 32 MiB) for a plugin that is `enabled && compatible && cloud`, traveling over the E2E relay.
- The pre-bundled frame runtime (`assets/plugin-frame-runtime-<hash>.js`, covered by `sw.js`'s integrity table and `web-manifest.json`) holds React, `react-dom`, `react/jsx-runtime`, `@droppen/plugin-sdk`, the `fetch`/`XMLHttpRequest` shims, and the `blob:` module loader.
- The parent transfers a `MessagePort`, the runtime source, and the package bundle to `/plugin-frame.html`. Inside the frame, relative ESM imports are rewritten to `@droppen-pkg/<path>` specifiers (`web/src/plugins/frame/rewrite-imports.ts`), a runtime `<script type="importmap">` maps them plus the shared React/SDK facades to `blob:` URLs, CSS files are injected as `blob:` stylesheets, and the entry module is imported.

## 4. `host.*` and HTTP forwarding through the page

Every call crosses the frame's `MessagePort`, bound by the parent to `(machine, pluginId)`:

| Surface | On droppen.app |
|---|---|
| `host.storage` | That machine's `/api/plugins/{id}/docs…` and `/batch` over the E2E relay with `X-Rh-Plugin: <id>`. `watch` follows that machine's `plugin-doc` frames. |
| Relative `fetch` / `XMLHttpRequest` (`/api/...`) | Forwarded over the E2E relay to that machine's API with `Authorization`, `Cookie`, `Host`, and `X-Rh-*` stripped from plugin headers and `X-Rh-Plugin: <id>` stamped by the parent. |
| External `fetch` / `XMLHttpRequest` (`http(s)://...`) | Executed on the plugin's machine via `POST /api/plugins/{id}/fetch` (no CORS, LAN/VPN reachable), returning status, headers, and body over the E2E relay. |
| `host.ui.confirm` | The parent page's confirmation dialog, naming both plugin and machine in the title. |
| `host.ui.Dialog`, `host.ui.Spinner` | Drawn inside the frame by the runtime using the host's synced `--color-*` and `--font-*` CSS variables. |
| `host.files.save` | Sent over the port; the parent triggers a download. |
| `host.files.open` | `<input type="file">` inside the frame. |
| `host.clipboard.writeText` | Sent over the port; the parent writes to the clipboard. |
| `host.print` | `capabilities.print` is `false`. |
| `host.navigation.navigate` | Scoped to `/p/<pluginId>/…` (or `/m/<machine>/p/<pluginId>/…`) on that machine. |
| `host.navigation.useLeaveGuard` | Reported to the parent router's `useBlocker`. |
| `host.i18n` | Synced from the parent at boot and on language changes. |

## 5. In the page & Screens

- `activate()` runs once per `(machine, id, build)` inside a hidden sandboxed frame to discover serializable `nav`, `routes`, `panes`, `settings`, and `i18n` contributions.
- Outside every visible frame, the parent renders an uncoverable provenance banner naming the plugin, its version, and the machine it runs on.
- Plugin pages are routed at `/p/$plugin/$` (home machine) and `/m/$machine/p/$plugin/$` (any other machine).
- In **Screens (multi-pane layouts)**, plugin panes carry an optional `machine` field so panes from any machine can be arranged side by side, both locally and on `droppen.app`. Plugins that only define `routes`/`nav` without explicit `contributions.panes` are also available in Screens via the reserved pane id `"droppen-page"`.
- Aliases remain local-only.

## 6. Server, feed and hub

- **The bridge** (`crates/droppen-agent/src/bridge.rs`) forwards `X-Rh-Plugin` and classifies the cloud-capable plugin routes as `Use`: `GET /api/plugins`, `GET /api/plugins/{id}/package`, `POST /api/plugins/{id}/fetch`, `GET /api/plugins/{id}/docs`, `GET`/`PUT /api/plugins/{id}/docs/{name}` (including percent-encoded `/` in doc names), and `POST /api/plugins/{id}/batch`. Installing, uninstalling, `…/enabled`, and `…/cloud` stay `Never` (`not_from_cloud`).
- **The auth guard & cloud plugin layer** (`droppen-auth`, `droppen_plugins::routes::cloud_plugin_layer`): `droppen-auth` extracts `X-Rh-Plugin` on authenticated bridged requests into a `PluginCaller` extension and strips the header on every request. `cloud_plugin_layer` checks that any bridged request carrying `PluginCaller(id)` belongs to a plugin that is currently `enabled && cloud`, rejecting revoked or non-cloud plugins immediately with `404 plugin_not_found`.
- **The handlers** serve `cloud:` callers only for plugins that are `enabled && cloud` (answering `404 plugin_not_found` for all others). `GET /api/plugins` returns only those plugins with `entry: null`. `cloud:agent-sync` receives `[]` and is rejected on plugin endpoints so the offline hub copy never carries plugin status or documents.
- **The asset route** `/plugins/<key>/…` is never bridged.
- **The feed** filters `settings.plugins`, `plugins`, and `plugin-doc` frames for `cloud:` subscribers to `enabled && cloud` plugins (with `entry: null`).
- **Headers & integrity verification.** `_headers`, `sw.js`, and `hub/crates/rh-hub/src/web.rs` serve `/plugin-frame.html` with the sandbox CSP above and add `frame-src 'self'` to the app's CSP. `scripts/verify-web.mjs` verifies `/plugin-frame.html`'s CSP via `pluginFramePolicy` (requiring `sandbox` without `allow-same-origin`, `connect-src 'none'`, `object-src`/`default-src 'none'`, and `frame-ancestors 'self'|'none'`).

## 7. What it does not stop

- **Phishing inside the frame.** A plugin can draw a fake prompt inside its own iframe. The provenance banner outside the frame (§5) and the parent-owned `ui.confirm` title are the defences.
- **Reach of a cloud-enabled plugin on its own machine.** Because a cloud-enabled plugin's relative `/api/...` and external `POST /api/plugins/{id}/fetch` calls execute on its machine, turning on "Show on droppen.app" grants the same trust on **that one machine** as installing the plugin locally — while still preventing any plugin from reaching `my.droppen.app`'s device keys, hub session, device chain, or any other machine.
- **A busy frame** can use the tab's CPU, as any page can.
