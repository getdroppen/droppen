import { expect, test } from "@playwright/test";
import { API, EVIL, waitForHub } from "./helpers";

/**
 * An authenticated API response must be readable only from the origins the
 * hub allows. `evil.test` sends the same bearer token the app sends; the
 * browser must refuse to hand it the body.
 */
test.describe("CORS negative", () => {
  test("a page on evil.test cannot read /v1/me", async ({ page }) => {
    await page.goto(`${EVIL}/`);
    const outcome = await page.evaluate(async (api) => {
      try {
        const response = await fetch(`${api}/v1/me`, { headers: { authorization: "Bearer e2e-device-session" } });
        return { read: true, status: response.status, body: (await response.text()).slice(0, 120) };
      } catch (error) {
        return { read: false, error: String(error) };
      }
    }, API);
    expect(outcome.read, `the browser must not expose the response: ${JSON.stringify(outcome)}`).toBe(false);

    // The hub saw the attempt and said nothing that allows it.
    const entry = await waitForHub((line) => line.kind === "http" && line.path === "/v1/me" && line.origin === EVIL);
    expect(entry.allowed).toBe(false);

    // The same token from the app's origin is readable (app.spec.ts proves the
    // path end to end); here the Origin alone decides.
  });
});
