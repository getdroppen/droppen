// The browser's plugin host, held to the same storage contract as the SDK's
// in-memory test host: compare-and-set, all-or-nothing batches, conflicts that
// carry the current document, and failures told apart by `code`. The HTTP
// half runs the real typed client against a stubbed fetch that speaks
// docs/plugins/storage-protocol.md — so a path, an encoding or an error
// mapping that drifts from the protocol fails here.
import assert from "node:assert/strict";
import test from "node:test";
import { isStorageError, type Doc, type PluginStorage } from "@droppen/plugin-sdk";
import { createTestHost } from "@droppen/plugin-sdk/testing";
import { LOCAL } from "../src/lib/machines/key.js";

type Stored = Doc<unknown>;

/** An in-memory server for the document routes of one plugin. */
function protocolServer(plugin: string, enabled = () => true) {
  const docs = new Map<string, Stored>();
  const seen: string[] = [];
  const json = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  const conflict = (name: string) => json(409, { error: "conflict", message: "moved", doc: name, current: docs.get(name) ?? null });
  const write = (name: string, value: unknown, expected: number | null): Stored => {
    const doc = { name, revision: (expected ?? 0) + 1, updatedAt: "2026-10-17T00:00:00.000Z", value };
    docs.set(name, doc);
    return doc;
  };
  const handle = (method: string, url: string, body: unknown): Response => {
    const { pathname, searchParams } = new URL(url, "http://host.invalid");
    seen.push(`${method} ${pathname}${searchParams.size ? "?" + searchParams : ""}`);
    const prefix = `/api/plugins/${plugin}/`;
    if (!pathname.startsWith(prefix)) return json(404, { error: "plugin_not_found" });
    if (!enabled()) return json(404, { error: "plugin_disabled" });
    const rest = pathname.slice(prefix.length);
    if (rest === "docs" && method === "GET") {
      const p = searchParams.get("prefix") ?? "";
      return json(200, {
        docs: [...docs.values()].filter((d) => d.name.startsWith(p)).map(({ name, revision, updatedAt }) => ({ name, revision, updatedAt })),
      });
    }
    if (rest === "batch" && method === "POST") {
      const { writes } = body as { writes: { name: string; value: unknown; expectedRevision: number | null }[] };
      for (const w of writes) if ((docs.get(w.name)?.revision ?? null) !== w.expectedRevision) return conflict(w.name);
      return json(200, { docs: writes.map((w) => write(w.name, w.value, w.expectedRevision)) });
    }
    const m = /^docs\/([^/]+)$/.exec(rest);
    if (!m) return json(404, { error: "not found" });
    const name = decodeURIComponent(m[1]!);
    if (method === "GET") return docs.has(name) ? json(200, docs.get(name)) : json(404, { error: "doc_not_found" });
    if (method === "PUT") {
      const { expectedRevision, value } = body as { expectedRevision: number | null; value: unknown };
      if (JSON.stringify(value).length > 1000) return json(413, { error: "too_large", message: "too big", limit: 1000 });
      if ((docs.get(name)?.revision ?? null) !== expectedRevision) return conflict(name);
      return json(200, write(name, value, expectedRevision));
    }
    return json(405, { error: "method" });
  };
  return { docs, seen, handle };
}

/**
 * The typed client builds a `Request` from a root-relative URL, which Node's
 * own `Request` refuses; it reads `Request` and `fetch` when first imported,
 * so the stubs go in before the dynamic import (see session-account-bind).
 */
async function withServer(server: ReturnType<typeof protocolServer>, run: (storage: (id: string) => PluginStorage) => Promise<void>) {
  const realRequest = globalThis.Request;
  const realFetch = globalThis.fetch;
  class StubRequest {
    readonly url: string;
    readonly method: string;
    readonly headers: Headers;
    readonly bodyText: string;
    constructor(url: string, init: RequestInit = {}) {
      this.url = url;
      this.method = init.method ?? "GET";
      this.headers = new Headers(init.headers ?? {});
      this.bodyText = typeof init.body === "string" ? init.body : "";
    }
    clone() {
      return this;
    }
  }
  globalThis.Request = StubRequest as unknown as typeof Request;
  globalThis.fetch = (async (request: StubRequest) =>
    server.handle(request.method, request.url, request.bodyText ? JSON.parse(request.bodyText) : null)) as unknown as typeof fetch;
  try {
    const { httpStorage } = await import("../src/plugins/host.js");
    await run((id) => httpStorage(id, LOCAL));
  } finally {
    globalThis.Request = realRequest;
    globalThis.fetch = realFetch;
  }
}

/** What every host's storage must do. */
async function storageContract(storage: PluginStorage) {
  assert.equal(await storage.get("state"), null, "a missing document is null, not an error");
  const first = await storage.put("state", { n: 1 }, null);
  assert.equal(first.revision, 1);
  assert.deepEqual((await storage.get<{ n: number }>("state"))?.value, { n: 1 });
  await assert.rejects(storage.put("state", { n: 9 }, null), (e: unknown) => isStorageError(e, "conflict"), "null means must not exist");
  await assert.rejects(storage.put("state", { n: 9 }, 7), (e: unknown) => {
    assert.ok(isStorageError(e, "conflict"));
    assert.deepEqual((e as { current: Stored }).current.value, { n: 1 }, "a conflict carries the document as it is");
    return true;
  });
  // A name with a slash travels as one path segment.
  const month = await storage.put("events/2026-10", [1], null);
  assert.equal(month.name, "events/2026-10");
  assert.deepEqual((await storage.get("events/2026-10"))?.value, [1]);
  await assert.rejects(
    storage.batch([
      { name: "events/2026-11", value: [2], expectedRevision: null },
      { name: "state", value: { n: 2 }, expectedRevision: 5 },
    ]),
    (e: unknown) => isStorageError(e, "conflict"),
  );
  assert.equal(await storage.get("events/2026-11"), null, "a batch that conflicts writes nothing");
  const written = await storage.batch([
    { name: "events/2026-11", value: [2], expectedRevision: null },
    { name: "state", value: { n: 2 }, expectedRevision: 1 },
  ]);
  assert.deepEqual(
    written.map((d) => [d.name, d.revision]),
    [
      ["events/2026-11", 1],
      ["state", 2],
    ],
  );
  assert.deepEqual(
    (await storage.list("events/")).map((d) => d.name).sort(),
    ["events/2026-10", "events/2026-11"],
  );
}

test("the SDK's test host meets the storage contract", async () => {
  await storageContract(createTestHost().storage);
});

test("the browser host meets the storage contract over HTTP", async () => {
  const server = protocolServer("tasks");
  await withServer(server, async (storage) => {
    await storageContract(storage("tasks"));
    assert.ok(server.seen.includes("GET /api/plugins/tasks/docs/events%2F2026-10"), `encoded: ${server.seen.join(", ")}`);
    assert.ok(server.seen.includes("GET /api/plugins/tasks/docs?prefix=events%2F"));
  });
});

test("the browser host turns HTTP failures into the SDK's codes", async () => {
  let on = true;
  const server = protocolServer("tasks", () => on);
  await withServer(server, async (storage) => {
    const s = storage("tasks");
    await assert.rejects(s.put("big", "x".repeat(2000), null), (e: unknown) => isStorageError(e, "too_large") && (e as { limit: number }).limit === 1000);
    on = false;
    await assert.rejects(s.get("state"), (e: unknown) => isStorageError(e, "disabled"));
    await assert.rejects(storage("other").get("state"), (e: unknown) => isStorageError(e, "disabled"));
  });
  const { storageFailure } = await import("../src/plugins/host.js");
  const at = (status: number, error: unknown) => storageFailure("tasks", { response: new Response(null, { status }), error });
  assert.equal(at(503, { error: "machine_offline" }).code, "offline", "a machine that is not there is offline, whatever the hub calls it");
  assert.equal(at(500, { error: "boom" }).code, "failed");
  assert.equal(at(409, { error: "conflict", doc: "state", current: null }).code, "conflict");
});

test("watchers hear the feed's plugin-doc frames for their document, and every reconnect", async () => {
  const { httpStorage } = await import("../src/plugins/host.js");
  const { pluginEvents } = await import("../src/plugins/events.js");
  const heard: (number | null)[] = [];
  const stop = httpStorage("tasks", LOCAL).watch("state", (revision) => heard.push(revision));
  assert.equal(pluginEvents.takeFrame({ type: "plugin-doc", plugin: "tasks", name: "state", revision: 4 }), true);
  pluginEvents.takeFrame({ type: "plugin-doc", plugin: "tasks", name: "events/2026-10", revision: 2 });
  pluginEvents.takeFrame({ type: "plugin-doc", plugin: "other", name: "state", revision: 9 });
  pluginEvents.resync();
  assert.equal(pluginEvents.takeFrame({ type: "sessions:delta" }), false, "other frames are left for the feed");
  stop();
  pluginEvents.takeFrame({ type: "plugin-doc", plugin: "tasks", name: "state", revision: 5 });
  assert.deepEqual(heard, [4, null]);
});

test("plugin pages route by the longest matching prefix", async () => {
  const { matchRoute } = await import("../src/plugins/routing.js");
  const routes = [{ path: "" }, { path: "weekly" }, { path: "weekly/print" }];
  assert.equal(matchRoute(routes, "")?.path, "");
  assert.equal(matchRoute(routes, "anything/else")?.path, "");
  assert.equal(matchRoute(routes, "weekly")?.path, "weekly");
  assert.equal(matchRoute(routes, "weekly/2026-10-12")?.path, "weekly");
  assert.equal(matchRoute(routes, "weekly/print/")?.path, "weekly/print");
  assert.equal(matchRoute(routes, "weeklyish")?.path, "", "a prefix is a whole segment");
  assert.equal(matchRoute([{ path: "a" }], "b"), undefined);
});

test("old task panes read as the tasks plugin's checklist and are saved the old way", async () => {
  const { readPaneContent, storePaneContent, validPaneState } = await import("../src/plugins/pane-content.js");
  assert.deepEqual(readPaneContent({ kind: "tasks" }), { kind: "plugin", plugin: "tasks", pane: "checklist" });
  assert.deepEqual(readPaneContent({ kind: "files", path: "/" }), { kind: "files", path: "/" });
  assert.deepEqual(storePaneContent({ kind: "plugin", plugin: "tasks", pane: "checklist" }), { kind: "tasks" });
  assert.deepEqual(storePaneContent({ kind: "plugin", plugin: "tasks", pane: "checklist", state: { tab: "x" } }), {
    kind: "plugin",
    plugin: "tasks",
    pane: "checklist",
    state: { tab: "x" },
  });
  assert.deepEqual(storePaneContent({ kind: "plugin", plugin: "notes", pane: "list" }), { kind: "plugin", plugin: "notes", pane: "list" });
  assert.ok(validPaneState({ a: 1, b: "x", c: null, d: true }));
  assert.ok(!validPaneState({ nested: { a: 1 } }));
  assert.ok(!validPaneState({ big: "x".repeat(3000) }));
  assert.ok(!validPaneState([1]));
});

test("a plugin can print in a browser on the server, but not in the desktop app's window", async () => {
  // wry 0.57's WKWebView UI delegate implements no print callback: the app's
  // window reports a print function that does nothing. A browser on the same
  // server prints, and `isDesktopApp()` is true there too — the app's own
  // marker (`window.__droppenDesktop`, desktop/src-tauri/src/init.js) decides.
  const { createPluginHost } = await import("../src/plugins/host.js");
  const hostOf = () =>
    createPluginHost("tasks", { navigate: () => undefined, machine: LOCAL, storage: createTestHost().storage });
  const setWindow = (value: unknown) => Object.defineProperty(globalThis, "window", { configurable: true, writable: true, value });
  try {
    setWindow({ print: () => undefined, isSecureContext: false });
    assert.equal(hostOf().capabilities.print, true, "a browser on the server prints");
    setWindow({ print: () => undefined, isSecureContext: false, __droppenDesktop: {} });
    assert.equal(hostOf().capabilities.print, false, "the app's window cannot print");
    setWindow({ isSecureContext: false });
    assert.equal(hostOf().capabilities.print, false, "no print function at all");
  } finally {
    delete (globalThis as Record<string, unknown>).window;
  }
});
