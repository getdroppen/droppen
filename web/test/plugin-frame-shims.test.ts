// Request shims (`web/src/plugins/frame/shims.ts`): `fetch` and `XMLHttpRequest`
// inside the sandboxed plugin iframe route over the MessagePort transport, while
// `blob:` / `data:` URLs stay on native `fetch` and synchronous XHR is refused.
import assert from "node:assert/strict";
import test from "node:test";
import type { HttpRequestParams, HttpResponseResult } from "../src/plugins/frame/protocol.js";
import { createShimmedFetch, createShimmedXHR } from "../src/plugins/frame/shims.js";

test("shimmed fetch forwards relative and external URLs over the port transport and handles 204", async () => {
  const seen: HttpRequestParams[] = [];
  const shimmed = createShimmedFetch(async (params): Promise<HttpResponseResult> => {
    seen.push(params);
    if (params.method === "DELETE") {
      return { status: 204, statusText: "No Content", url: params.url, headers: [], body: null };
    }
    return {
      status: 200,
      statusText: "OK",
      url: params.url,
      headers: [["content-type", "application/json"]],
      body: new TextEncoder().encode(JSON.stringify({ echo: new TextDecoder().decode(params.body ?? new ArrayBuffer(0)) })).buffer,
    };
  });

  const post = await shimmed("/api/demo?x=1", {
    method: "POST",
    headers: { "X-Test": "1" },
    body: "payload",
  });
  assert.equal(post.status, 200);
  assert.deepEqual(await post.json(), { echo: "payload" });
  assert.equal(seen[0]!.url, "/api/demo?x=1");
  assert.equal(seen[0]!.method, "POST");
  assert.ok(seen[0]!.headers.some(([k, v]) => k.toLowerCase() === "x-test" && v === "1"));

  const del = await shimmed("https://example.com/item", { method: "DELETE" });
  assert.equal(del.status, 204);
  assert.equal(seen[1]!.url, "https://example.com/item");
});

test("shimmed fetch lets blob: and data: URLs fall through to native fetch", async () => {
  let nativeCalls = 0;
  let portCalls = 0;
  const shimmed = createShimmedFetch(
    async () => {
      portCalls++;
      return { status: 200, statusText: "OK", url: "", headers: [], body: null };
    },
    (async (input: RequestInfo | URL) => {
      nativeCalls++;
      return new Response(`native:${String(input)}`);
    }) as typeof fetch,
  );

  const res = await shimmed("data:text/plain,hello");
  assert.equal(await res.text(), "native:data:text/plain,hello");
  assert.equal(nativeCalls, 1);
  assert.equal(portCalls, 0);
});

test("shimmed XMLHttpRequest delivers JSON/text responses and refuses synchronous open", async () => {
  const FakeXHR = createShimmedXHR(async (params): Promise<HttpResponseResult> => ({
    status: 200,
    statusText: "OK",
    url: params.url,
    headers: [["Content-Type", "application/json"]],
    body: new TextEncoder().encode('{"ok":true}').buffer,
  }));

  const sync = new FakeXHR();
  assert.throws(() => sync.open("GET", "/api/x", false), /synchronous XMLHttpRequest/);

  const xhr = new FakeXHR();
  xhr.responseType = "json";
  xhr.open("GET", "/api/ping");
  const done = new Promise<void>((resolve, reject) => {
    xhr.onload = () => resolve();
    xhr.onerror = () => reject(new Error("xhr failed"));
  });
  xhr.send();
  await done;

  assert.equal(xhr.readyState, 4);
  assert.equal(xhr.status, 200);
  assert.deepEqual(xhr.response, { ok: true });
  assert.equal(xhr.getResponseHeader("content-type"), "application/json");
});
