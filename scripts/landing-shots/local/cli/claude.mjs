#!/usr/bin/env node
// A scripted `claude` for the demo: speaks the CLI's stream-json protocol to
// the server and writes the session transcript the way Claude Code does, but
// plays a turn from the scenario instead of asking a model.
//
// The server runs it exactly as it runs the real CLI (through its session
// shim, with `--resume <id> --permission-prompt-tool stdio …`). For a session
// that has a turn in <DEMO_STATE>/live/<id>.json, the first message it is sent
// plays that turn, which ends either
//
// * "ask": with an AskUserQuestion the CLI blocks on — the `can_use_tool`
//   control request the server turns into a pending question, held until
//   somebody answers it; or
// * "run": in a tool call that never returns, so the session stays working.
//
// Anything else gets a short reply. It also answers the server's side probes:
// `--version`, `auth status`, `/usage` and the model-list handshake.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { randomUUID } from "node:crypto";
import { CLAUDE_MODEL } from "../scenario.mjs";
import { CLAUDE_VERSION, ClaudeTranscript, projectDirName } from "../transcript.mjs";

const args = process.argv.slice(2);
const out = (value) => process.stdout.write(JSON.stringify(value) + "\n");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

if (args[0] === "--version" || args[0] === "-v") {
  console.log(`${CLAUDE_VERSION} (Claude Code)`);
  process.exit(0);
}
if (args[0] === "auth") {
  console.log(JSON.stringify({ loggedIn: true, authMethod: "claude.ai", apiProvider: "firstParty" }));
  process.exit(0);
}
const after = (flag) => {
  const i = args.indexOf(flag);
  return i >= 0 ? args[i + 1] : undefined;
};
const id = after("--resume") ?? after("--session-id");
if (args.includes("/usage")) {
  // A signed-in login's plan usage, as `/usage` prints it.
  const at = (hours) => {
    const d = new Date(Date.now() + hours * 3600e3);
    const month = d.toLocaleString("en-US", { month: "short", timeZone: "UTC" });
    const hour = d.getUTCHours() % 12 || 12;
    return `${month} ${d.getUTCDate()} at ${hour}${d.getUTCHours() < 12 ? "am" : "pm"} (UTC)`;
  };
  const text = `Current session: 23% used · resets ${at(3)}\nCurrent week (all models): 41% used · resets ${at(76)}\n`;
  console.log(JSON.stringify({ type: "result", subtype: "success", is_error: false, result: text }));
  process.exit(0);
}
const streaming = args.includes("--input-format");
if (!streaming) process.exit(0); // anything else one-shot: nothing to say.

const MODELS = [
  { value: "default", displayName: "Default (recommended)", resolvedModel: CLAUDE_MODEL, supportedEffortLevels: ["low", "medium", "high", "xhigh", "max"] },
  { value: "opus", displayName: "Opus", resolvedModel: "claude-opus-5-5", supportedEffortLevels: ["low", "medium", "high", "xhigh", "max"] },
  { value: "sonnet", displayName: "Sonnet", resolvedModel: "claude-sonnet-5-5", supportedEffortLevels: ["low", "medium", "high"] },
  { value: "haiku", displayName: "Haiku", resolvedModel: "claude-haiku-5-5", supportsEffort: false },
];

// --------------------------------------------------------------- the session

const state = process.env.DEMO_STATE ?? "/opt/demo/state";
const liveFile = id ? path.join(state, "live", `${id}.json`) : null;
const live = liveFile && fs.existsSync(liveFile) ? JSON.parse(fs.readFileSync(liveFile, "utf8")) : null;
const cwd = live?.cwd ?? process.cwd();
const root = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), ".claude");

/** The transcript to append to: where Claude Code keeps it, else wherever this id already is. */
function transcriptFile() {
  const projects = path.join(root, "projects");
  const own = path.join(projects, projectDirName(cwd), `${id}.jsonl`);
  if (fs.existsSync(own)) return own;
  for (const dir of fs.existsSync(projects) ? fs.readdirSync(projects) : []) {
    const candidate = path.join(projects, dir, `${id}.jsonl`);
    if (fs.existsSync(candidate)) return candidate;
  }
  return own;
}

const file = id ? transcriptFile() : null;
const transcript = new ClaudeTranscript({ id, cwd, branch: live?.branch ?? "main", model: CLAUDE_MODEL });
if (file && fs.existsSync(file)) {
  const last = fs
    .readFileSync(file, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .filter((line) => line.uuid)
    .pop();
  transcript.parent = last?.uuid ?? live?.parent ?? null;
}

const append = (lines) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.appendFileSync(file, lines.map((line) => JSON.stringify(line) + "\n").join(""));
};

/** Write one step to the transcript and print what the CLI prints for it. */
function play(step) {
  const written = transcript.step(step, new Date().toISOString());
  append(written.lines.map(({ line }) => line));
  for (const { event } of written.lines) if (event) out(event);
  return written.pending;
}

function finish(text) {
  out({
    type: "result",
    subtype: "success",
    is_error: false,
    duration_ms: 8421,
    duration_api_ms: 7710,
    num_turns: 1,
    result: text,
    session_id: id,
    total_cost_usd: 0,
    usage: { input_tokens: 6, output_tokens: 120 },
  });
}

out({
  type: "system",
  subtype: "init",
  cwd,
  session_id: id,
  tools: ["Bash", "Edit", "Glob", "Grep", "Read", "Write", "AskUserQuestion", "TodoWrite", "WebFetch"],
  mcp_servers: [],
  model: CLAUDE_MODEL,
  permissionMode: "default",
  apiKeySource: "none",
  claude_code_version: CLAUDE_VERSION,
  uuid: randomUUID(),
});

let played = false;
let question = null;

async function turn(text) {
  play({ user: text });
  if (!live || played) {
    await sleep(600);
    play({ say: "On it." });
    finish("On it.");
    return;
  }
  played = true;
  // The first step is the user's message, which was just written.
  for (const step of live.steps.slice(1)) {
    await sleep(step.say !== undefined ? 500 : 350);
    const pending = play(step);
    if (pending?.name === "AskUserQuestion") {
      question = { requestId: randomUUID(), toolUseId: pending.id, input: pending.input };
      out({
        type: "control_request",
        request_id: question.requestId,
        request: { subtype: "can_use_tool", tool_name: "AskUserQuestion", input: pending.input, tool_use_id: pending.id },
      });
      return; // blocked until the answer arrives as a control_response
    }
    if (pending) return; // a command that is still running: the turn stays open
  }
  finish(live.steps.at(-1)?.say ?? "");
}

const input = readline.createInterface({ input: process.stdin });
for await (const raw of input) {
  let line;
  try {
    line = JSON.parse(raw);
  } catch {
    continue;
  }
  if (line.type === "control_request") {
    const subtype = line.request?.subtype;
    const response = subtype === "initialize" ? { models: MODELS, commands: [], output_style: "default" } : {};
    out({ type: "control_response", response: { subtype: "success", request_id: line.request_id, response } });
    continue;
  }
  if (line.type === "control_response" && question && line.response?.request_id === question.requestId) {
    // The user's answer: the tool call returns it and the agent carries on.
    const answers = line.response?.response?.updatedInput?.answers ?? {};
    const chosen = Object.values(answers).join(", ") || "(no answer)";
    const block = { tool_use_id: question.toolUseId, type: "tool_result", content: `User has answered your questions: ${chosen}` };
    const entry = transcript.entry("user", { role: "user", content: [block] }, new Date().toISOString());
    append([entry]);
    out(transcript.event(entry));
    question = null;
    await sleep(400);
    play({ say: `Done: the default is now ${chosen}.` });
    finish("Done.");
    continue;
  }
  if (line.type === "user") {
    const content = line.message?.content;
    const text = typeof content === "string" ? content : (content ?? []).filter((b) => b.type === "text").map((b) => b.text).join("\n");
    await turn(text);
  }
}
