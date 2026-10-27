// The local app's gateway (`features/gateway/gateway-boot.tsx`): its shell is
// a chunk of its own, fetched once there are other devices to show. A chunk
// that does not come is fetched again at the next look; a page that no longer
// shows the gateway does not fetch it again.
//
// The next look is a timer of the page's own; each test runs it on the test's
// clock (`t.mock.timers`) so "before" and "after" are exact, rather than a real
// sleep racing a real timer on a busy machine.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach, type TestContext } from "node:test";
import { installFakeDom } from "./fake-dom.js";

const container = installFakeDom();
after(() => void setTimeout(() => process.exit(), 50));
/** Let the promises the page set going settle, without a timer. */
const settle = () =>
  React.act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
/** Advance the page's timers by `ms` on the test's clock, and let what they set going settle. */
const advance = (t: TestContext, ms: number) =>
  React.act(async () => {
    t.mock.timers.tick(ms);
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });

const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { GatewayBoot } = await import("../src/features/gateway/gateway-boot.js");
const { cloudKey } = await import("../src/features/link/link-flow.js");
const { LOCAL } = await import("../src/lib/machines/key.js");

/** The gateway shown (`GET /api/cloud` already answered so), with `loadShell` for its chunk. */
async function mount(loadShell: () => Promise<{ startGatewayShell: (...args: unknown[]) => never }>) {
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  client.setQueryData(cloudKey(LOCAL), { seesOtherDevices: true, aggregator: { enabled: true, state: "on" } });
  const root = createRoot(container as never);
  await React.act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        <GatewayBoot loadShell={loadShell as never} retryMs={20} />
      </QueryClientProvider>,
    ),
  );
  let removed = false;
  const page = {
    unmount: async () => {
      if (removed) return;
      removed = true;
      await React.act(async () => root.unmount());
    },
  };
  mounted = page;
  return page;
}

/** The page a test mounted, so a failed assertion still takes it down (a root left up holds the container). */
let mounted: { unmount: () => Promise<void> } | null = null;
afterEach(async () => {
  const page = mounted;
  mounted = null;
  await page?.unmount();
});

const notFetched = () => Promise.reject(new TypeError("Failed to fetch dynamically imported module"));

test("a gateway shell's chunk that did not come is fetched again at the next look, and the shell starts", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const started: unknown[] = [];
  let tries = 0;
  const page = await mount(() => {
    tries++;
    return tries === 1 ? notFetched() : Promise.resolve({ startGatewayShell: ((link: unknown) => void started.push(link)) as never });
  });
  await settle();
  assert.equal(tries, 1);
  assert.equal(started.length, 0);
  await advance(t, 19);
  assert.equal(tries, 1, "not before the wait is over");
  await advance(t, 1);
  assert.equal(tries, 2, "fetched again");
  assert.equal(started.length, 1, "and started over the gateway's link");
  await page.unmount();
});

test("a page that no longer shows the gateway does not fetch its chunk again", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let tries = 0;
  const page = await mount(() => {
    tries++;
    return notFetched();
  });
  // The chunk did not come; its next look is on the timer.
  await settle();
  assert.equal(tries, 1);
  await page.unmount();
  // Long past the wait, with the page gone: nothing is fetched.
  await advance(t, 60_000);
  assert.equal(tries, 1);
});
