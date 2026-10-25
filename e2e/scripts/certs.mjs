// A throwaway TLS certificate for the e2e hosts, generated at run time (never
// committed: `e2e/.e2e/` is gitignored, and this is a test certificate that
// trusts nothing). One certificate names every host the harness serves; the
// single HTTPS listener answers the `Host` header.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

export const HOSTS = {
  api: "api.droppen.test",
  app: "my.droppen.test",
  site: "www.droppen.test",
  apex: "droppen.test",
  evil: "evil.test",
};

const SAN = Object.values(HOSTS).map((host) => `DNS:${host}`).join(",");

/** Regenerate a day before it expires, so a long-lived pod never serves a dead cert. */
const RENEW_BEFORE_MS = 24 * 60 * 60 * 1000;
const DAYS = 3;

/**
 * `dir` with `key.pem` and `cert.pem`, generated if missing (or nearly
 * expired). Returns their paths. Uses `openssl`, which the Playwright image and
 * macOS both have.
 */
export function ensureCertificates(dir) {
  const key = join(dir, "key.pem");
  const cert = join(dir, "cert.pem");
  const fresh = existsSync(key) && existsSync(cert) && statSync(cert).mtimeMs > Date.now() - (DAYS * 24 * 60 * 60 * 1000 - RENEW_BEFORE_MS);
  if (fresh) return { key, cert };
  mkdirSync(dir, { recursive: true });
  execFileSync(
    "openssl",
    [
      "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-days", String(DAYS), "-nodes",
      "-keyout", key, "-out", cert,
      "-subj", "/CN=droppen.test",
      "-addext", `subjectAltName=${SAN}`,
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  // The private key is generated for this run only; keep it unreadable to others.
  execFileSync("chmod", ["600", key]);
  return { key, cert };
}

export function certificateBody(certPath) {
  return readFileSync(certPath);
}
