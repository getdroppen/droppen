import assert from "node:assert/strict";
import test from "node:test";
import { canCreateConversation, creationRequest, resolveCreatedSession, type NewConversation, type PendingCreatedSession } from "../src/features/sessions/new-session-flow.js";

test("a created session resolves only after the delayed global feed includes it", () => {
  const pending: PendingCreatedSession = {
    source: "codex",
    id: "12345678-1234-4234-8234-123456789abc",
  };
  const existing = [
    {
      source: "codex" as const,
      id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    },
  ];

  assert.equal(resolveCreatedSession(pending, existing), null);
  assert.deepEqual(resolveCreatedSession(pending, [...existing, pending]), pending);
});

const session: NewConversation = { kind: "session", source: "claude", cwd: " /project ", text: " hello ", model: "fable", effort: null, permission: "acceptEdits", accountId: null };
const chat: NewConversation = { kind: "chat", source: "codex", text: " hello ", model: "gpt-5.5", effort: "high", providerReady: false };

test("both CLI chat backends work without an API provider", () => {
  for (const source of ["claude", "codex"] as const) {
    assert.equal(canCreateConversation({ ...chat, source }, false), true);
  }
});

test("only API chats require a configured provider and model", () => {
  assert.equal(canCreateConversation({ ...chat, source: "api" }, false), false);
  assert.equal(canCreateConversation({ ...chat, source: "api", providerReady: true }, false), true);
  assert.equal(canCreateConversation({ ...chat, source: "api", providerReady: true, model: "" }, false), false);
});

test("empty messages, pending creation and missing session directories are blocked", () => {
  for (const input of [session, chat]) {
    assert.equal(canCreateConversation({ ...input, text: "  " }, false), false);
    assert.equal(canCreateConversation(input, true), false);
  }
  assert.equal(canCreateConversation({ ...session, cwd: "  " }, false), false);
});

test("session requests trim text/cwd and preserve explicit local login and omitted effort", () => {
  assert.deepEqual(creationRequest(session), { path: "/api/sessions/new", body: {
    source: "claude", cwd: "/project", text: "hello", model: "fable", permission: "acceptEdits", accountId: null,
  } });
  assert.equal("accountId" in creationRequest({ ...session, accountId: undefined }).body, false);
  assert.equal(creationRequest({ ...session, accountId: "account-1" }).body.accountId, "account-1");
});

test("a codex session carries its account like a claude one", () => {
  // Codex accounts came later, and the request used to drop `accountId` for
  // them: a picked Codex login silently became the local one.
  const codex = creationRequest({ ...session, source: "codex", accountId: "codex-account" }).body;
  assert.equal(codex.accountId, "codex-account");
  assert.equal("accountId" in creationRequest({ ...session, source: "codex", accountId: undefined }).body, false);
  assert.equal(creationRequest({ ...session, source: "codex", accountId: null }).body.accountId, null);
});

test("CLI chats retain their separate persona endpoint and no account/permission override", () => {
  assert.deepEqual(creationRequest(chat), { path: "/api/chats/new", body: {
    backend: "codex", text: "hello", model: "gpt-5.5", effort: "high",
  } });
});

test("a CLI chat names its login only when one was picked: absent follows the server's start rule", () => {
  assert.equal("accountId" in creationRequest(chat).body, false, "untouched: the server decides, as it always did");
  assert.equal((creationRequest({ ...chat, accountId: null }).body as { accountId?: unknown }).accountId, null, "the local login, picked by hand");
  assert.equal((creationRequest({ ...chat, accountId: "account-1" }).body as { accountId?: unknown }).accountId, "account-1");
  assert.equal("accountId" in creationRequest({ ...chat, source: "api", accountId: "account-1" }).body, false, "an API chat has no CLI login");
});

test("API chat creation does not send the first message or CLI effort twice", () => {
  assert.deepEqual(creationRequest({ ...chat, source: "api" }), { path: "/api/chats/new", body: { backend: "api", model: "gpt-5.5" } });
});

test("discovery matches source and id, never another CLI with the same id", () => {
  const pending: PendingCreatedSession = { source: "codex", id: "shared-id" };
  assert.equal(resolveCreatedSession(pending, [{ source: "claude", id: pending.id }]), null);
  assert.equal(resolveCreatedSession(pending, null), null);
  assert.equal(resolveCreatedSession(null, [pending]), null);
  assert.deepEqual(resolveCreatedSession(pending, [pending]), pending);
});

test("ACP session creation includes permission when non-empty and omits it when empty", () => {
  const base: NewConversation = {
    kind: "session",
    source: "acp",
    acpAgent: "claude-acp",
    cwd: "/project",
    text: "hello",
    model: "",
    effort: null,
    permission: "",
  };
  assert.equal("permission" in creationRequest(base).body, false);
  const withMode = creationRequest({ ...base, permission: "accept-edits" }).body;
  assert.equal(withMode.permission, "accept-edits");
  assert.equal(withMode.acpAgent, "claude-acp");
});
