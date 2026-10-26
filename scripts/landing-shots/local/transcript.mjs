// Scenario steps (scenario.mjs) as the files the CLIs write: Claude Code's
// session JSONL and Codex's rollout JSONL. The seeder writes history with
// these, and the scripted `claude` writes its live turns with the same code,
// so both read exactly like the real CLIs' files to the server.

import { randomBytes, randomUUID } from "node:crypto";

export const CLAUDE_VERSION = "2.1.300";
export const CODEX_VERSION = "0.150.0";

const ALNUM = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const token = (n) => Array.from(randomBytes(n), (b) => ALNUM[b % ALNUM.length]).join("");

/** Where Claude Code keeps a project's sessions: the cwd with every non-alphanumeric as `-`. */
export const projectDirName = (cwd) => cwd.replace(/[^A-Za-z0-9]/g, "-");

/** One Claude session's transcript lines, chained the way the CLI chains them. */
export class ClaudeTranscript {
  constructor({ id, cwd, branch, model }) {
    Object.assign(this, { id, cwd, branch, model });
    this.parent = null;
  }

  entry(type, message, ts, extra = {}) {
    const uuid = randomUUID();
    const line = {
      parentUuid: this.parent,
      isSidechain: false,
      userType: "external",
      cwd: this.cwd,
      sessionId: this.id,
      version: CLAUDE_VERSION,
      gitBranch: this.branch,
      type,
      message,
      uuid,
      timestamp: ts,
      ...extra,
    };
    this.parent = uuid;
    return line;
  }

  assistant(content, ts) {
    const message = {
      id: `msg_01${token(22)}`,
      type: "message",
      role: "assistant",
      model: this.model,
      content,
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: 6, cache_creation_input_tokens: 812, cache_read_input_tokens: 24518, output_tokens: 214 },
    };
    return this.entry("assistant", message, ts, { requestId: `req_011${token(21)}` });
  }

  /**
   * The lines one step writes, each with the stream-json event the CLI prints
   * for it on stdout (`event`, absent for lines it does not print). `pending`
   * names the tool call a step leaves open, for the caller that has to answer
   * for it (a question, a command still running).
   */
  step(step, ts) {
    if (step.title !== undefined) {
      return { lines: [{ line: { type: "custom-title", customTitle: step.title, sessionId: this.id } }] };
    }
    if (step.user !== undefined) {
      const line = this.entry("user", { role: "user", content: step.user }, ts);
      return { lines: [{ line }] };
    }
    if (step.say !== undefined) {
      const line = this.assistant([{ type: "text", text: step.say }], ts);
      return { lines: [{ line, event: this.event(line) }] };
    }
    if (step.ask !== undefined) {
      const id = `toolu_01${token(22)}`;
      const line = this.assistant([{ type: "tool_use", id, name: "AskUserQuestion", input: step.ask }], ts);
      return { lines: [{ line, event: this.event(line) }], pending: { id, name: "AskUserQuestion", input: step.ask } };
    }
    if (step.tool !== undefined) {
      const id = `toolu_01${token(22)}`;
      const call = this.assistant([{ type: "tool_use", id, name: step.tool, input: step.input }], ts);
      const lines = [{ line: call, event: this.event(call) }];
      if (step.result === undefined) return { lines, pending: { id, name: step.tool, input: step.input } };
      const block = { tool_use_id: id, type: "tool_result", content: step.result };
      if (step.error) block.is_error = true;
      const result = this.entry("user", { role: "user", content: [block] }, ts, { toolUseResult: toolUseResult(step) });
      lines.push({ line: result, event: this.event(result) });
      return { lines };
    }
    throw new Error(`unknown claude step: ${JSON.stringify(step)}`);
  }

  event(line) {
    return { type: line.type, message: line.message, parent_tool_use_id: null, session_id: this.id, uuid: line.uuid };
  }
}

/** What Claude Code records beside a tool's result: an edit's diff, a command's output. */
function toolUseResult(step) {
  if ((step.tool === "Edit" || step.tool === "Write") && step.patch) {
    const lines = step.patch.lines;
    const oldLines = lines.filter((l) => !l.startsWith("+")).length;
    const newLines = lines.filter((l) => !l.startsWith("-")).length;
    return {
      filePath: step.input.file_path,
      oldString: step.input.old_string ?? "",
      newString: step.input.new_string ?? step.input.content ?? "",
      structuredPatch: [{ oldStart: step.patch.start, oldLines, newStart: step.patch.start, newLines, lines }],
      userModified: false,
      replaceAll: false,
    };
  }
  if (step.tool === "Write") return { type: "create", filePath: step.input.file_path, content: step.input.content, structuredPatch: [] };
  if (step.tool === "Bash") return { stdout: step.result, stderr: "", interrupted: false, isImage: false };
  return step.result;
}

/** A Codex rollout (`~/.codex/sessions/YYYY/MM/DD/rollout-…-<id>.jsonl`). */
export class CodexRollout {
  constructor({ id, cwd, model }) {
    Object.assign(this, { id, cwd, model });
  }

  open(ts) {
    return [
      {
        timestamp: ts,
        type: "session_meta",
        payload: { id: this.id, timestamp: ts, cwd: this.cwd, originator: "codex_cli_rs", cli_version: CODEX_VERSION, source: "cli", model_provider: "openai" },
      },
      {
        timestamp: ts,
        type: "turn_context",
        payload: { cwd: this.cwd, approval_policy: "on-request", sandbox_policy: { type: "workspace-write" }, model: this.model, effort: "medium", summary: "auto" },
      },
    ];
  }

  step(step, ts) {
    const item = (payload) => ({ timestamp: ts, type: "response_item", payload });
    if (step.user !== undefined) {
      return [
        item({ type: "message", role: "user", content: [{ type: "input_text", text: step.user }] }),
        { timestamp: ts, type: "event_msg", payload: { type: "user_message", message: step.user, images: [] } },
      ];
    }
    if (step.say !== undefined) {
      return [
        item({ type: "message", role: "assistant", content: [{ type: "output_text", text: step.say }] }),
        { timestamp: ts, type: "event_msg", payload: { type: "agent_message", message: step.say } },
      ];
    }
    if (step.exec !== undefined) {
      const call = `call_${token(24)}`;
      const output = `Chunk ID: ${randomBytes(3).toString("hex")}\nWall time: 0.${String(100 + (randomBytes(1)[0] % 800)).padStart(4, "0")} seconds\nProcess exited with code ${step.exit ?? 0}\nOriginal token count: ${Math.ceil(step.output.length / 4)}\nOutput:\n${step.output}\n`;
      return [
        item({ type: "function_call", name: "exec_command", arguments: JSON.stringify({ cmd: step.exec, workdir: this.cwd }), call_id: call }),
        item({ type: "function_call_output", call_id: call, output }),
      ];
    }
    if (step.patch !== undefined) {
      const call = `call_${token(24)}`;
      const touched = [...step.patch.matchAll(/^\*\*\* (Add|Update|Delete) File: (.+)$/gm)].map((m) => `${m[1][0]} ${m[2]}`).join("\n");
      return [
        item({ type: "custom_tool_call", status: "completed", call_id: call, name: "apply_patch", input: step.patch }),
        item({
          type: "custom_tool_call_output",
          call_id: call,
          output: JSON.stringify({ output: `Success. Updated the following files:\n${touched}\n`, metadata: { exit_code: 0, duration_seconds: 0.0 } }),
        }),
      ];
    }
    throw new Error(`unknown codex step: ${JSON.stringify(step)}`);
  }

  close(ts) {
    return [{ timestamp: ts, type: "event_msg", payload: { type: "task_complete", last_agent_message: null } }];
  }
}
