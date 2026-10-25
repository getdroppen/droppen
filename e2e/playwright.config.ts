import { createHash, X509Certificate } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";
import { ensureCertificates } from "./scripts/certs.mjs";

/**
 * Three engines against the deployed shape: the real web build on `my.`, the
 * site fixture on `www.`, the interstitial on the apex, the fake hub on
 * `api.` and an unrelated origin on `evil.` — all over TLS, all resolved to
 * this machine by `/etc/hosts` (local mode) or Docker `extra_hosts`.
 *
 * `E2E_EXTERNAL_SERVERS=1` skips the managed server: useful when the HTTPS
 * listener runs as root (port 443) in another terminal.
 */
const app = process.env.E2E_APP_URL ?? "https://my.droppen.test";

// Chromium's service worker (unlike its pages) refuses a certificate that
// `ignoreHTTPSErrors` only papers over. Pinning this run's throwaway
// certificate by SPKI lets the worker register without trusting it anywhere
// else. The certificate is generated here (idempotently) so its hash is known
// before the browsers launch; `scripts/prepare.mjs` reuses the same files.
const here = dirname(fileURLToPath(import.meta.url));
const certs = ensureCertificates(resolve(here, ".e2e", "certs"));
const spki = createHash("sha256")
  .update(new X509Certificate(readFileSync(certs.cert)).publicKey.export({ type: "spki", format: "der" }))
  .digest("base64");

export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  use: {
    baseURL: app,
    ignoreHTTPSErrors: true,
    trace: "retain-on-failure",
    video: "off",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"], launchOptions: { args: [`--ignore-certificate-errors-spki-list=${spki}`] } },
    },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  webServer: process.env.E2E_EXTERNAL_SERVERS
    ? undefined
    : {
        command: "node scripts/serve.mjs",
        url: `${app}/__health`,
        ignoreHTTPSErrors: true,
        reuseExistingServer: !process.env.CI,
        timeout: 180_000,
        stdout: "pipe",
        stderr: "pipe",
      },
});
