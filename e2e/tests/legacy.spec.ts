import { expect, test, type Page } from "@playwright/test";
import { APEX, APP, HOSTS, indexedDbNames, waitForHub } from "./helpers";

/**
 * The apex during the compatibility window, with the real site-role build and
 * the hub's `apex_mode: move` answer: a browser with no device here is sent on
 * to the app; one that keeps a device sees the move notice; and a machine
 * that still dials `wss://droppen.test` is accepted by the hub (the alias).
 */

/** A browser that "keeps a device here" without any keys: the note `deviceHere()` reads. */
async function seedDeviceHere(page: Page): Promise<void> {
  await page.context().addInitScript(() => {
    try {
      if (location.hostname === "droppen.test") localStorage.setItem("rh.hub.device.acct-e2e", "dev-e2e");
    } catch {
      // storage not available: the notice just goes straight on
    }
  });
}

test.describe("legacy apex", () => {
  test("sends a browser with no device here on to the app", async ({ page }) => {
    await page.goto(`${APEX}/`);
    // The hub's origin hands a browser with nothing here to the app host, at
    // the same path (a bare `/` is the app host's sign-in, not the site's
    // landing page).
    await page.waitForURL((url) => url.origin === APP, { timeout: 20_000 });
    expect(new URL(page.url()).host).toBe(HOSTS.app);
  });

  test("shows the move notice to a browser that keeps a device, and never keeps keys", async ({ page }) => {
    await seedDeviceHere(page);
    await page.goto(`${APEX}/`);

    const notice = page.locator('[data-testid="move-notice"]');
    await expect(notice).toBeVisible();
    await expect(notice).toContainText(HOSTS.app);
    await expect(page.locator('[data-testid="move-notice-continue"]')).toBeVisible();

    const state = await page.evaluate(async () => {
      const registrations = "serviceWorker" in navigator ? (await navigator.serviceWorker.getRegistrations()).length : 0;
      const factory = indexedDB as IDBFactory & { databases?: () => Promise<{ name?: string }[]> };
      const names = typeof factory.databases === "function" ? (await factory.databases()).map((db) => db.name ?? "") : [];
      return { registrations, names };
    });
    expect(state.registrations).toBe(0);
    expect(state.names).not.toContain("rh-e2e");
    expect(await indexedDbNames(page)).not.toContain("rh-e2e");

    // Onward is the app host.
    await page.locator('[data-testid="move-notice-continue"]').click();
    await page.waitForURL((url) => url.origin === APP, { timeout: 20_000 });
  });

  test("a machine handshake naming the apex is still accepted (alias)", async ({ page }) => {
    await seedDeviceHere(page);
    await page.goto(`${APEX}/`);
    await expect(page.locator('[data-testid="move-notice"]')).toBeVisible();

    const result = await page.evaluate(
      (url) =>
        new Promise<{ ok: boolean; message?: unknown; reason?: string }>((resolve) => {
          const socket = new WebSocket(url);
          const timer = setTimeout(() => {
            socket.close();
            resolve({ ok: false, reason: "timeout" });
          }, 10_000);
          socket.onopen = () => socket.send(JSON.stringify({ t: "login", role: "agent", hub: "wss://droppen.test", account: "acct-e2e", id: "host-e2e" }));
          socket.onmessage = (event) => {
            clearTimeout(timer);
            let message: unknown = null;
            try {
              message = JSON.parse(String(event.data));
            } catch {}
            socket.close();
            resolve({ ok: (message as { t?: string } | null)?.t === "accepted", message });
          };
          socket.onerror = () => {
            clearTimeout(timer);
            resolve({ ok: false, reason: "error" });
          };
        }),
      "wss://droppen.test/v1/agent",
    );
    expect(result.ok, JSON.stringify(result)).toBe(true);

    const handshake = await waitForHub((entry) => entry.kind === "ws-handshake" && entry.path === "/v1/agent");
    expect(handshake.hub).toBe("wss://droppen.test");
    expect(handshake.accepted).toBe(true);
  });
});
