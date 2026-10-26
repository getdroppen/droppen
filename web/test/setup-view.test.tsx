// The setup page: the first account of a device whose server has none yet.
//
// What is pinned: nobody signed in — on the device itself or from another
// one, which the server cannot tell apart — is told how to open the page
// signed in from the device itself (`droppen open`, in the command this
// device's server reports), with the
// one-time setup code folded away underneath, and why a `#local-open=` link
// that did not work did not; the code form asks for the code, a name and the
// password twice, with the autofill hints a phone needs; this device's owner
// (a page opened that way) is asked for no code, sends none, and leaves on
// the new account's token; a form that breaks one of the server's rules is
// refused here, in the reader's language, without a request (a wrong guess at
// the code costs the person a wait, so a malformed one must never reach the
// server); each refusal the server gives reads as its own sentence; and an
// accepted setup leaves the page signed in, exactly as a login does.
//
// There is no headless browser in this repo, so typing and clicking cannot be
// driven: the form's logic is `submitSetup`, which the page calls on submit,
// and the markup is checked with `react-dom/server`.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";

// ---------------------------------------------------------------------------
// A server
// ---------------------------------------------------------------------------

type Sent = { url: string; method: string; body: unknown };
const sent: Sent[] = [];
/** What `POST /api/auth/setup` answers next. */
let answer: { status: number; body: unknown } = { status: 500, body: { error: "no answer set" } };
/** What `GET /api/auth/status` answers. */
let statusAnswer: Record<string, unknown> = { authRequired: true, setupRequired: true, authOff: false, user: null, desktopApp: false };

// `openapi-fetch` builds a `Request` before it calls `fetch`, and node cannot
// parse the app's relative URLs, so the constructor is replaced with one that
// keeps what the test reads. In place before the client is first imported.
class FakeRequest {
  url: string;
  method: string;
  headers: Headers;
  body: unknown;
  constructor(url: string, init: { method?: string; headers?: HeadersInit; body?: unknown } = {}) {
    this.url = url;
    this.method = init.method ?? "GET";
    this.headers = new Headers(init.headers);
    this.body = init.body;
  }
}
(globalThis as unknown as { Request: unknown }).Request = FakeRequest;

(globalThis as unknown as { fetch: unknown }).fetch = async (req: FakeRequest) => {
  sent.push({ url: req.url, method: req.method, body: typeof req.body === "string" ? JSON.parse(req.body) : req.body });
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  if (req.url === "/api/auth/setup") return reply(answer.status, answer.body);
  if (req.url === "/api/auth/ticket") return reply(200, { ticket: "T1", expiresAt: "2026-10-19T00:10:00Z" });
  if (req.url === "/api/auth/status") return reply(200, statusAnswer);
  return reply(404, { error: `unexpected request: ${req.url}` });
};

const { default: i18n } = await import("./i18n-all.js");
const { ApiError } = await import("../src/api/errors.js");
const { authStatus, currentUser, forgetAuthStatus, getToken, setToken } = await import("../src/lib/auth.js");
const { localOpenRefusal, SetupPage, SetupProblemLine, SetupView, setupFormProblem, setupRefusal, submitSetup, normalizeSetupCode } =
  await import("../src/features/auth/setup-view.js");

type Problem = Parameters<typeof SetupProblemLine>[0]["problem"];

async function inLanguage<T>(lng: string, fn: () => T): Promise<T> {
  await i18n.changeLanguage(lng);
  try {
    return fn();
  } finally {
    await i18n.changeLanguage("en");
  }
}

const said = (problem: Problem) => renderToStaticMarkup(<SetupProblemLine problem={problem} />);

const GOOD = { code: "abcd-efgh-jkmn-pqrs", username: "admin", password: "a long password here", confirm: "a long password here" };

await i18n.changeLanguage("en");

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/** The `<details>` element the code form is folded into, opening tag only. */
const foldOf = (html: string) => /<details[^>]*data-testid="setup-with-code"[^>]*>/.exec(html)?.[0] ?? "";

test("nobody signed in: how to open the page signed in from this device, and the setup code folded away", () => {
  const html = renderToStaticMarkup(<SetupView />);
  assert.match(html, /data-testid="setup-view"/);
  assert.match(html, /data-owner="false"/);
  assert.match(html, /set up this device/);
  assert.match(html, /This device has no account yet\./);
  assert.match(html, /On this device: open Droppen from the app, or run `remote-hand-server open` in a terminal — it opens this page signed in\./);
  assert.match(html, /<pre[^>]*>remote-hand-server open<\/pre>/);
  // The way in from the device itself comes first; the code is folded, and closed.
  const fold = foldOf(html);
  assert.ok(fold, "the code form is folded into a <details>");
  assert.doesNotMatch(fold, /\sopen/, "closed until asked for");
  assert.ok(html.indexOf("setup-local-open") < html.indexOf("setup-with-code"));
  assert.match(html, /Or create the first account with the one-time setup code this device&#x27;s server keeps \(`remote-hand-server setup-code` on this device shows it\)/);
  assert.match(html, /remote-hand-server user add &lt;name&gt;/);
  assert.doesNotMatch(html, /setup-owner|setup-local-open-error|setup-error/);
});

test("the folded form asks for the code, a name and the password twice", () => {
  const html = renderToStaticMarkup(<SetupPage owner={false} />);
  const folded = html.slice(html.indexOf("setup-with-code"));
  assert.match(folded, /data-with-code="true"/);
  // The code: a one-time code to a phone's autofill, upper case, no spelling help.
  const code = /<input[^>]*data-testid="setup-code"[^>]*>/.exec(folded)?.[0] ?? "";
  assert.match(code, /autoComplete="one-time-code"/i);
  assert.match(code, /autoCapitalize="characters"/i);
  assert.match(code, /spellCheck="false"/i);
  assert.match(code, /placeholder="ABCD-EFGH-JKMN-PQRS"/);
  const name = /<input[^>]*data-testid="setup-username"[^>]*>/.exec(folded)?.[0] ?? "";
  assert.match(name, /autoComplete="username"/i);
  for (const id of ["setup-password", "setup-confirm"]) {
    const input = new RegExp(`<input[^>]*data-testid="${id}"[^>]*>`).exec(folded)?.[0] ?? "";
    assert.match(input, /type="password"/, id);
    assert.match(input, /autoComplete="new-password"/i, id);
  }
  // Nothing to send until every field is filled.
  assert.match(folded, /<button[^>]*disabled=""[^>]*data-testid="setup-submit"/);
});

test("a local-open link that did not sign the page in says why, on the setup page", async () => {
  const page = (error: unknown) => renderToStaticMarkup(<SetupPage owner={false} localOpenError={error} />);
  const used = page(new ApiError({ error: "invalid_local_open", hint: "x" }));
  assert.match(used, /data-testid="setup-local-open-error"/);
  assert.match(used, /That link was already used or has expired\. Run `remote-hand-server open` on this device again\./);
  // Still above the ways in, which stay.
  assert.ok(used.indexOf("setup-local-open-error") < used.indexOf('data-testid="setup-local-open"'));
  assert.match(await inLanguage("zh-CN", () => page(new ApiError({ error: "invalid_local_open" }))), /这个链接已经用过或已过期/);
  const taken = page(new ApiError({ error: "accounts_exist", hint: "x" }));
  assert.match(taken, /This device has an account now: sign in\./);
  assert.match(taken, /<a href="\/login" data-testid="setup-sign-in"/);
  assert.match(page(new ApiError({ error: "not_from_cloud" })), /only on the device itself, not through Droppen/);
  assert.deepEqual(localOpenRefusal(new TypeError("Failed to fetch")), { text: "Failed to fetch" });
});

test("this device's owner is asked for a name and a password only", () => {
  const html = renderToStaticMarkup(<SetupPage owner />);
  assert.match(html, /data-owner="true"/);
  assert.match(html, /data-testid="setup-owner"/);
  assert.match(html, /You opened this page as this device&#x27;s owner\. This device has no account yet: create one to sign in from other devices; from then on every browser signs in, this one too\./);
  assert.match(html, /data-with-code="false"/);
  assert.doesNotMatch(html, /setup-code|setup-with-code|setup-local-open/);
  for (const id of ["setup-username", "setup-password", "setup-confirm", "setup-submit"]) assert.match(html, new RegExp(`data-testid="${id}"`), id);
});

test("the setup page reads in the reader's language, and calls the device this device", async () => {
  const zh = await inLanguage("zh-CN", () => renderToStaticMarkup(<SetupView />));
  assert.match(zh, /设置这台设备/);
  assert.match(zh, /这台设备还没有账号。/);
  assert.match(zh, /它会以已登录的状态打开这个页面/);
  assert.match(zh, /设置码/);
  assert.match(zh, /再输一次密码/);
  assert.match(zh, /创建账号并登录/);
  const tw = await inLanguage("zh-TW", () => renderToStaticMarkup(<SetupView />));
  assert.match(tw, /這台裝置還沒有帳號。/);
  const ja = await inLanguage("ja", () => renderToStaticMarkup(<SetupView />));
  assert.match(ja, /このデバイスにはまだアカウントがありません。/);
  const owner = await inLanguage("zh-CN", () => renderToStaticMarkup(<SetupPage owner />));
  assert.match(owner, /你以这台设备主人的身份打开了这个页面/);
});

test("the commands are said the way this device's server reports itself", async () => {
  const installed = "~/.local/share/remote-hand/bin/droppen";
  statusAnswer = { authRequired: true, setupRequired: true, authOff: false, user: null, desktopApp: false, droppenPath: installed };
  forgetAuthStatus();
  try {
    await authStatus();
    const html = renderToStaticMarkup(<SetupPage owner={false} />);
    assert.match(html, /<pre[^>]*>~\/\.local\/share\/remote-hand\/bin\/droppen open<\/pre>/);
    assert.match(html, /run `~\/\.local\/share\/remote-hand\/bin\/droppen open` in a terminal/);
    assert.match(html, /\(`~\/\.local\/share\/remote-hand\/bin\/droppen setup-code` on this device shows it\)/);
    assert.match(html, /~\/\.local\/share\/remote-hand\/bin\/droppen user add &lt;name&gt;/);
    assert.doesNotMatch(html, /remote-hand-server/);
    const zh = await inLanguage("zh-CN", () => renderToStaticMarkup(<SetupPage owner={false} />));
    assert.match(zh, /在终端里运行 `~\/\.local\/share\/remote-hand\/bin\/droppen open`/);
    assert.doesNotMatch(zh, /remote-hand-server/);
    assert.deepEqual(localOpenRefusal(new ApiError({ error: "invalid_local_open" })), {
      key: "That link was already used or has expired. Run `{{command}} open` on this device again.",
      values: { command: installed },
    });

    // A second instance (the desktop app's, say): the commands reach it,
    // not the default one, whose setup code would be a wrong one here.
    statusAnswer = { ...statusAnswer, droppenEnv: [{ name: "APP_INSTANCE", value: "droppen-app" }] };
    forgetAuthStatus();
    await authStatus();
    const app = renderToStaticMarkup(<SetupPage owner={false} />);
    assert.match(app, /<pre[^>]*>APP_INSTANCE=droppen-app ~\/\.local\/share\/remote-hand\/bin\/droppen open<\/pre>/);
    assert.match(app, /\(`APP_INSTANCE=droppen-app ~\/\.local\/share\/remote-hand\/bin\/droppen setup-code` on this device shows it\)/);
    assert.match(app, /APP_INSTANCE=droppen-app ~\/\.local\/share\/remote-hand\/bin\/droppen user add &lt;name&gt;/);
  } finally {
    forgetAuthStatus();
  }
});

test("on the desktop app's server: Open in Browser, and no code or `open` to offer", async () => {
  // Its server keeps no setup code, and its control socket opens nothing.
  statusAnswer = { authRequired: true, setupRequired: true, authOff: false, user: null, desktopApp: true, droppenPath: "/Applications/Droppen.app/Contents/MacOS/remote-hand-server" };
  forgetAuthStatus();
  try {
    await authStatus();
    const html = renderToStaticMarkup(<SetupPage owner={false} />);
    assert.match(html, /data-testid="setup-desktop-app"/);
    assert.match(html, /choose Open in Browser: it opens this page signed in/);
    assert.match(html, /remote-hand-server user add &lt;name&gt;/);
    assert.doesNotMatch(html, /setup-with-code|setup-local-open|setup-code/);
    const zh = await inLanguage("zh-CN", () => renderToStaticMarkup(<SetupPage owner={false} />));
    assert.match(zh, /在浏览器中打开/);
    // The owner a page from the app was opened as still makes the first account.
    assert.match(renderToStaticMarkup(<SetupPage owner />), /data-testid="setup-owner"/);
  } finally {
    forgetAuthStatus();
  }
});

// ---------------------------------------------------------------------------
// The rules, before anything is sent
// ---------------------------------------------------------------------------

test("a form that breaks a rule is refused here, translated, and nothing is sent", async () => {
  sent.length = 0;
  const cases: [Partial<typeof GOOD>, string][] = [
    [{ confirm: "a long password herE" }, "the two do not match"],
    [{ password: "short", confirm: "short" }, "a password is at least 12 characters long"],
    [{ password: "x".repeat(257), confirm: "x".repeat(257) }, "a password is at most 256 bytes long"],
    [{ password: " a long password", confirm: " a long password" }, "a password may not start or end with whitespace"],
    [{ username: "administrator", password: "ADMINISTRATOR", confirm: "ADMINISTRATOR" }, "a password may not be the user name"],
    [{ username: "Admin" }, "a user name is 1-32 characters of a-z, 0-9, '.', '_' or '-'"],
    [{ username: "a".repeat(33) }, "a user name is 1-32 characters of a-z, 0-9, '.', '_' or '-'"],
    [{ code: "ABCD-EFGH" }, "a setup code is 16 letters and digits, like ABCD-EFGH-JKMN-PQRS"],
    [{ code: "ABCD-EFGH-JKMN-PQRU" }, "a setup code is 16 letters and digits, like ABCD-EFGH-JKMN-PQRS"],
  ];
  for (const [change, key] of cases) {
    const problem = await submitSetup({ ...GOOD, ...change });
    assert.deepEqual(problem, { key }, JSON.stringify(change));
  }
  assert.deepEqual(sent, [], "not one request");
  // The owner is asked for no code, so none is checked.
  assert.equal(setupFormProblem({ ...GOOD, code: "" }, false), null);
  assert.deepEqual(await submitSetup({ ...GOOD, code: "", confirm: "x" }, false), { key: "the two do not match" });
  assert.deepEqual(sent, [], "still not one request");
  // Twelve characters are twelve code points, whatever their bytes.
  assert.equal(setupFormProblem({ ...GOOD, password: "密码密码密码密码密码密码", confirm: "密码密码密码密码密码密码" }), null);
  assert.equal(setupFormProblem({ ...GOOD, password: "密码密码密码密码密码密", confirm: "密码密码密码密码密码密" })?.key, "a password is at least 12 characters long");
  // The code is read the way the server reads it.
  assert.equal(normalizeSetupCode(" abcd efgh-jkmn-pqrs "), "ABCDEFGHJKMNPQRS");
  assert.equal(normalizeSetupCode("o1Il-0000-0000-0000"), "0111000000000000");
  assert.equal(setupFormProblem({ ...GOOD, code: "oooo iiii llll 2222" }), null);

  const zh = await inLanguage("zh-CN", () => said({ key: "the two do not match" }) + said({ key: "a password is at least 12 characters long" }));
  assert.match(zh, /两次输入不一致/);
  assert.match(zh, /密码至少要 12 个字符/);
  assert.match(zh, /data-signal="failed"/);
});

// ---------------------------------------------------------------------------
// The server's refusals
// ---------------------------------------------------------------------------

test("each refusal of the server reads as its own message", async () => {
  const refused = (body: Record<string, unknown>) => setupRefusal(new ApiError(body));
  assert.match(said(refused({ error: "wrong_setup_code", hint: "that is not the setup code" })), /not the setup code this device&#x27;s server printed/);
  assert.match(said(refused({ error: "not_from_cloud", hint: "x" })), /made on this device&#x27;s own page, not through Droppen/);
  assert.match(said(refused({ error: "no_setup_code", hint: "x" })), /has no setup code\. Restart it/);
  assert.match(said(refused({ error: "name_taken", hint: "x" })), /That name belongs to the Droppen app&#x27;s own account on this device\. Pick another\./);
  assert.match(said(refused({ error: "code_required", hint: "x" })), /The setup code is needed now\./);

  // Too many guesses: how long to wait, in the reader's language too.
  const waiting = refused({ error: "too_many_attempts", hint: "x", retryAfterSeconds: 42 });
  assert.match(said(waiting), /Try again in 42 s\./);
  assert.match(await inLanguage("zh-CN", () => said(waiting)), /请 42 秒后再试/);
  assert.match(await inLanguage("ja", () => said(waiting)), /42 秒後/);
  assert.match(said(refused({ error: "too_many_attempts", hint: "x" })), /Wait a little, then try again\./);

  // An account already: the way to the login page, by a document load.
  const done = said(refused({ error: "setup_done", hint: "x" }));
  assert.match(done, /This device already has an account\./);
  assert.match(done, /<a href="\/login" data-testid="setup-sign-in"[^>]*>sign in<\/a>/);
  assert.match(said(refused({ error: "auth_off", hint: "x" })), /<a href="\/" data-testid="setup-home"/);

  // A rule only the server knows reads as its sentence, translated when it is one of ours.
  const common = refused({ error: "invalid_account", hint: "that password is one of the most common ones; pick another" });
  assert.match(said(common), /one of the most common ones/);
  assert.match(await inLanguage("zh-CN", () => said(common)), /这个密码太常见了/);
  assert.match(said(refused({ error: "invalid_account", hint: "something new" })), /something new/);
  // Anything else: the server's words, or the failure's.
  assert.match(said(refused({ error: "boom", hint: "the disk is full" })), /the disk is full/);
  assert.match(said(setupRefusal(new TypeError("Failed to fetch"))), /Failed to fetch/);
});

test("a refused setup keeps no token", async () => {
  sent.length = 0;
  answer = { status: 403, body: { error: "wrong_setup_code", hint: "that is not the setup code this server printed when it started" } };
  const problem = await submitSetup(GOOD);
  assert.deepEqual(problem, {
    key: "That is not the setup code this device's server printed. `{{command}} setup-code` on this device shows it again.",
    values: { command: "remote-hand-server" },
  });
  assert.equal(sent.length, 1);
  assert.equal(getToken(), null);
});

test("a setup the server accepts signs the page in, as a login does", async () => {
  sent.length = 0;
  answer = { status: 200, body: { token: "tok-1", expiresAt: "2026-11-19T00:00:00Z", username: "admin" } };
  const problem = await submitSetup({ ...GOOD, code: "  abcd-efgh-jkmn-pqrs ", username: " admin " });
  assert.equal(problem, null);
  // The code as typed — the server reads it — and the name trimmed.
  assert.deepEqual(sent[0], {
    url: "/api/auth/setup",
    method: "POST",
    body: { code: "abcd-efgh-jkmn-pqrs", username: "admin", password: "a long password here" },
  });
  assert.equal(getToken(), "tok-1");
  assert.equal(currentUser(), "admin");
});

test("this device's owner sends no code, and the page goes on with the new account's token", async () => {
  // The page is the owner's, by a local-open session.
  setToken("owner-token");
  sent.length = 0;
  answer = { status: 200, body: { token: "tok-2", expiresAt: "2026-11-19T00:00:00Z", username: "owner" } };
  const problem = await submitSetup({ code: "", username: "owner", password: "a long password here", confirm: "a long password here" }, false);
  assert.equal(problem, null);
  assert.deepEqual(sent[0], { url: "/api/auth/setup", method: "POST", body: { username: "owner", password: "a long password here" } });
  assert.ok(!("code" in (sent[0]!.body as object)), "no code at all, not an empty one");
  // The server revoked the owner session; the new account's token replaces it.
  assert.equal(getToken(), "tok-2");
  assert.equal(currentUser(), "owner");
});
