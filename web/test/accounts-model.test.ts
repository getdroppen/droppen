// The rules that decide which login a run gets, now that both CLIs have
// accounts, and the rules that decide whether a pasted env block is one. Each
// has a way of going wrong that no type catches — a Claude default quietly
// deciding what a Codex session runs under, a picker offering the other CLI's
// logins, a block the server will reject after the secret was typed — so they
// live in pure functions and are pinned here.
import assert from "node:assert/strict";
import test from "node:test";
import { ENV_MAX_BYTES, ENV_MAX_VARS, parseEnvBlock } from "../src/features/accounts/env-block.js";
import {
  accountEnvKeys,
  accountKindLabel,
  accountProvider,
  accountsForProvider,
  accountUsageState,
  defaultAccountIdFor,
  modelFamily,
  startAccountFor,
  type Account,
  type AccountList,
} from "../src/features/accounts/use-accounts.js";

function account(over: Partial<Account> & Pick<Account, "id">): Account {
  return {
    label: over.id,
    kind: "oauth_token",
    provider: "claude",
    usageEnabled: false,
    rest: [],
    createdAt: "2024-05-01T12:00:00.000Z",
    updatedAt: "2024-05-01T12:00:00.000Z",
    ...over,
  };
}

const claudeOauth = account({ id: "c-oauth" });
const claudeEnv = account({ id: "c-env", kind: "env", envKeys: ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"] });
const codexEnv = account({ id: "x-env", kind: "env", provider: "codex", envKeys: ["OPENAI_API_KEY"] });

function list(over: Partial<AccountList> = {}): AccountList {
  return {
    defaultAccountId: null,
    defaultCodexAccountId: null,
    accounts: [claudeOauth, claudeEnv, codexEnv],
    ...over,
  };
}

test("each CLI follows its own default", () => {
  const data = list({ defaultAccountId: "c-env", defaultCodexAccountId: "x-env" });
  assert.equal(defaultAccountIdFor(data, "claude"), "c-env");
  assert.equal(defaultAccountIdFor(data, "codex"), "x-env");
});

test("one CLI's default never decides what the other runs under", () => {
  // The bug this pins: a Claude default leaking into a Codex spawn, which the
  // server would then refuse or — worse — honour with the wrong credential.
  const data = list({ defaultAccountId: "c-env" });
  assert.equal(defaultAccountIdFor(data, "codex"), null);
  assert.equal(defaultAccountIdFor(list({ defaultCodexAccountId: "x-env" }), "claude"), null);
});

test("a default naming the other CLI's account is no default", () => {
  assert.equal(defaultAccountIdFor(list({ defaultCodexAccountId: "c-env" }), "codex"), null);
  assert.equal(defaultAccountIdFor(list({ defaultAccountId: "x-env" }), "claude"), null);
});

test("a default that has since been deleted is no default", () => {
  assert.equal(defaultAccountIdFor(list({ defaultAccountId: "gone" }), "claude"), null);
  assert.equal(defaultAccountIdFor(list({ defaultCodexAccountId: "gone" }), "codex"), null);
});

test("no list yet means no default, not a guess", () => {
  assert.equal(defaultAccountIdFor(undefined, "claude"), null);
  assert.equal(defaultAccountIdFor(undefined, "codex"), null);
});

test("a picker only ever offers the CLI's own accounts", () => {
  assert.deepEqual(
    accountsForProvider(list().accounts, "claude").map((a) => a.id),
    ["c-oauth", "c-env"],
  );
  assert.deepEqual(
    accountsForProvider(list().accounts, "codex").map((a) => a.id),
    ["x-env"],
  );
});

test("accounts from a server that predates providers are Claude accounts", () => {
  // A new web build can reach an older server for one refresh; every account it
  // knows about is a Claude one, and none of them may vanish from the page.
  const legacy = { ...account({ id: "old" }), provider: undefined } as unknown as Account;
  assert.equal(accountProvider(legacy), "claude");
  assert.equal(accountsForProvider([legacy], "claude").length, 1);
  assert.equal(accountsForProvider([legacy], "codex").length, 0);
  assert.equal(defaultAccountIdFor({ ...list({ defaultAccountId: "old" }), accounts: [legacy] }, "claude"), "old");
});

test("a row names the kind, and an env row names the variables it sets", () => {
  assert.equal(accountKindLabel("oauth_token"), "OAuth");
  assert.equal(accountKindLabel("env"), "env");
  assert.deepEqual(accountEnvKeys(claudeEnv), ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"]);
  // An OAuth account has no block, and must not render an empty list.
  assert.deepEqual(accountEnvKeys(claudeOauth), []);
});

test("only an OAuth login has a /usage to enable", () => {
  assert.equal(accountUsageState("oauth_token", true, undefined), "ready");
  assert.equal(accountUsageState("env", false, undefined), "not_applicable");
});

// ---------------------------------------------------------------------------
// The env block
// ---------------------------------------------------------------------------

test("a block is read the way a shell would read it", () => {
  const block = parseEnvBlock(
    [
      "# a comment",
      "",
      "export ANTHROPIC_AUTH_TOKEN=sk-abc",
      '  ANTHROPIC_BASE_URL="https://relay.example.com"  ',
      "ANTHROPIC_MODEL='opus'",
      "EMPTY=",
      "ODD=a=b=c",
    ].join("\n"),
    "claude",
  );
  assert.equal(block.issue, null);
  assert.deepEqual(
    block.vars.map((v) => [v.key, v.value, v.line]),
    [
      ["ANTHROPIC_AUTH_TOKEN", "sk-abc", 3],
      // Quotes come off, and the line numbers count the comment and the blank
      // so they match what the textarea shows.
      ["ANTHROPIC_BASE_URL", "https://relay.example.com", 4],
      ["ANTHROPIC_MODEL", "opus", 5],
      ["EMPTY", "", 6],
      // Only the first `=` splits: a value may contain as many more as it likes.
      ["ODD", "a=b=c", 7],
    ],
  );
});

test("a line that is not KEY=VALUE is named by its line number", () => {
  assert.deepEqual(parseEnvBlock("A=1\njust some words\n", "claude").issue, { code: "syntax", line: 2 });
  assert.deepEqual(parseEnvBlock("A=1\nlower case=1", "claude").issue, { code: "syntax", line: 2 });
  assert.deepEqual(parseEnvBlock("A=1\n9LIVES=1", "claude").issue, { code: "syntax", line: 2 });
  assert.deepEqual(parseEnvBlock("=nokey", "claude").issue, { code: "syntax", line: 1 });
});

test("spaces around the `=` are trimmed, exactly as the server trims them", () => {
  const block = parseEnvBlock('FOO = bar\nBAZ= "q"', "claude");
  assert.equal(block.issue, null);
  assert.deepEqual(
    block.vars.map((v) => [v.key, v.value]),
    [
      ["FOO", "bar"],
      ["BAZ", "q"],
    ],
  );
});

test("OPENAI_BASE_URL is judged only where the server judges it", () => {
  // Claude never interpolates it into anything, so the server takes it as it is.
  assert.equal(parseEnvBlock("ANTHROPIC_API_KEY=k\nOPENAI_BASE_URL=not-a-url", "claude").issue, null);
});

test("a variable set twice is refused, at the line that repeats it", () => {
  assert.deepEqual(parseEnvBlock("A=1\n# note\nA=2", "claude").issue, {
    code: "duplicate",
    line: 3,
    key: "A",
  });
});

test("the variables that would move a CLI's home are refused", () => {
  for (const key of ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "HOME", "PATH"]) {
    assert.deepEqual(parseEnvBlock(`A=1\n${key}=/tmp`, "claude").issue, { code: "denied", line: 2, key });
  }
});

test("an empty block is not a credential", () => {
  assert.deepEqual(parseEnvBlock("", "claude").issue, { code: "empty" });
  assert.deepEqual(parseEnvBlock("\n\n# only a comment\n", "claude").issue, { code: "empty" });
});

test("a block has a size and a count the server will also enforce", () => {
  const many = Array.from({ length: ENV_MAX_VARS + 1 }, (_, i) => `K${i}=v`).join("\n");
  assert.deepEqual(parseEnvBlock(many, "claude").issue, { code: "too_many" });
  assert.deepEqual(parseEnvBlock(`A=${"x".repeat(ENV_MAX_BYTES)}`, "claude").issue, { code: "too_large" });
});

test("codex needs the key its provider is built from", () => {
  assert.deepEqual(parseEnvBlock("SOMETHING=1", "codex").issue, { code: "codex_key_missing" });
  assert.equal(parseEnvBlock("OPENAI_API_KEY=sk-1", "codex").issue, null);
  assert.equal(parseEnvBlock("CODEX_API_KEY=sk-1", "codex").issue, null);
  // Claude asks for no particular name: a relay may want any of several.
  assert.equal(parseEnvBlock("SOMETHING=1", "claude").issue, null);
});

test("a codex base url that is not a url would break the provider silently", () => {
  assert.deepEqual(parseEnvBlock("OPENAI_API_KEY=sk-1\nOPENAI_BASE_URL=api.openai.com", "codex").issue, {
    code: "bad_url",
    line: 2,
    key: "OPENAI_BASE_URL",
  });
  assert.equal(parseEnvBlock("OPENAI_API_KEY=sk-1\nOPENAI_BASE_URL=https://api.openai.com/v1", "codex").issue, null);
  // Empty is "leave it alone", not a broken URL.
  assert.equal(parseEnvBlock("OPENAI_API_KEY=sk-1\nOPENAI_BASE_URL=", "codex").issue, null);
});

test("an ANTHROPIC_BASE_URL ending in /v1 is warned about, not refused", () => {
  // The CLI builds `<base>/v1/messages`, so a base that already ends in `/v1`
  // reaches `/v1/v1/messages` and 404s — reported by the CLI as a problem with
  // the model that was asked for, which is where the hours go.
  const block = parseEnvBlock(
    "ANTHROPIC_AUTH_TOKEN=sk-1\nANTHROPIC_BASE_URL=https://glm.example.com/v1",
    "claude",
  );
  assert.equal(block.issue, null, "the block is still savable: it may be what they meant");
  assert.deepEqual(block.warning, { code: "base_url_v1", line: 2, key: "ANTHROPIC_BASE_URL" });
  // A trailing slash is the same URL, and the same mistake.
  assert.deepEqual(parseEnvBlock("ANTHROPIC_BASE_URL=https://h/v1/", "claude").warning, {
    code: "base_url_v1",
    line: 1,
    key: "ANTHROPIC_BASE_URL",
  });
});

test("only a whole trailing /v1 on claude's own base url is a warning", () => {
  const clean = [
    "ANTHROPIC_BASE_URL=https://glm.example.com",
    "ANTHROPIC_BASE_URL=https://h/api/v1beta",
    // A different mistake, and one this advice would not fix.
    "ANTHROPIC_BASE_URL=https://h/v1/messages",
    "ANTHROPIC_BASE_URL=",
  ];
  for (const line of clean) {
    assert.equal(parseEnvBlock(line, "claude").warning, null, line);
  }
  // OpenAI-compatible endpoints are reached at `<base>/v1/chat/completions`
  // with the `/v1` in the base, so the same tail there is right, not wrong.
  assert.equal(
    parseEnvBlock("OPENAI_API_KEY=sk-1\nOPENAI_BASE_URL=https://api.openai.com/v1", "codex").warning,
    null,
  );
});

test("a warning survives a block that also has a real problem", () => {
  // Fix the syntax error and the base url is still wrong; saying so only after
  // the first fix would be two round trips through the same box.
  const block = parseEnvBlock("ANTHROPIC_BASE_URL=https://h/v1\nnot a line", "claude");
  assert.deepEqual(block.issue, { code: "syntax", line: 2 });
  assert.equal(block.warning?.code, "base_url_v1");
});

// Where a start that names no account goes, as the list alone can tell — the
// server's `start_choice` without the readings only it has. The rule exists so
// that a lapsed local login does not take every chat and scheduled task down
// with it; what it must never do is bill an account nobody put in the pool.
/** Where the list says a start goes, and why — without the model and placement only the server fills in. */
function startOf(...args: Parameters<typeof startAccountFor>) {
  const { accountId, reason, model, placed } = startAccountFor(...args);
  assert.equal(model, null, "the list alone never chooses a model");
  assert.equal(placed, true);
  return { accountId, reason };
}

test("a signed-out local login is stood in for only by a free account in the auto-rotation pool", () => {
  const AUTH = { scope: "all", class: "auth", untilAt: null, reason: "", observedAt: "2026-10-19T05:11:15Z" };
  const later = "2099-01-01T00:00:00Z";
  const gateway = account({ id: "gateway", kind: "env", rotation: false });
  const resting = account({ id: "resting", rotation: true, rest: [{ ...AUTH, class: "session", untilAt: later }] as never });
  const opusOut = account({ id: "opus-out", rotation: true, rest: [{ ...AUTH, scope: "opus", class: "weekly", untilAt: later }] as never });
  const over = account({ id: "over", rotation: true, rest: [{ ...AUTH, class: "session", untilAt: "2000-01-01T00:00:00Z" }] as never });
  const spare = account({ id: "spare", rotation: true });
  const signedOut = (accounts: Account[], over: Partial<AccountList> = {}): AccountList => ({
    defaultAccountId: null,
    defaultCodexAccountId: null,
    accounts,
    local: { claude: { state: "signed_out", rest: [], isDefault: false }, codex: { state: "unknown", rest: [], isDefault: false } },
    ...over,
  } as AccountList);

  assert.deepEqual(startOf(signedOut([gateway, resting, opusOut, spare]), "claude", "claude-sonnet-4-5"), {
    accountId: "opus-out",
    reason: "fallback",
  }, "past the gateway (out of the pool) and the account resting on everything; an opus rest is no bar to sonnet");
  assert.equal(startOf(signedOut([gateway, resting, opusOut, spare]), "claude", "opus").accountId, "spare");
  assert.equal(
    startOf(signedOut([gateway, resting, opusOut, spare]), "claude", null).accountId,
    "spare",
    "a start naming no model is kept off every family's rest",
  );
  assert.equal(startOf(signedOut([over]), "claude").accountId, "over", "a rest that has ended is no rest");
  assert.deepEqual(
    startOf(signedOut([gateway, resting]), "claude"),
    { accountId: null, reason: "local_signed_out" },
    "nothing free in the pool: the local login, never the gateway",
  );

  const chosen = signedOut([spare]);
  chosen.local!.claude.isDefault = true;
  assert.deepEqual(startOf(chosen, "claude"), { accountId: null, reason: "local_chosen" }, "a chosen local login stands");
  assert.deepEqual(
    startOf(signedOut([gateway, spare], { defaultAccountId: "gateway" }), "claude"),
    { accountId: "gateway", reason: "default" },
    "a configured default is the user's, in the pool or not",
  );
  const known = signedOut([spare]);
  known.local!.claude.state = "unknown";
  assert.deepEqual(startOf(known, "claude"), { accountId: null, reason: "local" });

  const codexKey = account({ id: "codex-key", provider: "codex", kind: "env", rotation: false });
  const codexOut = signedOut([codexKey]);
  codexOut.local!.codex.state = "signed_out";
  assert.deepEqual(startOf(codexOut, "codex"), { accountId: null, reason: "local_signed_out" }, "codex keys stay out unless put in");
  codexKey.rotation = true;
  assert.deepEqual(startOf(codexOut, "codex"), { accountId: "codex-key", reason: "fallback" });

  assert.equal(modelFamily("claude-opus-4-1"), "opus");
  assert.equal(modelFamily("Fable"), "fable");
  assert.equal(modelFamily("gpt-5"), null);
  assert.equal(modelFamily(null), null);
});
