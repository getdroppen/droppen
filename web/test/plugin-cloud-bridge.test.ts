// Parent-side cloud plugin bridge (`web/src/plugins/cloud-bridge.ts`) and
// multi-machine / page-as-pane plugin pane helpers (`plugins/pane-content.ts`,
// `app/pane-tree.ts`).
import assert from "node:assert/strict";
import test from "node:test";
import { PluginStorageError } from "@droppen/plugin-sdk";
import { onMachine, paneMachine, withMachine, type PaneContent } from "../src/app/pane-tree.js";
import {
  base64ToArrayBuffer,
  bytesToBase64,
  forwardFrameHttp,
  handleFrameStorage,
  serializeError,
  type FetchTransport,
} from "../src/plugins/cloud-bridge.js";
import { PLUGIN_PAGE_PANE_ID, storePaneContent } from "../src/plugins/pane-content.js";

function mockTransport(handler: (req: Request) => Promise<Response> | Response): FetchTransport {
  return {
    fetch: async (req) => handler(req),
  };
}

test("relative /api/... calls strip forged auth/bridge headers and stamp X-Rh-Plugin", async () => {
  let seen: Request | null = null;
  let seenBody = "";
  const transport = mockTransport(async (req) => {
    seen = req;
    seenBody = await req.text();
    return new Response(JSON.stringify({ ok: true }), {
      status: 201,
      statusText: "Created",
      headers: { "content-type": "application/json", "x-reply": "yes" },
    });
  });

  const payload = new TextEncoder().encode('{"hello":"world"}').buffer;
  const res = await forwardFrameHttp(
    "m-1",
    "tasks",
    {
      url: "/api/sessions?limit=5",
      method: "POST",
      headers: [
        ["Authorization", "Bearer stolen"],
        ["Cookie", "sid=evil"],
        ["X-Rh-Agent", "forged"],
        ["X-Rh-Agent-Device", "other"],
        ["X-Rh-Plugin", "other-plugin"],
        ["Content-Type", "application/json"],
        ["X-Custom", "kept"],
      ],
      body: payload,
    },
    undefined,
    transport,
  );

  assert.ok(seen);
  assert.equal(new URL(seen!.url).pathname + new URL(seen!.url).search, "/api/sessions?limit=5");
  assert.equal(seen!.headers.get("authorization"), null);
  assert.equal(seen!.headers.get("cookie"), null);
  assert.equal(seen!.headers.get("x-rh-agent"), null);
  assert.equal(seen!.headers.get("x-rh-agent-device"), null);
  assert.equal(seen!.headers.get("x-rh-plugin"), "tasks");
  assert.equal(seen!.headers.get("x-custom"), "kept");
  assert.equal(seenBody, '{"hello":"world"}');

  assert.equal(res.status, 201);
  assert.equal(new TextDecoder().decode(res.body!), '{"ok":true}');
  assert.ok(res.headers.some(([k, v]) => k === "x-reply" && v === "yes"));
});

test("external http(s):// calls are proxied via POST /api/plugins/{id}/fetch with base64 body", async () => {
  let seenUrl = "";
  let seenPluginHeader = "";
  let seenJson: Record<string, unknown> | null = null;
  const transport = mockTransport(async (req) => {
    seenUrl = new URL(req.url).pathname;
    seenPluginHeader = req.headers.get("x-rh-plugin") ?? "";
    seenJson = (await req.json()) as Record<string, unknown>;
    return new Response(
      JSON.stringify({
        status: 200,
        statusText: "OK",
        url: "http://192.168.1.5:9090/status",
        headers: [{ name: "content-type", value: "text/plain" }],
        bodyBase64: bytesToBase64(new TextEncoder().encode("lan-ok")),
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  });

  const body = new TextEncoder().encode("ping").buffer;
  const res = await forwardFrameHttp(
    "m-1",
    "monitor",
    {
      url: "http://192.168.1.5:9090/status",
      method: "POST",
      headers: [["X-Token", "abc"]],
      body,
    },
    undefined,
    transport,
  );

  assert.equal(seenUrl, "/api/plugins/monitor/fetch");
  assert.equal(seenPluginHeader, "monitor");
  assert.equal(seenJson!.url, "http://192.168.1.5:9090/status");
  assert.equal(seenJson!.method, "POST");
  assert.equal(new TextDecoder().decode(base64ToArrayBuffer(String(seenJson!.bodyBase64))), "ping");

  assert.equal(res.status, 200);
  assert.equal(new TextDecoder().decode(res.body!), "lan-ok");
});

test("non-http(s) and protocol-relative URLs are refused", async () => {
  const transport = mockTransport(() => new Response("nope"));
  for (const url of ["//evil.example/api", "javascript:alert(1)", "file:///etc/passwd", "ftp://host/file"]) {
    await assert.rejects(
      forwardFrameHttp("m-1", "tasks", { url, method: "GET", headers: [], body: null }, undefined, transport),
    );
  }
});

test("handleFrameStorage stamps X-Rh-Plugin, handles 404 doc_not_found, and preserves conflict details", async () => {
  const seenHeaders: string[] = [];
  const transport = mockTransport(async (req) => {
    seenHeaders.push(req.headers.get("x-rh-plugin") ?? "");
    const path = new URL(req.url).pathname;
    if (path.endsWith("/docs/missing")) {
      return new Response(JSON.stringify({ error: "doc_not_found" }), {
        status: 404,
        headers: { "content-type": "application/json" },
      });
    }
    if (req.method === "PUT") {
      return new Response(
        JSON.stringify({
          error: "conflict",
          message: "stale revision",
          doc: "state",
          current: { name: "state", revision: 5, updatedAt: "2026-01-01T00:00:00Z", value: { ok: 1 } },
        }),
        { status: 409, headers: { "content-type": "application/json" } },
      );
    }
    return new Response(JSON.stringify({ docs: [{ name: "state", revision: 1, updatedAt: "2026-01-01T00:00:00Z" }] }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  });

  assert.equal(await handleFrameStorage("m-1", "tasks", "storage:get", { name: "missing" }, transport), null);
  const list = (await handleFrameStorage("m-1", "tasks", "storage:list", { prefix: "" }, transport)) as unknown[];
  assert.equal(list.length, 1);

  await assert.rejects(
    handleFrameStorage("m-1", "tasks", "storage:put", { name: "state", value: {}, expectedRevision: 1 }, transport),
    (err: unknown) => {
      assert.ok(err instanceof PluginStorageError);
      const serialized = serializeError(err);
      assert.equal(serialized.kind, "storage");
      if (serialized.kind === "storage") {
        assert.equal(serialized.code, "conflict");
        assert.equal(serialized.doc, "state");
      }
      return true;
    },
  );
  assert.deepEqual(seenHeaders, ["tasks", "tasks", "tasks"]);
});

test("plugin panes are machine-scoped and support page-as-pane (Issue #299)", () => {
  const pane: PaneContent = { kind: "plugin", plugin: "notes", pane: PLUGIN_PAGE_PANE_ID, state: { subpath: "inbox" } };
  assert.equal(onMachine(pane), true);
  assert.equal(paneMachine(pane, "home"), "home");

  const onOther = withMachine(pane, "dev-box", "home");
  assert.equal(paneMachine(onOther, "home"), "dev-box");
  assert.equal(onOther.kind === "plugin" && onOther.machine, "dev-box");

  // Legacy tasks checklist compresses to { kind: "tasks" } only on home with empty state.
  assert.deepEqual(storePaneContent({ kind: "plugin", plugin: "tasks", pane: "checklist" }), { kind: "tasks" });
  assert.deepEqual(
    storePaneContent({ kind: "plugin", plugin: "tasks", pane: "checklist", machine: "dev-box" }),
    { kind: "plugin", plugin: "tasks", pane: "checklist", machine: "dev-box" },
  );
});
