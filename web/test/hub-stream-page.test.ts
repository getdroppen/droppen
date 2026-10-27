// `HubApi.streamPage` reads one sealed snapshot page; the hub is assumed
// malicious, so the page is refused once it passes `MAX_PAGE_LEN` (the cap the
// Rust device's `read_capped` applies) instead of being buffered whole first.
// A `Content-Length` over the cap refuses it before the body, and a body that
// streams past the cap is refused and cancelled as it passes.
import assert from "node:assert/strict";
import test from "node:test";
import { MAX_PAGE_LEN, RecordError } from "../src/lib/e2e/index.js";
import { HubApi } from "../src/lib/hub/api.js";

const STREAM = new Uint8Array(32).fill(0x11);
const PAGE = new Uint8Array(32).fill(0x22);

/** A page body that arrives in `chunks`; `onCancel` sees the reader's cancellation. */
function pageBody(chunks: Uint8Array[], onCancel?: () => void): ReadableStream<Uint8Array> {
  const queue = [...chunks];
  return new ReadableStream({
    pull(controller) {
      const chunk = queue.shift();
      if (chunk) controller.enqueue(chunk);
      else controller.close();
    },
    cancel() {
      onCancel?.();
    },
  });
}

function apiReturning(response: Response): HubApi {
  return new HubApi("https://hub.test", async () => response);
}

const tooLarge = (error: unknown) => error instanceof RecordError && error.kind === "tooLarge";

test("a page whose Content-Length is over the cap is refused", async () => {
  let cancelled = false;
  const response = new Response(pageBody([new Uint8Array(1)], () => (cancelled = true)), {
    status: 200,
    headers: { "content-length": String(MAX_PAGE_LEN + 1) },
  });
  await assert.rejects(apiReturning(response).streamPage("tok", STREAM, PAGE), tooLarge);
  assert.equal(cancelled, false, "the body was not read");
});

test("a page with no Content-Length that streams past the cap is refused and its reader cancelled", async () => {
  let cancelled = false;
  const chunk = new Uint8Array(16_384);
  // Two chunks to spare: the stream is still open when the cap is passed, so
  // the cancellation the reader makes is visible to the body.
  const chunks = Array.from({ length: Math.floor(MAX_PAGE_LEN / chunk.length) + 2 }, () => chunk);
  const response = new Response(pageBody(chunks, () => (cancelled = true)), { status: 200 });
  await assert.rejects(apiReturning(response).streamPage("tok", STREAM, PAGE), tooLarge);
  assert.equal(cancelled, true, "the reader was cancelled once the cap was passed");
});

test("a page exactly at the cap is read whole", async () => {
  let cancelled = false;
  const first = new Uint8Array(30_000).fill(0xaa);
  const second = new Uint8Array(MAX_PAGE_LEN - first.length).fill(0xbb);
  const response = new Response(pageBody([first, second], () => (cancelled = true)), { status: 200 });
  const page = await apiReturning(response).streamPage("tok", STREAM, PAGE);
  assert.equal(page.length, MAX_PAGE_LEN);
  assert.equal(page[0], 0xaa);
  assert.equal(page[first.length - 1], 0xaa);
  assert.equal(page[first.length], 0xbb);
  assert.equal(page[page.length - 1], 0xbb);
  assert.equal(cancelled, false, "nothing to cancel: the page ended at the cap");
});

test("a normal page comes back as the bytes the hub sent", async () => {
  const body = new Uint8Array([1, 2, 3, 4, 5]);
  const response = new Response(pageBody([body.subarray(0, 2), body.subarray(2)]), { status: 200 });
  const page = await apiReturning(response).streamPage("tok", STREAM, PAGE);
  assert.deepEqual([...page], [1, 2, 3, 4, 5]);
});
