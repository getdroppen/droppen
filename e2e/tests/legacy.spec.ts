import { expect, test } from "@playwright/test";
import { APEX, APP, indexedDbNames, waitForHub } from "./helpers";

/**
 * The apex during the compatibility window: a page telling people the app
 * moved, no keys ever kept there, and a machine that still dials
 * `wss://droppen.app` accepted as an alias by the hub.
 */
test.describe("legacy apex", () => {
  test("serves the move interstitial and never keeps keys", async ({ page }) => {
    const response = await page.goto(`${APEX}/`);
    expect(response?.status()).toBe(200);
    await expect(page.locator("body")).toContainText(/moved/i);
    const href = await page.locator("#app-link").getAttribute("href");
    expect(href).toBe(`${APP}/`);

    await page.waitForTimeout(1_000);
    const state = await page.evaluate(async () => {
      const registrations = "serviceWorker" in navigator ? (await navigator.serviceWorker.getRegistrations()).length : 0;
      const factory = indexedDB as IDBFactory & { databases?: () => Promise<{ name?: string }[]> };
      const names = typeof factory.databases === "function" ? (await factory.databases()).map((db) => db.name ?? "") : [];
      return { registrations, names };
    });
    expect(state.registrations).toBe(0);
    expect(state.names).not.toContain("rh-e2e");
    expect(await indexedDbNames(page)).not.toContain("rh-e2e");
  });

  test("a machine handshake naming the apex is still accepted (alias)", async ({ page }) => {
    await page.goto(`${APEX}/`);
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
