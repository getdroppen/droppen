import { expect, test } from "@playwright/test";
import { API, APP, HOSTS, HUB_WS, cspSources, waitForHub } from "./helpers";

/**
 * `my.droppen.test`: the real web build, in cloud mode, with the API on a
 * different origin. This is the shape the move creates, and the one unit tests
 * cannot see: a cross-origin sign-up config, a device session, `/v1/me`, the
 * device socket, and the service worker.
 */
test.describe("app on my.", () => {
  test("loads the cloud build with the api. marker and calls the API there", async ({ page }) => {
    const response = await page.goto(`${APP}/`);
    expect(response?.status()).toBe(200);

    // The marker is the one source of truth for cloud mode and the API origin.
    await expect(page.locator('meta[name="rh-hub"]')).toHaveAttribute("content", HUB_WS);

    // The boot asks the fake hub who may sign up; that request left my. for api.
    const boot = await waitForHub(
      (entry) => entry.kind === "http" && entry.method === "GET" && entry.path === "/v1/signup/config" && entry.origin === APP,
    );
    expect(boot.allowed, "the API must allow my.droppen.test's CORS preflight/request").toBe(true);
    expect(boot.host).toBe(HOSTS.api);

    // The app's own CSP must let the page talk to the marker's HTTP and socket origins.
    const csp = (await response?.headerValue("content-security-policy")) ?? "";
    expect(cspSources(csp, "connect-src")).toEqual(expect.arrayContaining([API, HUB_WS]));
  });

  test("a device session and /v1/me succeed from my.", async ({ page }) => {
    await page.goto(`${APP}/`);
    const result = await page.evaluate(async (api) => {
      const post = (path: string, body: unknown) =>
        fetch(api + path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
      const challenge = await post("/v1/session/challenge", { account: "acct-e2e", device: "dev-e2e" }).then((r) => r.json());
      const sessionResponse = await post("/v1/session", { account: "acct-e2e", device: "dev-e2e", nonce: challenge.nonce, sig: "00" });
      const session = await sessionResponse.json();
      const meResponse = await fetch(api + "/v1/me", { headers: { authorization: `Bearer ${session.token}` } });
      return {
        challengeNonce: typeof challenge.nonce === "string" && challenge.nonce.length === 64,
        sessionStatus: sessionResponse.status,
        token: typeof session.token === "string" && session.token.length > 0,
        meStatus: meResponse.status,
        me: await meResponse.json(),
      };
    }, API);

    expect(result.challengeNonce).toBe(true);
    expect(result.sessionStatus).toBe(200);
    expect(result.token).toBe(true);
    expect(result.meStatus).toBe(200);
    expect(result.me.account).toBe("acct-e2e");

    // All three went to the fake hub with my. as the Origin.
    for (const path of ["/v1/session/challenge", "/v1/session", "/v1/me"]) {
      await waitForHub((entry) => entry.kind === "http" && entry.method !== "OPTIONS" && entry.path === path && entry.origin === APP && entry.allowed === true);
    }
  });

  test("the device socket on /v1/client connects", async ({ page }) => {
    await page.goto(`${APP}/`);
    const opened = await page.evaluate(
      (url) =>
        new Promise<{ ok: boolean; reason?: string }>((resolve) => {
          const socket = new WebSocket(url);
          const timer = setTimeout(() => resolve({ ok: false, reason: "timeout" }), 10_000);
          socket.onopen = () => {
            clearTimeout(timer);
            socket.close();
            resolve({ ok: true });
          };
          socket.onerror = () => {
            clearTimeout(timer);
            resolve({ ok: false, reason: "error" });
          };
        }),
      `${HUB_WS}/v1/client`,
    );
    expect(opened.ok, opened.reason).toBe(true);
    await waitForHub((entry) => entry.kind === "ws-open" && entry.path === "/v1/client" && entry.origin === APP);
  });

  test("the real service worker registers on my.", async ({ page }) => {
    await page.goto(`${APP}/`);
    const worker = await page.evaluate(async () => {
      if (!("serviceWorker" in navigator)) return { supported: false };
      const registration = await Promise.race([
        navigator.serviceWorker.ready,
        new Promise<null>((resolve) => setTimeout(() => resolve(null), 20_000)),
      ]);
      if (!registration) return { supported: true, ready: false };
      return { supported: true, ready: true, scope: registration.scope, script: registration.active?.scriptURL ?? null };
    });
    expect(worker.supported).toBe(true);
    expect(worker.ready, "navigator.serviceWorker.ready must settle: the worker is served on the app host").toBe(true);
    expect(worker.script).toContain("/sw.js");
    expect(new URL(worker.script!).host).toBe(HOSTS.app);

    // It is the real worker, not the site's retiring one.
    const body = await page.evaluate(() => fetch("/sw.js").then((r) => r.text()));
    expect(body).not.toContain("registration.unregister");
    expect(body).toContain("self.addEventListener");
  });
});
