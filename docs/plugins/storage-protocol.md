# Plugin document store — HTTP protocol (SDK 0.1)

The one server-side extension point of SDK 0.1. Every plugin gets a private
document store per instance; the host implements it, no plugin ships server
code. Licensed with the SDK (MIT OR Apache-2.0). Wire types live in
`server-rs/crates/droppen-plugin-sdk` and `packages/plugin-sdk`; this page is
what they describe.

All routes are behind the instance's normal authentication. JSON is camelCase.
Errors are `{ "error": <code>, "message"?: string, ... }`.

## Plugins

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/plugins` | — | `200 { plugins: PluginStatus[] }` — every plugin this build bundles |
| `PUT /api/plugins/{id}/enabled` | `{ enabled: boolean }` | `200 PluginStatus`; `404 plugin_not_found`; `409 forced_by_config` |

```ts
interface PluginStatus {
  id: string;            // ^[a-z][a-z0-9-]{1,31}$
  version: string;       // the bundled package version
  name: LocalizedText;   // string | { en, "zh-CN"?, "zh-TW"?, ja? }
  enabled: boolean;      // on for this instance AND compatible
  compatible: boolean;   // the host satisfies engines.droppen
  forced: boolean;       // the configuration pins it; the switch is read-only
  reason: string | null; // why it is off / incompatible / pinned
  notice: string | null; // every notice's text, joined
  notices: PluginNotice[]; // one by one, oldest first
}

interface PluginNotice {
  id: string;            // stable while the notice exists
  kind: string;          // storeReset
  text: string;
  at: string;
  dismissible: boolean;  // a store reset is not: it goes when its files are cleared
}
```

Notices are announced to open clients with a `plugins` frame. The shell lists
them above the plugin's page and flags them in its panes. Today the only one
is `storeReset`, computed from the `store.corrupt-<time>` files beside a store
that had to be set aside; it does not stop the plugin working, cannot be
dismissed, and goes away when those files are cleared.

`GET /api/settings` and the global feed's `settings` frame carry the same list
as `plugins`. After a switch changes, the feed sends
`{ "type": "plugins", "plugins": PluginStatus[] }`.

Who may switch: any signed-in user of the instance (there are no roles). The
configuration wins over the switch: `[plugins] enable = "a,b"` / `disable =
"c"`, or `APP_PLUGINS__ENABLE` / `APP_PLUGINS__DISABLE`; `disable` beats
`enable`. Without either, the instance's own decision (kept in its `state.db`),
else the distribution's `defaultEnabled`, else off.

## Documents

Every route below answers `404 { error: "plugin_disabled" }` when the plugin is
bundled but not enabled (or incompatible), and `404 { error:
"plugin_not_found" }` when it is not bundled at all. Nothing is created on
disk for a plugin that is not enabled.

Document names: 1-200 characters of `A-Z a-z 0-9 . _ -` in `/`-separated
segments, none empty, `.` or `..` (`state`, `events/2026-10`). In a URL the
name is one path segment, so `/` is sent as `%2F`.

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/plugins/{id}/docs?prefix=` | — | `200 { docs: DocMeta[] }`, by name |
| `GET /api/plugins/{id}/docs/{name}` | — | `200 Doc`; `404 doc_not_found` |
| `PUT /api/plugins/{id}/docs/{name}` | `{ expectedRevision: number \| null, value }` | `200 Doc`; `409 Conflict`; `413 too_large` |
| `POST /api/plugins/{id}/batch` | `{ writes: [{ name, expectedRevision, value }] }` | `200 { docs: Doc[] }`; `409 Conflict`; `413 too_large` |
| `GET /api/plugins/{id}/docs/{name}/history?offset=&limit=` | — | `200 { versions: DocVersion[], more: boolean }`, newest first, one page |

```ts
interface Doc<T> { name: string; revision: number; updatedAt: string; value: T }
interface DocMeta { name: string; revision: number; updatedAt: string }
interface Conflict<T> { error: "conflict"; message: string; doc: string; current: Doc<T> | null }
interface DocVersion<T> { revision: number; updatedAt: string; updatedBy: string | null; value: T }
```

- **Compare-and-set.** `expectedRevision: null` means "must not exist yet";
  a number means "must be exactly this revision". A successful write sets the
  revision to `expectedRevision + 1` (1 for a new document). Anything else is
  `409` with the document as it is now (`null` when it does not exist).
- **Batch.** One SQLite transaction: every write is checked first; if any one
  conflicts, nothing is written and the `409` names the first. A name may
  appear once per batch.
- **Limits.** One document's value ≤ 4 MiB of JSON; one request ≤ 8 MiB; one
  plugin's whole store ≤ 256 MiB — live documents **and** their history,
  names included. History gives way first: when a write would go over, the
  oldest past versions (of any document) are forgotten; only live documents
  that alone exceed the limit refuse a write. Over a limit: `413 { error:
  "too_large", message, limit }`.
- **History.** Every write keeps the value it replaced; per document the newest
  50 versions are kept, and at most 16 MiB of them. It is a safety net for a
  plugin's own bugs, not an API to build features on. A page is `limit`
  versions (10 by default, at most 50) after skipping `offset`, cut short once
  their values pass 8 MiB (the first is always there); `more` says whether
  older ones remain.
- **Feed.** After every committed write, the global feed (`/api/events`) sends
  one frame per written document: `{ "type": "plugin-doc", "plugin": <id>,
  "name": <doc>, "revision": <n> }`. That is what `host.storage.watch` listens
  to. After a reconnect the client re-reads what it watches.
- **No deletes** in 0.1. A plugin that needs one writes a tombstone value.

## On disk

`<data>/instances/<instance>/plugins/<id>/store.db`, one SQLite file per plugin
per instance (WAL, `synchronous = FULL`, `0600`), created the first time an
enabled plugin's store is used:

```sql
docs(name TEXT PRIMARY KEY, revision INTEGER, value TEXT, updated_at TEXT, updated_by TEXT)
doc_history(name TEXT, revision INTEGER, value TEXT, updated_at TEXT, updated_by TEXT)
meta(key TEXT PRIMARY KEY, value TEXT)
```

The host never interprets a value. Disabling a plugin keeps its directory;
`remote-hand-server plugin purge <id> --yes` moves it aside to
`plugins/<id>.purged-<time>/` rather than deleting it.

## CLI

```
remote-hand-server plugin list                     [--instance <name>]
remote-hand-server plugin enable  <id>             [--instance <name>]
remote-hand-server plugin disable <id>             [--instance <name>]
remote-hand-server plugin export  <id> [--out <file>]
remote-hand-server plugin import  <id> <file> [--replace]
remote-hand-server plugin verify  <id>
remote-hand-server plugin purge   <id> --yes
```

Safe while the server runs (SQLite arbitrates between processes). `export`
writes `{ format: "droppen.plugin-store.v1", plugin, exportedAt, docs: Doc[] }`;
`import` reads the same.
