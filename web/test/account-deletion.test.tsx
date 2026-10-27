// Deleting the account (`DELETE /v1/account`, `features/cloud/account-settings.tsx`
// and `sign-out.ts`), and a browser finding its account gone or suspended
// (`lib/hub/account.ts`, `lib/hub/follow.ts`, `cloud-app.tsx`).
//
// What these pin: the hub client sends the typed e-mail and the proof (the
// current password or a passkey's answer) to `DELETE /v1/account`, and reads
// the new refusals by kind (`confirm_mismatch`, `account_suspended`,
// `recently_deleted` with its `until_ms`, `410 account_deleted`) and the
// sign-up answers' `deletion` (written in the page); the typed address confirms only as the hub
// compares it (trimmed, any case); the page says what deleting means — with
// the hub's days, or none — and offers it on a full device only, folded away,
// its button held until the address is typed; a deletion the hub refuses
// leaves this browser as it was, one it took leaves it; a hub that says the
// account is gone (on the socket, or to a session renewal) has its keys
// wiped, while a suspended account's are kept; and the words for all of it
// read in the reader's language.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { authErrorKind, HubApi, HubApiError, hubErrorKind, type SignInMethods } from "../src/lib/hub/api.js";
import { accountDeletedMessage, ChainFollower, type FollowLink } from "../src/lib/hub/follow.js";
import { AccountSettings, DeleteAccountForm, deletionPoints, emailConfirms } from "../src/features/cloud/account-settings.js";
import { deleteAccount, deletingAccount, markAccountDeleted, takeAccountDeleted, wipeAccountHere } from "../src/features/cloud/sign-out.js";
import { authErrorText, errorText } from "../src/features/cloud/ui.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import i18next from "./i18n-all.js";
import { createV2Genesis, FakeKeysHub } from "./fake-keys-hub.js";
import { MemoryStorage } from "./storage.js";

Object.assign(globalThis, { localStorage: new MemoryStorage(), sessionStorage: new MemoryStorage(), location: { pathname: "/account", search: "" } });
// A browser with WebAuthn (the page offers passkeys only there).
Object.assign(globalThis, { PublicKeyCredential: function PublicKeyCredential() {} });
Object.defineProperty(globalThis, "navigator", { value: { credentials: { create: async () => null, get: async () => null } }, configurable: true });

// The app's own tables (`i18n/index.ts`, every language loaded: `i18n-all.ts`): the page's words and `errorText`'s are read from them.
function draw(node: React.ReactElement, lng = "en"): string {
  void i18next.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18next}>{node}</I18nextProvider>);
}

type Sent = { url: string; method: string; body: unknown };

function fakeHub(...answers: { status?: number; body?: unknown }[]) {
  const sent: Sent[] = [];
  const fetch = async (input: string, init?: RequestInit): Promise<Response> => {
    sent.push({ url: input, method: init?.method ?? "GET", body: init?.body ? JSON.parse(init.body as string) : undefined });
    const a = answers.shift() ?? { status: 500, body: { error: "no_answer" } };
    return new Response(a.body === undefined ? null : JSON.stringify(a.body), { status: a.status ?? 200, headers: { "content-type": "application/json" } });
  };
  return { api: new HubApi("https://hub.test", fetch, new MemoryStorage()), sent };
}

const ANSWER = { challenge: "AQ", credential: { id: "c" } as never };

// ------------------------------------------------------------ the hub's calls

test("DELETE /v1/account: the typed e-mail and the proof, the password's or a passkey's; purged or still being erased", async () => {
  const { api, sent } = fakeHub({ body: { deleted: true, purged: true } }, { body: { deleted: true, purged: false } });
  assert.deepEqual(await api.deleteAccount("dev", "Me@Example.test", { currentPassword: "the password" }), { purged: true });
  assert.deepEqual(await api.deleteAccount("dev", "me@example.test", { passkey: ANSWER }), { purged: false }, "still being erased: the account is gone all the same");
  assert.deepEqual(
    sent.map((s) => `${s.method} ${s.url.replace("https://hub.test", "")}`),
    ["DELETE /v1/account", "DELETE /v1/account"],
  );
  assert.deepEqual(sent[0]!.body, { confirm: "Me@Example.test", current_password: "the password" });
  assert.deepEqual(sent[1]!.body, { confirm: "me@example.test", passkey: { challenge: "AQ", credential: { id: "c" } } });
});

test("the new refusals, by kind: confirm_mismatch, account_suspended, recently_deleted with until_ms, 410 account_deleted", async () => {
  const refusal = async (status: number, body: Record<string, unknown>) => {
    const { api } = fakeHub({ status, body: { message: "the hub's words", ...body } });
    return api.deleteAccount("dev", "x", { currentPassword: "pw" }).catch((e: unknown) => e);
  };
  assert.equal(hubErrorKind(await refusal(400, { error: "confirm_mismatch" })), "confirmMismatch");
  const suspended = await refusal(403, { error: "account_suspended" });
  assert.equal(hubErrorKind(suspended), "accountSuspended");
  assert.equal(authErrorKind(suspended), "accountSuspended");
  const recent = await refusal(409, { error: "recently_deleted", until_ms: Date.UTC(2026, 9, 30, 12) });
  assert.ok(recent instanceof HubApiError);
  assert.equal(recent.untilMs, Date.UTC(2026, 9, 30, 12));
  assert.equal(authErrorKind(recent), "recentlyDeleted");
  assert.equal((await refusal(409, { error: "email_taken", until_ms: 5 }) as HubApiError).untilMs, undefined, "only a recently_deleted carries it");
  assert.equal(hubErrorKind(await refusal(410, { error: "account_deleted" })), "accountDeleted");
  // Not every 410, nor any 401, is the account gone.
  assert.equal(hubErrorKind(await refusal(410, { error: "expired" })), null);
  assert.notEqual(hubErrorKind(await refusal(401, { error: "unauthorized" })), "accountDeleted");
});

test("sign-up config's deletion: droppen.app holds a deleted address for seven days, written in the page", async () => {
  let asked = 0;
  const api = new HubApi("https://hub.test", (async () => {
    asked++;
    return new Response(null, { status: 204 });
  }) as unknown as typeof fetch);
  assert.deepEqual((await api.signupConfig()).deletion, { emailHoldDays: 7 });
  assert.equal(asked, 0, "no request");
});

test("the known-client token kept for an address goes with its account", async () => {
  const storage = new MemoryStorage();
  const fetch = async () => new Response(JSON.stringify({ account: "a-1", token: "t", expires_ms: 1, known_client: "kc1.x" }), { status: 200, headers: { "content-type": "application/json" } });
  const api = new HubApi("https://hub.test", fetch, storage);
  await api.login("Me@Example.test", "pw");
  assert.equal(storage.values.size, 1);
  await api.forgetKnownClient("me@example.test ");
  assert.equal(storage.values.size, 0);
});

// ------------------------------------------------------------ the page

test("the typed address confirms only as the hub compares it: trimmed, in any case", () => {
  assert.equal(emailConfirms("me@example.test", "me@example.test"), true);
  assert.equal(emailConfirms("  ME@Example.TEST ", "me@example.test"), true);
  assert.equal(emailConfirms("me@example.tes", "me@example.test"), false);
  assert.equal(emailConfirms("", "me@example.test"), false);
  assert.equal(emailConfirms("   ", "   "), false, "nothing typed confirms nothing");
  assert.equal(emailConfirms("me@example.test", null), false, "no address read yet: nothing to confirm against");
});

test("what deleting means: gone for good, every device stops, this browser wiped — and the hub's days, if it said", () => {
  const t = (key: string, vars?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_m, k: string) => String(vars?.[k] ?? ""));
  const said = deletionPoints(7, t);
  assert.equal(said.length, 4);
  assert.match(said[0]!, /Nobody can bring any of it back — not you, not us\./);
  assert.match(said[1]!, /Every device of the account stops working at once\. To use a device that runs AI again, add it to another account\./);
  assert.match(said[2]!, /This browser forgets the account's keys now\. Your other browsers and phones are not wiped from here/);
  assert.equal(said[3], "For 7 days, this e-mail address cannot be used to sign up again.");
  assert.equal(deletionPoints(1, t)[3], "For a day, this e-mail address cannot be used to sign up again.");
  assert.equal(deletionPoints(null, t)[3], "For a few days, this e-mail address cannot be used to sign up again.", "no number when the hub did not say");
  assert.equal(deletionPoints(0, t).length, 3, "none kept: nothing to say");
});

const methods = (over: Partial<SignInMethods> = {}): SignInMethods => ({ password: true, max: 10, passkeys: [], ...over });
const PASSKEY = {
  id: "pk-1",
  label: "My laptop",
  provider: "iCloud Keychain",
  credentialId: "AQID",
  credHash: "ab".repeat(32),
  backupEligible: true,
  backupState: true,
  prf: "yes" as const,
  createdMs: Date.UTC(2026, 9, 17),
  createdBy: "d-1",
  lastUsedMs: null,
  unlock: "off" as const,
};

function account(admin = true) {
  return {
    account: "a-1",
    standing: "active",
    isAdmin: admin,
    chain: { isV3: true, currentRecoveryKey: new Uint8Array(32) },
    session: { token: "dev" },
    passkeySlotFor: () => null,
  } as never;
}

function page(m: SignInMethods | null, opts: { admin?: boolean; email?: string | null; lng?: string } = {}) {
  return draw(
    <AccountSettings
      account={account(opts.admin ?? true)}
      email={opts.email === undefined ? "me@example.test" : opts.email}
      verified={true}
      methods={m}
      hashes={new Map()}
      loadError={null}
      rpId="hub.test"
      holdDays={7}
      token="dev"
      onChanged={() => undefined}
      onWords={() => undefined}
    />,
    opts.lng,
  );
}

test("the page ends with deleting the account: folded away, a danger button, on a full device only", () => {
  const html = page(methods());
  const at = html.indexOf('data-testid="delete-account-section"');
  assert.ok(at > html.indexOf('data-testid="sign-out-section"'), "at the bottom, after signing out");
  assert.ok(html.includes(">deleting the account<"));
  assert.match(html, /data-testid="delete-account" class="[^"]*text-danger[^"]*"/);
  assert.doesNotMatch(html, /data-testid="delete-account"[^>]*disabled=""/);
  assert.ok(!html.includes('data-testid="delete-account-form"'), "folded away until asked for");
  assert.doesNotMatch(html, /data-variant="primary"[^>]*data-testid="delete-account"/, "never the page's primary action");
  // A device that is not full: told why, nothing offered.
  const limited = page(methods(), { admin: false });
  assert.ok(limited.includes('data-testid="delete-read-only"'));
  assert.ok(limited.includes("Only a full device of the account can delete it. Do it on one of those."));
  assert.ok(!limited.includes('data-testid="delete-account"'));
  // Held while the ways to sign in, or the address, are still being read.
  assert.match(page(null), /data-testid="delete-account"[^>]*disabled=""/);
  assert.match(page(methods(), { email: null }), /data-testid="delete-account"[^>]*disabled=""/);
});

test("passkeys only, on a browser that cannot use them: held, and said why", () => {
  const before = (globalThis as { PublicKeyCredential?: unknown }).PublicKeyCredential;
  (globalThis as { PublicKeyCredential?: unknown }).PublicKeyCredential = undefined;
  try {
    const html = page(methods({ password: false, passkeys: [PASSKEY] }));
    assert.match(html, /data-testid="delete-account"[^>]*disabled=""[^>]*title="deleting the account asks for one of your passkeys, which this browser cannot use"/);
    // With a password it can always be proved.
    assert.doesNotMatch(page(methods({ password: true, passkeys: [PASSKEY] })), /data-testid="delete-account"[^>]*disabled=""/);
  } finally {
    (globalThis as { PublicKeyCredential?: unknown }).PublicKeyCredential = before;
  }
});

test("the form: what it means, the address to type, the proof, and its button held until the address is typed", () => {
  const form = (m: SignInMethods, holdDays: number | null, lng = "en") =>
    draw(<DeleteAccountForm account={account()} token="dev" email="me@example.test" methods={m} webauthn={true} holdDays={holdDays} onCancel={() => undefined} />, lng);
  const html = form(methods(), 7);
  assert.equal(html.match(/<li>/g)?.length, 4);
  assert.ok(html.includes("For 7 days, this e-mail address cannot be used to sign up again."));
  assert.ok(html.includes("To confirm, type the account&#x27;s e-mail address:"));
  assert.match(html, /<span class="font-mono text-fg">me@example.test<\/span>/, "the address shown, to type");
  assert.match(html, /data-testid="delete-confirm"[^>]*value=""/);
  assert.match(html, /data-testid="delete-account-confirm"[^>]*disabled=""/, "held until the address is typed");
  assert.ok(html.includes('autoComplete="current-password"') || html.includes('autocomplete="current-password"'), "the current password, where there is one");
  // A passkey too: it proves by default, the password a step away.
  const both = form(methods({ passkeys: [PASSKEY] }), null);
  assert.ok(both.includes("First, one of your passkeys checks it is you."));
  assert.ok(both.includes(">use the current password instead<"));
  assert.ok(both.includes("For a few days, this e-mail address cannot be used to sign up again."));
  // In Chinese.
  const cn = form(methods(), 7, "zh-CN");
  for (const s of ["谁都无法恢复", "之后 7 天内，这个邮箱不能再用来注册。", "请输入账户的邮箱地址以确认：", "永久删除账户"]) assert.ok(cn.includes(s), s);
});

test("a deletion the hub refuses leaves this browser as it was; one it took leaves the account", async () => {
  const sent: unknown[] = [];
  const left: string[] = [];
  const answers: (Error | null)[] = [new HubApiError(403, "reauth_failed", "no"), null];
  const acct = {
    session: { token: "dev" },
    api: {
      deleteAccount: async (token: string, confirm: string, proof: unknown) => {
        sent.push({ token, confirm, proof });
        assert.equal(deletingAccount(), true, "while it is asked, the hub ending the sessions is this deletion");
        const a = answers.shift();
        if (a) throw a;
        return { purged: true };
      },
    },
  } as never;
  const leave = async (_a: unknown, email: string | null) => void left.push(String(email));
  await assert.rejects(deleteAccount(acct, { confirm: " me@example.test ", proof: { currentPassword: "nope" }, email: "me@example.test" }, leave), (e: unknown) => hubErrorKind(e) === "reauthFailed");
  assert.deepEqual(left, [], "refused: nothing wiped, nothing left");
  assert.equal(deletingAccount(), false, "and a refusal from the hub is a refusal again");
  await deleteAccount(acct, { confirm: "me@example.test", proof: { currentPassword: "right" }, email: "me@example.test" }, leave);
  assert.deepEqual(left, ["me@example.test"]);
  assert.deepEqual(sent[0], { token: "dev", confirm: "me@example.test", proof: { currentPassword: "nope" } }, "the address as typed, trimmed");
  // The page's own deletion: the refusal screen waits it out, and the sign-in screen says so once.
  const app = readFileSync(new URL("../src/features/cloud/cloud-app.tsx", import.meta.url), "utf8");
  assert.match(app, /if \(deletingAccount\(\)\) return;/);
  const storage = new MemoryStorage();
  markAccountDeleted(storage as never);
  assert.equal(takeAccountDeleted(storage as never), true);
  assert.equal(takeAccountDeleted(storage as never), false, "said once");
});

test("an answer lost after the hub said the account is gone still leaves it: the deletion was taken", async () => {
  const left: string[] = [];
  const acct = {
    session: { token: "dev" },
    refusal: null as string | null,
    api: {
      deleteAccount: async () => {
        // The socket's `error account_deleted` arrives, then the answer is lost.
        acct.refusal = "deleted";
        throw new TypeError("Failed to fetch");
      },
    },
  };
  const leave = async (_a: unknown, email: string | null) => void left.push(String(email));
  await deleteAccount(acct as never, { confirm: "me@example.test", proof: { currentPassword: "right" }, email: "me@example.test" }, leave);
  assert.deepEqual(left, ["me@example.test"], "wiped and left all the same");
  // Without that word from the hub, a lost answer is an error and nothing is wiped.
  const quiet = { ...acct, refusal: null, api: { deleteAccount: async () => Promise.reject(new TypeError("Failed to fetch")) } };
  left.length = 0;
  await assert.rejects(deleteAccount(quiet as never, { confirm: "me@example.test", proof: { currentPassword: "right" }, email: "me@example.test" }, leave));
  assert.deepEqual(left, []);
  assert.equal(deletingAccount(), false);
});

test("leaving a deleted account wipes as a removed device does, and says what could not be cleared", async () => {
  const failures = await wipeAccountHere({ wipe: async () => Promise.reject(new Error("store locked")) }, undefined);
  assert.ok(failures.includes("keys: store locked"), failures.join(" | "));
  const src = readFileSync(new URL("../src/features/cloud/sign-out.ts", import.meta.url), "utf8");
  const leave = src.slice(src.indexOf("export async function leaveDeletedAccount"));
  for (const step of ["endHubShell();", "account.stopSession();", "forgetPasskeySignIn();", "clearSession();", "await account.api.forgetKnownClient(email)", "wipeAccountHere(account, account.store)", "markAccountDeleted();", "location.replace(SIGN_IN_PATH);"]) {
    assert.ok(leave.includes(step), step);
  }
  const shellSrc = readFileSync(new URL("../src/features/cloud/hub-shell.ts", import.meta.url), "utf8");
  assert.ok(shellSrc.slice(shellSrc.indexOf("export function endHubShell")).includes("hubShell()?.stop();"), "the shared teardown stops the shell");
  const app = readFileSync(new URL("../src/features/cloud/cloud-app.tsx", import.meta.url), "utf8");
  assert.match(app, /const failures = await wipeAccountHere\(account, store\);/, "the removed device's wipe is the same one");
});

// ------------------------------------------------------------ an account gone, or suspended

/** A hub link the test drives (only what the follower listens to). */
class FakeLink {
  state: "open" | "closed" | "connecting" = "closed";
  lastClose: number | null = null;
  private controlFns = new Set<(m: Record<string, unknown>) => void>();
  onState() {
    return () => undefined;
  }
  onChain() {
    return () => undefined;
  }
  onControl(fn: (m: Record<string, unknown>) => void) {
    this.controlFns.add(fn);
    return () => this.controlFns.delete(fn);
  }
  sendControl() {
    return true;
  }
  attest() {
    return true;
  }
  control(m: Record<string, unknown>) {
    for (const fn of this.controlFns) fn(m);
  }
}

async function until(done: () => boolean, ms = 5000): Promise<void> {
  const end = Date.now() + ms;
  while (!done()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

test("the device socket's `error account_deleted` is the account gone; `account_suspended` is not", async (t) => {
  assert.equal(accountDeletedMessage({ t: "error", code: "account_deleted", message: "this account was deleted" }), true);
  assert.equal(accountDeletedMessage({ t: "error", code: "account_suspended" }), false);
  assert.equal(accountDeletedMessage({ t: "error", code: "removed" }), false);
  assert.equal(accountDeletedMessage({ t: "mail", code: "account_deleted" }), false);
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, false);
  await a.sync();
  const link = new FakeLink();
  const refused: string[] = [];
  const follower = new ChainFollower({ link: link as unknown as FollowLink, account: a, onRemoved: () => refused.push("removed"), onRefused: () => refused.push(String(a.refusal)) });
  t.after(() => follower.stop());
  follower.start();
  link.control({ t: "error", code: "account_suspended", message: "this account is suspended" });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(a.refusal, null, "suspended: nothing decided from it");
  link.control({ t: "error", code: "account_deleted", message: "this account was deleted" });
  await until(() => refused.length > 0);
  assert.equal(a.refusal, "deleted");
  assert.deepEqual(refused, ["deleted"], "handed to the page, which wipes for it");
});

test("a session renewal answered 410 account_deleted is the account gone; 403 account_suspended keeps the keys", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, false);
  await a.sync();
  hub.suspended = true;
  const held = await hub.reload(a);
  assert.equal(held.refusal, "denied", "suspended: refused like any session, never `deleted`");
  hub.suspended = false;
  hub.accountGone = true;
  const gone = await hub.reload(a);
  assert.equal(gone.refusal, "deleted");
  // The page wipes for `deleted` (and a removal the chain shows), and keeps the keys for any other refusal.
  const app = readFileSync(new URL("../src/features/cloud/cloud-app.tsx", import.meta.url), "utf8");
  const refusedFn = app.slice(app.indexOf("const refused = useCallback("), app.indexOf("/** The chain as served conflicts"));
  assert.match(refusedFn, /if \(refusal === "deleted"\) \{\s*const failures = await wipeRemoved\(account, link, store\);\s*return setPhase\(\{ kind: "deleted", failures \}\);/);
  assert.match(app, /case "deleted":\s*return <AccountDeletedScreen failures=\{phase\.failures\} \/>;/);
});

test("the words: the new refusals in the reader's language, and the deleted account's screens", async () => {
  await i18next.changeLanguage("en");
  assert.equal(errorText(new HubApiError(400, "confirm_mismatch", "x")), "that is not this account's e-mail address: type it exactly as shown");
  assert.equal(errorText(new HubApiError(403, "account_suspended", "x")), "this account is suspended: it cannot be used for now");
  assert.equal(authErrorText(new HubApiError(403, "account_suspended", "x"), "login"), "this account is suspended, so it cannot sign in for now");
  assert.equal(authErrorText(new HubApiError(409, "recently_deleted", "x")), "an account with this e-mail address was deleted a short while ago: it can sign up again in a few days");
  const until = authErrorText(new HubApiError(409, "recently_deleted", "x", { untilMs: Date.UTC(2026, 9, 30, 12) }));
  assert.match(until, /^an account with this e-mail address was deleted a short while ago: it can sign up again from Oct \d+/);
  assert.equal(errorText(new HubApiError(410, "account_deleted", "x")), "this account was deleted");
  await i18next.changeLanguage("zh-CN");
  try {
    assert.equal(authErrorText(new HubApiError(403, "account_suspended", "x"), "login"), "这个账户已被停用，暂时无法登录");
    assert.equal(errorText(new HubApiError(400, "confirm_mismatch", "x")), "这不是这个账户的邮箱地址：请照所示原样输入");
  } finally {
    await i18next.changeLanguage("en");
  }
  // Every string of the screens that say an account was deleted, in every language.
  const app = readFileSync(new URL("../src/features/cloud/cloud-app.tsx", import.meta.url), "utf8");
  const keys = [
    "this account was deleted",
    "It no longer exists at Droppen, so this browser forgot its keys and its offline copies. To use Droppen again, sign in to another account or make a new one.",
    "Your account was deleted.",
    "What it kept at Droppen is erased for good, and this browser no longer holds its keys.",
  ];
  for (const k of keys) assert.ok(app.includes(`t("${k}")`), k);
  for (const [name, table] of [
    ["zh-CN", { ...cloudDevicesZhCN, ...zhCN }],
    ["zh-TW", { ...cloudDevicesZhTW, ...zhTW }],
    ["ja", { ...cloudDevicesJa, ...ja }],
  ] as const) {
    assert.deepEqual(keys.filter((k) => !(k in table)), [], `${name} lacks these`);
  }
});
