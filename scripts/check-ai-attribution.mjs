#!/usr/bin/env node
/**
 * Commits and pull requests carry their human authors only (CONTRIBUTING.md):
 * - no commit message line that is a co-author trailer naming an AI tool or
 *   its vendor ({@link CO_AUTHOR});
 * - no line that says the change was generated with or by one
 *   ({@link GENERATED}), in a commit message or in the pull request's title or
 *   body (a squash merge's message is made from them);
 * - no commit whose author or committer email is an AI vendor's no-reply
 *   address ({@link VENDOR_EMAILS}).
 *
 * `node scripts/check-ai-attribution.mjs <revision range>` checks the commits
 * `git log <range>` lists, and the title and body in `PR_TITLE` and `PR_BODY`
 * (either may be unset or empty). `pr-guard.yml`'s `no-ai-attribution` job
 * runs it over a pull request's commits, `origin/<base>..<head>`. It prints a
 * GitHub `::error::` line per finding, with the commit and the line, and exits
 * 1 when there is one, 2 when git cannot list the range or it lists nothing.
 */

import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

/** A co-author trailer naming an AI tool or its vendor. */
export const CO_AUTHOR = /^\s*co-authored-by:.*(claude|anthropic|openai|chatgpt|copilot|codex|gemini|cursor|noreply@anthropic\.com)/i;

/**
 * A line saying the change was generated with or by an AI tool, as the tools
 * write it (the name may be a Markdown link, the line may start with an emoji
 * or other punctuation): only at the start of a line, so prose about sessions
 * a tool generated, in a product that drives these tools, is not one.
 */
export const GENERATED = /^[^a-z0-9]*generated (with|by) \[?(claude|chatgpt|copilot|codex|an ai)/i;

/** Author or committer addresses that are an AI vendor's, not a person's. */
export const VENDOR_EMAILS = ["noreply@anthropic.com"];

/** The lines of `text` that {@link CO_AUTHOR} or {@link GENERATED} match. */
export function attributionLines(text) {
  return String(text ?? "")
    .split(/\r?\n/)
    .filter((line) => CO_AUTHOR.test(line) || GENERATED.test(line))
    .map((line) => line.trim());
}

/**
 * Every finding in `commits` (`{sha, authorEmail, committerEmail, message}`)
 * and in the pull request's `title` and `body`: `{where, what}`.
 */
export function findAttribution({ commits = [], title = "", body = "" } = {}) {
  const found = [];
  for (const c of commits) {
    const where = `commit ${c.sha}`;
    for (const [role, email] of [
      ["author", c.authorEmail],
      ["committer", c.committerEmail],
    ]) {
      if (VENDOR_EMAILS.includes(String(email ?? "").trim().toLowerCase())) found.push({ where, what: `${role} email ${email}` });
    }
    for (const line of attributionLines(c.message)) found.push({ where, what: line });
  }
  for (const [part, text] of [
    ["title", title],
    ["body", body],
  ]) {
    for (const line of attributionLines(text)) found.push({ where: `the pull request's ${part}`, what: line });
  }
  return found;
}

/** The commits `git log <range>` lists, newest first. */
export function commitsIn(range, cwd = process.cwd()) {
  const out = execFileSync("git", ["-c", "log.showSignature=false", "log", "-z", "--format=%H%x1f%ae%x1f%ce%x1f%B", range, "--"], {
    cwd,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return out
    .split("\0")
    .filter((record) => record.trim() !== "")
    .map((record) => {
      const [sha, authorEmail, committerEmail, ...message] = record.replace(/^\n/, "").split("\x1f");
      return { sha, authorEmail, committerEmail, message: message.join("\x1f") };
    });
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const range = process.argv[2];
  if (!range) {
    console.error("usage: check-ai-attribution.mjs <revision range>   (PR_TITLE and PR_BODY from the environment)");
    process.exit(2);
  }
  let commits;
  try {
    commits = commitsIn(range);
  } catch (e) {
    console.log(`::error::cannot list the commits in ${range}: ${String(e.message ?? e).split("\n")[0]}`);
    process.exit(2);
  }
  if (commits.length === 0) {
    console.log(`::error::no commits in ${range}`);
    process.exit(2);
  }
  const found = findAttribution({ commits, title: process.env.PR_TITLE, body: process.env.PR_BODY });
  if (found.length) {
    for (const f of found) console.log(`::error::${f.where}: ${f.what}`);
    console.error(
      `\n${found.length} AI attribution line(s) or address(es). Commits and pull requests carry their human authors ` +
        "only (CONTRIBUTING.md): take the line out (reword the commit, edit the title or body), and author and commit " +
        "under your own name and address.",
    );
    process.exit(1);
  }
  console.log(`ok: no AI attribution in ${commits.length} commit(s), the title or the body`);
}
