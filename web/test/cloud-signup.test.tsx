// Sign-up at a public hub (M3a-3): the hub API's sign-up, sign-in and e-mail
// verification wrappers, the Turnstile frame's message filter, and the
// verification link's token.
//
// What these pin: sign-up sends an invite only when there is one, the
// widget's token and the mail's language; an answer from a hub that predates
// verification verifies nothing; every refusal the hub gives comes back as a
// kind (a 429 with how long to wait); `/v1/me` is typed, `email` and `plan`
// included; the verification POST carries no session; a message from the
// Turnstile frame counts only from its origin, its very frame and its action;
// the frame is sandboxed and sends no Referer; and only a token of the hub's
// shape is read from `#t=…`.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { authErrorKind, HubApi, HubApiError, hubErrorKind } from "../src/lib/hub/api.js";
import { verifyTokenFromHash } from "../src/features/cloud/email-verify.js";
import { Turnstile, turnstileAnswer, turnstileSrc } from "../src/features/cloud/turnstile.js";
import { authErrorText } from "../src/features/cloud/ui.js";

void i18next.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

type Sent = { url: string; method: string; headers: Record<string, string>; body: unknown };

/** A hub that answers each request with the next of `answers`, and keeps what it was sent. */
function fakeHub(...answers: { status?: number; body?: unknown; headers?: Record<string, string> }[]) {
  const sent: Sent[] = [];
  const fetch = async (input: string, init?: RequestInit): Promise<Response> => {
    sent.push({
      url: input,
      method: init?.method ?? "GET",
      headers: (init?.headers ?? {}) as Record<string, string>,
      body: init?.body ? JSON.parse(init.body as string) : undefined,
    });
    const a = answers.shift() ?? { status: 500, body: { error: "no_answer" } };
    return new Response(a.body === undefined ? null : JSON.stringify(a.body), {
      status: a.status ?? 200,
      headers: { "content-type": "application/json", ...a.headers },
    });
  };
  return { api: new HubApi("https://hub.test", fetch), sent };
}

const SIGNED = { account: "a-1", token: "login-token", expires_ms: 1_000 };

test("sign-up config: droppen.app's answers, written in the page, with no request", async () => {
  const { api, sent } = fakeHub();
  const answers = { signup: "open", inviteRequired: false, turnstile: null, verification: "required", passkeys: { rpId: "droppen.app" }, deletion: { emailHoldDays: 7 } };
  assert.deepEqual(await api.signupConfig(), answers);
  assert.deepEqual(await api.signupConfig(), answers);
  assert.deepEqual(sent, [], "no request is made");
  // A copy each time: a caller cannot change the page's answers.
  const config = await api.signupConfig();
  config.passkeys!.rpId = "elsewhere.test";
  assert.equal((await api.signupConfig()).passkeys?.rpId, "droppen.app");
});

test("sign-up sends the invite only when there is one, the widget's token and the mail's language", async () => {
  const { api, sent } = fakeHub({ body: { ...SIGNED, email_verified: false, verification: "required" } }, { body: { ...SIGNED, email_verified: true, verification: "off" } });
  const s = await api.signUp("me@example.test", "a long password", { turnstile: "tok", lang: "zh-TW" });
  assert.deepEqual(sent[0]!.body, { email: "me@example.test", password: "a long password", turnstile: "tok", lang: "zh-TW" });
  assert.equal(sent[0]!.url, "https://hub.test/v1/signup");
  assert.equal(sent[0]!.headers.authorization, undefined);
  assert.deepEqual(s, { account: "a-1", token: "login-token", expiresMs: 1_000, emailVerified: false, verification: "required" });
  await api.signUp("me@example.test", "a long password", { invite: "INVITE-1" });
  assert.deepEqual(sent[1]!.body, { email: "me@example.test", password: "a long password", invite: "INVITE-1" });
});

test("a hub from before verification: its sign-in answer verifies nothing", async () => {
  const { api } = fakeHub({ body: SIGNED }, { body: { ...SIGNED, email_verified: false, verification: "off" } });
  assert.deepEqual(await api.login("me@example.test", "pw"), { account: "a-1", token: "login-token", expiresMs: 1_000, emailVerified: true, verification: "off" });
  // Verification off: nothing to wait for, whatever the flag says.
  assert.equal((await api.login("me@example.test", "pw")).emailVerified, true);
});

test("sign-in carries the widget's token only once there is one", async () => {
  const { api, sent } = fakeHub(
    { status: 403, body: { error: "turnstile_required", message: "complete the human check first" } },
    { body: { ...SIGNED, email_verified: true, verification: "required" } },
  );
  const refused = await api.login("me@example.test", "pw").catch((e: unknown) => e);
  assert.ok(refused instanceof HubApiError);
  assert.equal(authErrorKind(refused), "turnstileRequired");
  assert.deepEqual(sent[0]!.body, { email: "me@example.test", password: "pw" });
  const s = await api.login("me@example.test", "pw", "tok");
  assert.deepEqual(sent[1]!.body, { email: "me@example.test", password: "pw", turnstile: "tok" });
  assert.equal(s.emailVerified, true);
});

test("every refusal comes back as a kind; a 429 says how long to wait", async () => {
  const cases: [number, string, ReturnType<typeof authErrorKind>][] = [
    [403, "signup_closed", "signupClosed"],
    [403, "bad_invite", "badInvite"],
    [403, "turnstile_required", "turnstileRequired"],
    [403, "turnstile_retry", "turnstileRetry"],
    [403, "turnstile_failed", "turnstileFailed"],
    [503, "turnstile_unavailable", "turnstileUnavailable"],
    [400, "disposable_email", "disposableEmail"],
    [409, "email_taken", "emailTaken"],
    [401, "bad_login", "badLogin"],
    [401, "unauthorized", "unauthorized"],
    [409, "already_verified", "alreadyVerified"],
    [409, "verification_off", "verificationOff"],
    [400, "bad_token", "badToken"],
    [410, "expired", "expired"],
    [400, "bad_request", null],
  ];
  for (const [status, code, kind] of cases) {
    const { api } = fakeHub({ status, body: { error: code, message: `the hub's words for ${code}` } });
    const error = await api.signUp("me@example.test", "pw").catch((e: unknown) => e);
    assert.equal(authErrorKind(error), kind, code);
  }
  const { api } = fakeHub({ status: 429, body: { error: "rate", message: "too many requests; try again in 3 s", retry_after_ms: 2_400 } });
  const rate = await api.signUp("me@example.test", "pw").catch((e: unknown) => e);
  assert.ok(rate instanceof HubApiError);
  assert.equal(rate.retryAfterMs, 2_400);
  assert.equal(authErrorKind(rate), "rate");
  assert.equal(hubErrorKind(rate), "rate");
  assert.equal(authErrorText(rate), "too many attempts: try again in 3 s");
  // Only the header said: seconds.
  const header = fakeHub({ status: 429, body: { error: "rate", message: "slow down" }, headers: { "retry-after": "7" } });
  const fromHeader = await header.api.login("me@example.test", "pw").catch((e: unknown) => e);
  assert.equal((fromHeader as HubApiError).retryAfterMs, 7_000);
  // An unverified account's refusal anywhere else (adding a device, a machine).
  const unverified = fakeHub({ status: 403, body: { error: "email_unverified", message: "verify first" } });
  assert.equal(hubErrorKind(await unverified.api.registerDevice("t", new Uint8Array(32), new Uint8Array(32)).catch((e: unknown) => e)), "emailUnverified");
  // In words, by kind; never the hub's message (not the reader's language): a refusal of no known kind by its code.
  const words = async (status: number, code: string, context?: "signup" | "login" | "resend") => {
    const { api } = fakeHub({ status, body: { error: code, message: `the hub's words for ${code}` } });
    return authErrorText(await api.login("me@example.test", "pw").catch((e: unknown) => e), context);
  };
  assert.equal(await words(400, "disposable_email"), "use an e-mail address you keep: throwaway mail services are not accepted");
  assert.equal(await words(503, "turnstile_unavailable", "signup"), "the human check cannot be reached right now, so sign-up is paused: try again in a few minutes");
  assert.equal(await words(503, "turnstile_unavailable", "login"), "the human check cannot be reached right now: try again in a few minutes");
  assert.equal(await words(401, "bad_login"), "wrong e-mail or password");
  // What was typed, refused: by the field the hub's message names (`docs/hub-api`), never the message itself — nor the e-mail blamed for another field.
  assert.equal(await words(400, "bad_request"), "Droppen did not accept what was typed: check it and try again");
  const typed = (message: string, context: "signup" | "login" = "signup") => authErrorText(new HubApiError(400, "bad_request", message), context);
  assert.equal(typed("a password is 10–1024 bytes"), "Droppen did not accept that password: use 10 to 1024 characters");
  assert.equal(typed("password: at most 1024 bytes", "login"), "Droppen did not accept that password: use 10 to 1024 characters");
  assert.equal(typed("that is not an e-mail address"), "Droppen did not accept that e-mail address: check it and try again");
  assert.equal(typed("email: at most 254 bytes", "login"), "Droppen did not accept that e-mail address: check it and try again");
  assert.equal(typed("invite: too long"), "Droppen did not accept what was typed: check it and try again");
  assert.equal(await words(400, "bad_request", "resend"), "Droppen could not do that (bad_request)");
  assert.equal(await words(409, "something_new"), "Droppen could not do that (something_new)");
});

test("resend asks with the session and the widget's token; a login token is not renewed on a 401", async () => {
  const { api, sent } = fakeHub({ body: { queued: 12 } }, { body: { queued: 13 } }, { status: 401, body: { error: "unauthorized", message: "sign in first" } });
  let renewed = 0;
  api.renew = async () => {
    renewed++;
    return "fresh";
  };
  assert.deepEqual(await api.resendVerification("login-token", "tok", { noRenew: true }), { queued: 12 });
  assert.equal(sent[0]!.url, "https://hub.test/v1/email/resend");
  assert.equal(sent[0]!.method, "POST");
  assert.equal(sent[0]!.headers.authorization, "Bearer login-token");
  assert.deepEqual(sent[0]!.body, { turnstile: "tok" });
  await api.resendVerification("device-token");
  assert.deepEqual(sent[1]!.body, {});
  const gone = await api.resendVerification("login-token", null, { noRenew: true }).catch((e: unknown) => e);
  assert.equal(authErrorKind(gone), "unauthorized");
  assert.equal(renewed, 0);
  assert.equal(sent.length, 3);
});

test("verifying the link's token needs no session; expired and invalid links are told apart", async () => {
  const { api, sent } = fakeHub(
    { body: { account: "a-1", verified: true } },
    { status: 410, body: { error: "expired", message: "this link expired: sign in and ask for a new one" } },
    { status: 400, body: { error: "bad_token", message: "this link is not valid" } },
  );
  assert.deepEqual(await api.verifyEmail("v1.a-1.1.tag"), { account: "a-1", verified: true });
  assert.equal(sent[0]!.url, "https://hub.test/v1/email/verify");
  assert.deepEqual(sent[0]!.body, { token: "v1.a-1.1.tag" });
  assert.equal(sent[0]!.headers.authorization, undefined);
  assert.equal(authErrorKind(await api.verifyEmail("v1.a-1.1.tag").catch((e: unknown) => e)), "expired");
  assert.equal(authErrorKind(await api.verifyEmail("v1.a-1.1.tag").catch((e: unknown) => e)), "badToken");
});

test("/v1/me is typed: the e-mail's standing and the plan with its entitlements and usage", async () => {
  const me = {
    account: "a-1",
    email: { address: "me@example.test", verified: false, verification: "required", due_ms: 5_000, delivery: "failed", delivery_error: "mailbox unavailable" },
    plan: {
      id: "free",
      source: "default",
      status: "active",
      period_start_ms: null,
      period_end_ms: null,
      grace_plan: null,
      grace_until_ms: null,
      entitlements: { "storage.retention_days": 7, "storage.bytes": 5_000_000_000, "devices.max": 5, "machines.max": 3, "relay.max": null },
      usage: { storage_bytes: 12, devices: 1, machines: 0, relays: 0, transfer: { period_start_ms: 1, period_end_ms: 2, bytes: 3, included: null, overage_bytes: 0 } },
    },
    chain_seq: null,
    machines: [],
  };
  const { api, sent } = fakeHub({ body: me }, { body: { account: "a-1", email: "old@example.test", chain_seq: 4, machines: [] } });
  const view = await api.me("login-token", { noRenew: true });
  assert.equal(sent[0]!.headers.authorization, "Bearer login-token");
  assert.deepEqual(view.email, { address: "me@example.test", verified: false, verification: "required", dueMs: 5_000, delivery: "failed", deliveryError: "mailbox unavailable" });
  assert.equal(view.plan?.id, "free");
  assert.deepEqual(view.plan?.entitlements, { "storage.retention_days": 7, "storage.bytes": 5_000_000_000, "devices.max": 5, "machines.max": 3, "relay.max": null });
  // `hosts` is the new name of `machines` (a hub that says only the old one: read as it).
  assert.deepEqual(view.plan?.usage, { storageBytes: 12, devices: 1, hosts: 0, machines: 0, relays: 0, transfer: { periodStartMs: 1, periodEndMs: 2, bytes: 3, included: null, overageBytes: 0, throttled: false } });
  assert.equal(view.chainV3, null, "a hub from before chain v3: no v3 entry is written");
  assert.equal(view.chainSeq, null);
  // A hub from before verification and plans: a bare address, nothing to verify, no plan.
  const old = await api.me("t");
  assert.deepEqual(old.email, { address: "old@example.test", verified: false, verification: "off", dueMs: null, delivery: null, deliveryError: null });
  assert.equal(old.plan, null);
  assert.equal(old.chainSeq, 4);
});

test("the Turnstile frame's message counts only from its origin, its very frame and its action", () => {
  const frame = { name: "the widget's window" };
  const other = { name: "another window" };
  const expected = { origin: "https://verify.hub.test", source: frame, action: "signup" as const };
  const ok = { origin: "https://verify.hub.test", source: frame, data: { t: "turnstile", action: "signup", token: "tok" } };
  assert.deepEqual(turnstileAnswer(ok, expected), { token: "tok", expired: false, error: false });
  // Another origin (the app's own, a look-alike), another window, another action, another shape: ignored.
  assert.equal(turnstileAnswer({ ...ok, origin: "https://hub.test" }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, origin: "https://verify.hub.test.evil.example" }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, origin: "http://verify.hub.test" }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, source: other }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, source: null }, { ...expected, source: null }), null, "no frame yet: nothing counts");
  assert.equal(turnstileAnswer({ ...ok, data: { ...ok.data, action: "login" } }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, data: { ...ok.data, t: "turnstile-reset" } }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, data: "tok" }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, data: null }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, data: { ...ok.data, token: 42 } }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, data: { ...ok.data, token: "" } }, expected), null);
  assert.equal(turnstileAnswer({ ...ok, data: { ...ok.data, token: "x".repeat(2049) } }, expected), null);
  // Expiry and errors take the token back.
  assert.deepEqual(turnstileAnswer({ ...ok, data: { t: "turnstile", action: "signup", token: null, expired: true } }, expected), { token: null, expired: true, error: false });
  assert.deepEqual(turnstileAnswer({ ...ok, data: { t: "turnstile", action: "signup", token: null, error: true } }, expected), { token: null, expired: false, error: true });
});

test("the Turnstile frame: the widget's own origin, sandboxed, no Referer", () => {
  assert.equal(turnstileSrc("https://verify.hub.test", "resend", true), "https://verify.hub.test/turnstile?action=resend&theme=dark");
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18next}>
      <Turnstile config={{ siteKey: "k", origin: "https://verify.hub.test" }} action="login" onToken={() => undefined} />
    </I18nextProvider>,
  );
  assert.match(html, /<iframe [^>]*src="https:\/\/verify\.hub\.test\/turnstile\?action=login&amp;theme=light"/);
  assert.match(html, /sandbox="allow-scripts allow-same-origin allow-popups"/);
  assert.match(html, /referrerPolicy="no-referrer"|referrerpolicy="no-referrer"/);
  assert.match(html, /title="human check"/);
  // Never handed the app's passkeys (docs/passkeys.md §3.8 rule 2): no
  // `allow` at all, so no `publickey-credentials-*` (a cross-origin frame
  // cannot use WebAuthn without one).
  assert.doesNotMatch(html, /\ballow=/i);
  assert.doesNotMatch(html, /publickey-credentials/i);
});

test("the verification link's token is read from #t=…, and only in the hub's shape", () => {
  const token = "v1.a-k3j4h5g6f7d8s9a0.1760700000000.AbC-_dEf0123456789abcdefABCDEF01";
  assert.equal(verifyTokenFromHash(`#t=${token}`), token);
  assert.equal(verifyTokenFromHash(`t=${token}`), token);
  assert.equal(verifyTokenFromHash(`#utm=x&t=${token}`), token);
  assert.equal(verifyTokenFromHash(`#t=${encodeURIComponent(token)}`), token);
  // Missing, empty or garbage: no token (the page says the link is not valid, and asks the hub nothing).
  for (const hash of ["", "#", "#t=", "#x=1", "#t=hello", "#t=v2.a-1.1.tag", "#t=v1.a-1.1", "#t=v1.a-1.x.tag", "#t=v1.a.1.1.tag", "#t=v1.a 1.1.tag", "#t=%E0%A4%A", `#t=v1.a-1.1.${"x".repeat(200)}`, "#t=v1.a-1.1.tag<script>"]) {
    assert.equal(verifyTokenFromHash(hash), null, hash);
  }
});
