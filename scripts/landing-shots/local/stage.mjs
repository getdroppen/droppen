// Bring the demo server into the state the tour shows, through its own API:
// the language, the sessions already read, the two live sessions started (one
// ends blocked on a question, one stays working), two terminals and a
// three-pane screen. Writes what it made to <state>/staged.json.
//
//   node stage.mjs --lang en|zh-CN --url http://127.0.0.1:8787 --state /opt/demo/state

import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { parseArgs } from "node:util";
import { CODE, IDS, names, sessions } from "./scenario.mjs";

const { values: opts } = parseArgs({ options: { lang: { type: "string" }, url: { type: "string" }, state: { type: "string" } } });
const { lang, url, state } = opts;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function api(method, route, body) {
  const res = await fetch(url + route, {
    method,
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${method} ${route}: ${res.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

async function until(what, check, seconds = 60) {
  const deadline = Date.now() + seconds * 1000;
  for (;;) {
    try {
      const value = await check();
      if (value) return value;
    } catch (error) {
      if (Date.now() > deadline) throw error;
    }
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(500);
  }
}

await until("the server", () => api("GET", "/api/prefs"), 120);

// The UI language is a server preference.
const prefs = await api("GET", "/api/prefs");
await api("PUT", "/api/prefs", { ...prefs, language: lang });

// The first-run guide, as somebody who went through it long ago.
const progress = await api("GET", "/api/setup/progress");
const long = new Date(Date.now() - 30 * 86400e3).toISOString();
await api("PUT", "/api/setup/progress", { ...progress, dismissed: true, startedAt: long, finishedAt: long });

// Every session the scenario calls read, read; the rest stay unread.
await until("the session list", async () => (await api("GET", "/api/sessions")).sessions?.length >= sessions(lang).length);
for (const session of sessions(lang).filter((s) => s.seen && !s.live)) {
  await api("POST", `/api/sessions/${session.source}/${session.id}/seen`);
}

// The live sessions: the first message of each plays its scripted turn.
for (const session of sessions(lang).filter((s) => s.live)) {
  await api("POST", `/api/sessions/claude/${session.id}/message`, { text: session.live.steps[0].user, clientMessageId: randomUUID() });
}
await until("the question to be pending", async () => {
  const { questions } = await api("GET", `/api/sessions/claude/${IDS.ask}/questions`);
  return questions?.some((q) => q.state === "pending");
});
await until("the working session's last step", async () => {
  const detail = await api("GET", `/api/sessions/claude/${IDS.work}`);
  return detail.items.at(-1)?.kind === "tool_use";
});
for (const session of sessions(lang).filter((s) => s.live)) {
  await api("POST", `/api/sessions/claude/${session.id}/seen`);
}

// Two terminals: one for the full-screen shot, one beside the agents.
const label = names(lang);
const shell = await api("POST", "/api/terminals", { cwd: `${CODE}/shop-api`, title: label.terminal });
const dev = await api("POST", "/api/terminals", { cwd: `${CODE}/web`, title: label.devTerminal });

// The split screen: the working Claude session on the left, Codex above the
// dev server on the right.
const screen = await api("POST", "/api/screens", { title: label.screen });
const leaf = (id, content) => ({ type: "leaf", id, content });
const layout = {
  type: "split",
  id: "split-main",
  direction: "row",
  ratio: 0.5,
  children: [
    leaf("pane-claude", { kind: "session", source: "claude", id: IDS.work }),
    {
      type: "split",
      id: "split-side",
      direction: "column",
      ratio: 0.55,
      children: [
        leaf("pane-codex", { kind: "session", source: "codex", id: IDS.deploy }),
        leaf("pane-term", { kind: "terminal", id: dev.id }),
      ],
    },
  ],
};
await api("PATCH", `/api/screens/${screen.id}`, { baseRevision: screen.revision, layout, emoji: "🧾" });

const staged = { screen: screen.id, terminal: shell.id, devTerminal: dev.id };
fs.writeFileSync(path.join(state, "staged.json"), JSON.stringify(staged, null, 2));
console.log(`staged (${lang}): ${JSON.stringify(staged)}`);
