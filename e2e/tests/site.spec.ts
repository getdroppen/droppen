import { expect, test } from "@playwright/test";
import { API, APP, HOSTS, HUB_WS, SITE, cspSources, indexedDbNames, webauthnPolicy } from "./helpers";

/**
 * `www.droppen.test`: the real site-role build. The hub's marker is in the
 * shell (it is the same build), but `/.well-known/droppen-hub` puts the page
 * in site mode: public pages only, no key store, no service worker, WebAuthn
 * denied by policy, and every way into the app absolute to `my.`.
 */
test.describe("site on www.", () => {
  test("is site mode: the hub marker, but no API in connect-src", async ({ page }) => {
    const response = await page.goto(`${SITE}/`);
    expect(response?.status()).toBe(200);
    await expect(page.locator('meta[name="rh-hub"]')).toHaveAttribute("content", HUB_WS);
    // The site host's own answer settles the host mode to `site`.
    await expect(page.locator('[data-testid="landing"]')).toBeVisible();

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
    await expect(page.locator('[data-testid="landing"]')).toBeVisible();
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
    await expect(page.locator('[data-testid="landing"]')).toBeVisible();
    const links = await page.locator("a[href]").evaluateAll((anchors) => anchors.map((a) => (a as HTMLAnchorElement).href));
    const intoApp = links.filter((href) => href.startsWith(`${APP}/`) || href === APP);
    expect(intoApp.length).toBeGreaterThanOrEqual(2);
    for (const href of intoApp) expect(new URL(href).host).toBe(HOSTS.app);
    // Sign-in and the app itself are not pages of this host.
    expect(links.some((href) => href === `${SITE}/signin` || href === `${SITE}/sessions`)).toBe(false);
  });
});
