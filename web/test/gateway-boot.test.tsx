// The local app's gateway (`features/gateway/gateway-boot.tsx`): its shell is
// a chunk of its own, fetched once there are other devices to show. A chunk
// that does not come is fetched again at the next look; a page that no longer
// shows the gateway does not fetch it again.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { installFakeDom } from "./fake-dom.js";

const container = installFakeDom();
after(() => void setTimeout(() => process.exit(), 50));
const flush = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

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
  return { unmount: () => React.act(async () => root.unmount()) };
}

const notFetched = () => Promise.reject(new TypeError("Failed to fetch dynamically imported module"));

test("a gateway shell's chunk that did not come is fetched again at the next look, and the shell starts", async () => {
  const started: unknown[] = [];
  let tries = 0;
  const page = await mount(() => {
    tries++;
    return tries === 1 ? notFetched() : Promise.resolve({ startGatewayShell: ((link: unknown) => void started.push(link)) as never });
  });
  await React.act(() => flush(5));
  assert.equal(tries, 1);
  assert.equal(started.length, 0);
  await React.act(() => flush(60));
  assert.equal(tries, 2, "fetched again");
  assert.equal(started.length, 1, "and started over the gateway's link");
  await page.unmount();
});

test("a page that no longer shows the gateway does not fetch its chunk again", async () => {
  let tries = 0;
  const page = await mount(() => {
    tries++;
    return notFetched();
  });
  await React.act(() => flush(5));
  await page.unmount();
  await flush(60);
  assert.equal(tries, 1);
});
