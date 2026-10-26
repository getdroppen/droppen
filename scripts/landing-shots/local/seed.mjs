// Write the demo home: dev's projects (git repos), finished Claude and Codex
// sessions as the CLIs would have left them, and the turns the scripted
// `claude` will play live (under <state>/live).
//
//   node seed.mjs --lang en|zh-CN --home /Users/dev --state /opt/demo/state
//
// Run as the user the server runs as, on an empty home, before the server
// starts. The sessions are where the CLIs keep them on the user's own login
// (`~/.claude`, `~/.codex`), and every one is listed in the instance's
// ownership registry: the server's own (drivable, on the local login), not a
// browse-only external one.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { CLAUDE_MODEL, CODEX_MODEL, projects, sessions } from "./scenario.mjs";
import { ClaudeTranscript, CodexRollout, projectDirName } from "./transcript.mjs";

const { values: opts } = parseArgs({
  options: { lang: { type: "string" }, home: { type: "string" }, state: { type: "string" } },
});
const lang = opts.lang;
const home = opts.home;
const state = opts.state;
if (!["en", "zh-CN"].includes(lang) || !home || !state) {
  console.error("usage: seed.mjs --lang en|zh-CN --home <dir> --state <dir>");
  process.exit(2);
}

const instance = path.join(home, ".local/share/remote-hand/instances/default");
const claudeRoot = path.join(home, ".claude");
const now = Date.now();
const iso = (ms) => new Date(ms).toISOString();

const write = (file, body, mode) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, body, mode ? { mode } : undefined);
};
const jsonl = (lines) => lines.map((line) => JSON.stringify(line)).join("\n") + "\n";
const touch = (file, ms) => fs.utimesSync(file, new Date(ms), new Date(ms));

// ---------------------------------------------------------------- projects

const git = (cwd, args, when) =>
  execFileSync("git", args, {
    cwd,
    stdio: ["ignore", "ignore", "inherit"],
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "dev",
      GIT_AUTHOR_EMAIL: "dev@macbook.local",
      GIT_COMMITTER_NAME: "dev",
      GIT_COMMITTER_EMAIL: "dev@macbook.local",
      ...(when ? { GIT_AUTHOR_DATE: iso(when), GIT_COMMITTER_DATE: iso(when) } : {}),
    },
  });

for (const [name, project] of Object.entries(projects(lang))) {
  const dir = path.join(home, "code", name);
  for (const [file, body] of Object.entries(project.files)) write(path.join(dir, file), body);
  git(dir, ["init", "-q", "-b", "main"]);
  // Spread the history over the last few days, one commit per message; the
  // files all land in the first commit, which is all `git log` needs.
  const all = [...project.commits, ...project.branchCommits];
  project.commits.forEach((message, i) => {
    git(dir, ["add", "-A"]);
    git(dir, ["commit", "-q", "--allow-empty", "-m", message], now - (all.length - i) * 7.5 * 3600e3);
  });
  if (project.branch !== "main") git(dir, ["checkout", "-q", "-b", project.branch]);
  project.branchCommits.forEach((message, i) => {
    git(dir, ["commit", "-q", "--allow-empty", "-m", message], now - (project.branchCommits.length - i) * 1.5 * 3600e3);
  });
}

// ---------------------------------------------------------------- sessions

const owned = { claude: [], codex: [] };
const index = [];

for (const session of sessions(lang)) {
  // History is spread out the way a person and an agent take turns: a
  // message, then the agent's steps a few seconds to a minute apart.
  let clock = now - session.startMinutesAgo * 60e3;
  const tick = (step) => {
    clock += step.user !== undefined ? 15e3 : step.say !== undefined ? 9e3 : 21e3;
    return iso(clock);
  };

  if (session.source === "claude") {
    const transcript = new ClaudeTranscript({ id: session.id, cwd: session.cwd, branch: session.branch, model: CLAUDE_MODEL });
    const lines = session.history.flatMap((step) => transcript.step(step, tick(step)).lines.map(({ line }) => line));
    const file = path.join(claudeRoot, "projects", projectDirName(session.cwd), `${session.id}.jsonl`);
    write(file, jsonl(lines));
    touch(file, clock);
    owned.claude.push(session.id);
    if (session.live) {
      // The scripted CLI continues this transcript: it needs the chain to
      // hang its entries off, and the turn it is to play.
      write(
        path.join(state, "live", `${session.id}.json`),
        JSON.stringify({ cwd: session.cwd, branch: session.branch, parent: transcript.parent, end: session.live.end, steps: session.live.steps }, null, 2),
      );
    }
  } else {
    const rollout = new CodexRollout({ id: session.id, cwd: session.cwd, model: CODEX_MODEL });
    const started = clock;
    const lines = [...rollout.open(iso(clock)), ...session.history.flatMap((step) => rollout.step(step, tick(step)))];
    lines.push(...rollout.close(iso(clock)));
    const day = new Date(started);
    const stamp = day.toISOString().slice(0, 19).replace(/:/g, "-");
    const file = path.join(
      home,
      ".codex/sessions",
      String(day.getUTCFullYear()),
      String(day.getUTCMonth() + 1).padStart(2, "0"),
      String(day.getUTCDate()).padStart(2, "0"),
      `rollout-${stamp}-${session.id}.jsonl`,
    );
    write(file, jsonl(lines));
    touch(file, clock);
    owned.codex.push(session.id);
    index.push({ id: session.id, thread_name: session.title, updated_at: iso(clock) });
  }
}

write(path.join(home, ".codex/session_index.jsonl"), jsonl(index));
// What Codex caches of the models the account may use; the composer's picker reads it.
const codexModel = (slug, name) => ({
  slug,
  display_name: name,
  visibility: "list",
  supported_reasoning_levels: [{ effort: "low" }, { effort: "medium" }, { effort: "high" }],
  default_reasoning_level: "medium",
});
write(path.join(home, ".codex/models_cache.json"), JSON.stringify({ models: [codexModel("gpt-5.5", "GPT-5.5"), codexModel("gpt-5.4", "GPT-5.4")] }));
// Drained into the instance's database on its first boot (see the server's
// `sessions::owned_sessions`): every session here is this server's own.
write(path.join(instance, "owned-sessions.json"), JSON.stringify(owned));
write(path.join(state, "seen.json"), JSON.stringify(sessions(lang).filter((s) => s.seen).map((s) => ({ source: s.source, id: s.id }))));
console.log(`seeded ${home} (${lang}): ${sessions(lang).length} sessions, ${Object.keys(projects(lang)).length} projects`);
