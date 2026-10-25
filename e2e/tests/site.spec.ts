import { expect, test } from "@playwright/test";
import { API, APP, HOSTS, SITE, cspSources, indexedDbNames, webauthnPolicy } from "./helpers";

/**
 * `www.droppen.test`: pages only. No `rh-hub` marker (so the app never enters
 * cloud mode), no device keys, no worker, WebAuthn denied by policy, and the
 * app links absolute to `my.` — the policy a static site build must carry.
 */
test.describe("site on www.", () => {
  test("is site mode: no marker, no API in connect-src", async ({ page }) => {
    const response = await page.goto(`${SITE}/`);
    expect(response?.status()).toBe(200);
    expect(await page.locator('meta[name="rh-hub"]').count()).toBe(0);

    const csp = (await response?.headerValue("content-security-policy")) ?? "";
    const connect = cspSources(csp, "connect-src");
    expect(connect).toContain("'self'");
    expect(connect.join(" ")).not.toContain(HOSTS.api);
    expect(connect.join(" ")).not.toContain(API);
  });

  test("Permissions-Policy denies WebAuthn", async ({ page, browserName }) => {
    const response = await page.goto(`${SITE}/`);
    const policy = webauthnPolicy(await response?.headerValue("permissions-policy"));
    expect(policy.get).toBe("()");
    expect(policy.create).toBe("()");

    // Where the browser exposes the effective policy, it says the same.
    const effective = await page.evaluate(() => {
      const doc = document as unknown as { featurePolicy?: { allowsFeature?: (name: string) => boolean } };
      return doc.featurePolicy?.allowsFeature ? doc.featurePolicy.allowsFeature("publickey-credentials-get") : undefined;
    });
    if (effective !== undefined) expect(effective, `${browserName} must deny publickey-credentials-get here`).toBe(false);
  });

  test("keeps no device keys and registers no worker", async ({ page }) => {
    await page.goto(`${SITE}/`);
    // Give a registration (if any) a moment to happen, then look.
    await page.waitForTimeout(1_000);
    const state = await page.evaluate(async () => {
      const registrations = "serviceWorker" in navigator ? (await navigator.serviceWorker.getRegistrations()).map((r) => r.scope) : [];
      const factory = indexedDB as IDBFactory & { databases?: () => Promise<{ name?: string }[]> };
      const names = typeof factory.databases === "function" ? (await factory.databases()).map((db) => db.name ?? "") : [];
      return { registrations, names };
    });
    expect(state.registrations).toEqual([]);
    expect(state.names).not.toContain("rh-e2e");
    expect(await indexedDbNames(page)).not.toContain("rh-e2e");
  });

  test("app links are absolute to my.", async ({ page }) => {
    await page.goto(`${SITE}/`);
    const links = await page.locator("a[data-app-link]").evaluateAll((anchors) => anchors.map((a) => (a as HTMLAnchorElement).href));
    expect(links.length).toBeGreaterThanOrEqual(2);
    for (const href of links) {
      expect(href.startsWith(`${APP}/`), `${href} must point at the app host`).toBe(true);
      expect(new URL(href).host).toBe(HOSTS.app);
    }
  });
});
