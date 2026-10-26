import type { AccountProvider } from "@/features/accounts/use-accounts";

/**
 * The `KEY=VALUE` block an env account is made of, read the way the server
 * reads it.
 *
 * The server is the authority — it parses the same text again and refuses what
 * it does not like — but a refusal that arrives after the secret was typed,
 * with a line number the browser never showed, is a bad way to learn that line
 * 3 has a space in its name. So the same rules run here as the user types, and
 * the first problem is named with its line.
 */

/** One accepted variable, with the line it came from for error reporting. */
export type EnvVar = { key: string; value: string; line: number };

/**
 * The first thing wrong with a block, or nothing. Line numbers are 1-based and
 * count every line of the raw text, including the blanks and comments that are
 * skipped, so they match what the textarea shows.
 */
export type EnvBlockIssue =
  | { code: "empty" }
  | { code: "too_large" }
  | { code: "too_many" }
  | { code: "syntax"; line: number }
  | { code: "denied"; line: number; key: string }
  | { code: "duplicate"; line: number; key: string }
  | { code: "bad_url"; line: number; key: string }
  | { code: "codex_key_missing" };

/**
 * Something suspicious about an accepted block. Unlike an issue this refuses
 * nothing: the server would store it, the CLI would start, and the request
 * would fail somewhere the user has no way of seeing — so it is said here,
 * beside the line, without standing in the way of someone who meant it.
 */
export type EnvBlockWarning = { code: "base_url_v1"; line: number; key: string };

export type EnvBlock = {
  vars: EnvVar[];
  issue: EnvBlockIssue | null;
  warning: EnvBlockWarning | null;
};

/** Variables that would move a CLI's home or its search path. */
export const ENV_DENYLIST = [
  "CLAUDE_CONFIG_DIR",
  "CODEX_HOME",
  "HOME",
  "PATH",
  "NODE_OPTIONS",
  "LD_PRELOAD",
  "LD_LIBRARY_PATH",
  "DYLD_INSERT_LIBRARIES",
  "DYLD_LIBRARY_PATH",
  "DYLD_FRAMEWORK_PATH",
];

/** How many variables one account may carry. */
export const ENV_MAX_VARS = 64;

/** How big the raw block may be, in bytes. */
export const ENV_MAX_BYTES = 8 * 1024;

/** Either of the names a Codex account's key may go by. */
const CODEX_KEYS = ["OPENAI_API_KEY", "CODEX_API_KEY"];

const KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** An http(s) URL with a host, which is all a base URL may ever be. */
function isHttpUrl(value: string): boolean {
  if (!/^https?:\/\//i.test(value) || /\s/.test(value)) return false;
  try {
    return new URL(value).host !== "";
  } catch {
    return false;
  }
}

/**
 * A base URL that already carries the version segment claude appends itself.
 *
 * `ANTHROPIC_BASE_URL=https://host/v1` is the single commonest way to point the
 * CLI at a self-hosted endpoint and get nothing back: the CLI builds
 * `<base>/v1/messages`, so the request goes to `/v1/v1/messages` and the 404
 * comes back worded as a problem with the model that was asked for — which
 * sends people looking at the model name for an hour.
 *
 * Only `/v1` at the very end counts. `/api/v1beta` is a different path and
 * `/v1/messages` is a different mistake, neither of which this would fix.
 */
function endsInV1(value: string): boolean {
  return /\/v1\/?$/.test(value);
}

/** `"v"` and `'v'` are the same value as `v`; a lone quote is not a quote. */
function unquote(value: string): string {
  const first = value[0];
  if ((first === '"' || first === "'") && value.length >= 2 && value.endsWith(first)) {
    return value.slice(1, -1);
  }
  return value;
}

/**
 * Parse a block and report the first thing wrong with it.
 *
 * Blank lines and `#` comments are skipped, a leading `export ` is allowed (so
 * a block can be pasted straight out of a shell profile), and the split is on
 * the first `=` so a value may contain as many more as it likes.
 */
export function parseEnvBlock(text: string, provider: AccountProvider): EnvBlock {
  const vars: EnvVar[] = [];
  let warning: EnvBlockWarning | null = null;
  const done = (issue: EnvBlockIssue | null): EnvBlock => ({ vars, issue, warning });

  if (new TextEncoder().encode(text).length > ENV_MAX_BYTES) return done({ code: "too_large" });

  const seen = new Map<string, number>();
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    const line = i + 1;
    let raw = (lines[i] ?? "").trim();
    if (raw === "" || raw.startsWith("#")) continue;
    if (raw.startsWith("export ")) raw = raw.slice("export ".length).trim();

    const eq = raw.indexOf("=");
    if (eq <= 0) return done({ code: "syntax", line });
    // Trimmed on both sides of the `=`, exactly as the server trims them, so
    // `FOO = bar` is the one variable it is there too rather than a syntax
    // error the server would never have raised.
    const key = raw.slice(0, eq).trim();
    if (!KEY_RE.test(key)) return done({ code: "syntax", line });
    if (ENV_DENYLIST.includes(key)) return done({ code: "denied", line, key });
    if (seen.has(key)) return done({ code: "duplicate", line, key });

    // An empty value is a real value: it is how a variable is unset for the
    // CLI without the block having to mention it twice.
    const value = unquote(raw.slice(eq + 1).trim());
    // Only where the server checks it: codex interpolates it into inline TOML,
    // and claude hands it to the CLI like any other variable — so judging it on
    // a claude block would refuse one the server would have taken. Blank is
    // "the vendor default" on both sides.
    if (provider === "codex" && key === "OPENAI_BASE_URL" && value !== "" && !isHttpUrl(value)) {
      return done({ code: "bad_url", line, key });
    }
    // Only claude's, and only as a warning: OpenAI-compatible endpoints are
    // reached at `<base>/v1/chat/completions` with the `/v1` in the base, so
    // the same tail on `OPENAI_BASE_URL` is right rather than wrong.
    if (key === "ANTHROPIC_BASE_URL" && endsInV1(value)) {
      warning = { code: "base_url_v1", line, key };
    }
    seen.set(key, line);
    vars.push({ key, value, line });
  }

  if (vars.length === 0) return done({ code: "empty" });
  if (vars.length > ENV_MAX_VARS) return done({ code: "too_many" });
  // Codex reaches its provider through one key; without it the account would be
  // stored and then do nothing at all.
  if (provider === "codex" && !CODEX_KEYS.some((k) => seen.has(k))) {
    return done({ code: "codex_key_missing" });
  }
  return done(null);
}
