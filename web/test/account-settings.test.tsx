// The account's settings (`features/cloud/account-settings.tsx`, `/account`)
// and joining with a passkey as a person meets it (`passkey-join.tsx`), with
// the hub calls behind them (`HubApi`, `lib/hub/passkey.ts`).
//
// What these pin: the hub's K2 calls send what the contract says (a proof
// with every way in added or taken away — a passkey or the password: the
// current password or a passkey's answer);
// adding a passkey can ask its PRF at creation, and the PRF output never goes
// to the hub; the page shows the e-mail, the password and every passkey with
// what it is, offers changes on a full device only, keeps the account's last
// way to sign in, says when new devices can join with a passkey, and has the
// recovery words and sign-out; `/account` is a page of the shell like
// `/devices`, reached from the bar and from 「我的设备」; a passkey's removal
// takes the proof at the hub before any slot goes; which slot names which
// passkey, and the passkey just made, are worked out from the credential ids
// this page holds; the pending device shows the ordinary pairing offer and
// every full device opens the ordinary pairing to approve it, or refuses it
// after the warning about the passkey; changes to the ways to sign in are
// said on the other full devices; and every string is translated.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { type AccountPasskey, HubApi, HubApiError, type Reauth, type SignInChange, type SignInMethods } from "../src/lib/hub/api.js";
import { MeStore } from "../src/lib/hub/me.js";
import { type CreationOptionsJSON, type CredentialsLike, fromBase64Url, passkeyAdd, passkeyPrf, prfOutputOf, toBase64Url } from "../src/lib/hub/passkey.js";
import { forgetPasskeySignIn, passkeySignIn, rememberPasskeySignIn } from "../src/lib/hub/passkey-join.js";
import { AccountSettings, removePasskey } from "../src/features/cloud/account-settings.js";
import { ACCOUNT_PATH, askRemoval, isAccountPath, isShellPagePath, takeAskedRemoval } from "../src/features/cloud/devices-route.js";
import { AccountButton } from "../src/features/cloud/my-devices.js";
import { PasskeyJoin, PendingPasskeyBanner } from "../src/features/cloud/passkey-join.js";
import { PasskeyPendingScreen } from "../src/features/cloud/pairing-screen.js";
import { SignInChangeList, signInChangeText } from "../src/features/cloud/plan.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "../src/i18n/cross-device.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { signOutJa, signOutZhCN, signOutZhTW } from "../src/i18n/sign-out.js";
import { zhTW } from "../src/i18n/zh-TW.js";

class MemoryStorage {
  values = new Map<string, string>();
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
}
Object.assign(globalThis, { localStorage: new MemoryStorage(), sessionStorage: new MemoryStorage(), location: { pathname: ACCOUNT_PATH, search: "" } });
// A browser with WebAuthn (the page offers passkeys only there).
Object.assign(globalThis, { PublicKeyCredential: function PublicKeyCredential() {} });
Object.defineProperty(globalThis, "navigator", { value: { credentials: { create: async () => null, get: async () => null } }, configurable: true });

const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...cloudDevicesZhCN, ...zhCN } } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

function draw(node: React.ReactElement, lng = "en"): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

// ------------------------------------------------------------ the hub's calls

type Sent = { url: string; method: string; body: unknown };

function fakeHub(...answers: { status?: number; body?: unknown }[]) {
  const sent: Sent[] = [];
  const fetch = async (input: string, init?: RequestInit): Promise<Response> => {
    sent.push({ url: input, method: init?.method ?? "GET", body: init?.body ? JSON.parse(init.body as string) : undefined });
    const a = answers.shift() ?? { status: 500, body: { error: "no_answer" } };
    return new Response(a.body === undefined ? null : JSON.stringify(a.body), { status: a.status ?? 200, headers: { "content-type": "application/json" } });
  };
  return { api: new HubApi("https://hub.test", fetch), sent };
}

const ROW = {
  id: "pk-1",
  label: "My laptop",
  provider: "iCloud Keychain",
  credential_id: toBase64Url(new Uint8Array([1, 2, 3])),
  cred_hash: "ab".repeat(32),
  backup_eligible: true,
  backup_state: true,
  prf: "yes",
  transports: ["internal"],
  created_ms: 1_000,
  created_by: "d-1",
  last_used_ms: null,
  unlock: "on",
};

test("the hub's sign-in methods calls: the list, adding, renaming, removing — and a proof with every way in added or taken away", async () => {
  const { api, sent } = fakeHub(
    { body: { passkeys: [ROW, { id: 7 }], password: true, max: 10 } },
    { body: { challenge: "AQ", options: { rp: { id: "hub.test", name: "Droppen" }, user: { id: "AQ", name: "x", displayName: "x" }, challenge: "AQ", pubKeyCredParams: [] } } },
    { body: ROW },
    { body: { ...ROW, label: "New" } },
    { body: { id: "pk-1", removed: true } },
    { body: { challenge: "AQ", options: { challenge: "AQ", rpId: "hub.test", allowCredentials: [] } } },
    { body: { password: true } },
    { body: { password: true } },
    { body: { password: false } },
  );
  const methods = await api.signInMethods("dev");
  assert.equal(methods.password, true);
  assert.equal(methods.max, 10);
  assert.equal(methods.passkeys.length, 1, "a row that names no passkey is dropped");
  assert.deepEqual(methods.passkeys[0], {
    id: "pk-1",
    label: "My laptop",
    provider: "iCloud Keychain",
    credentialId: ROW.credential_id,
    credHash: ROW.cred_hash,
    backupEligible: true,
    backupState: true,
    prf: "yes",
    createdMs: 1_000,
    createdBy: "d-1",
    lastUsedMs: null,
    unlock: "on",
  });
  await api.passkeyAddOptions("dev", { currentPassword: "the old one" });
  await api.addPasskey("dev", { challenge: "AQ", label: "Phone", credential: { id: "x" } as never });
  await api.renamePasskey("dev", "pk-1", "New");
  await api.deletePasskey("dev", "pk-1", { passkey: { challenge: "AQ", credential: { id: "c" } as never } });
  await api.passkeyReauthOptions("dev");
  await api.setPassword("dev", "a long new password", { currentPassword: "the old one" });
  await api.setPassword("dev", "a long new password", { passkey: { challenge: "AQ", credential: { id: "c" } as never } });
  await api.deletePassword("dev", { passkey: { challenge: "AQ", credential: { id: "c" } as never } });
  assert.deepEqual(
    sent.map((s) => `${s.method} ${s.url.replace("https://hub.test", "")}`),
    [
      "GET /v1/passkeys",
      "POST /v1/passkeys/add/options",
      "POST /v1/passkeys",
      "PATCH /v1/passkeys/pk-1",
      "DELETE /v1/passkeys/pk-1",
      "POST /v1/passkeys/reauth/options",
      "PUT /v1/password",
      "PUT /v1/password",
      "DELETE /v1/password",
    ],
  );
  assert.deepEqual(sent[1]!.body, { current_password: "the old one" }, "adding a way in takes a proof too");
  assert.deepEqual(sent[2]!.body, { challenge: "AQ", credential: { id: "x" }, label: "Phone" });
  assert.deepEqual(sent[4]!.body, { passkey: { challenge: "AQ", credential: { id: "c" } } }, "and taking one away");
  assert.deepEqual(sent[3]!.body, { label: "New" });
  assert.deepEqual(sent[6]!.body, { password: "a long new password", current_password: "the old one" });
  assert.deepEqual(sent[7]!.body, { password: "a long new password", passkey: { challenge: "AQ", credential: { id: "c" } } });
  assert.deepEqual(sent[8]!.body, { passkey: { challenge: "AQ", credential: { id: "c" } } });
});

test("a 409 unlock_on carries the hub's grant (nothing else does), and the removal goes again with it", async () => {
  const { api, sent } = fakeHub(
    { status: 409, body: { error: "unlock_on", message: "on", grant: "g-1" } },
    { status: 409, body: { error: "last_credential", message: "no", grant: "g-2" } },
    { body: { id: "pk-1", removed: true } },
  );
  const first = await api.deletePasskey("dev", "pk-1", { currentPassword: "pw" }).catch((e: unknown) => e);
  assert.ok(first instanceof HubApiError && first.code === "unlock_on" && first.grant === "g-1");
  const second = await api.deletePasskey("dev", "pk-1", { currentPassword: "pw" }).catch((e: unknown) => e);
  assert.ok(second instanceof HubApiError && second.grant === undefined);
  await api.deletePasskey("dev", "pk-1", { grant: "g-1" });
  assert.deepEqual(sent[2]!.body, { grant: "g-1" });
});

// ------------------------------------------------------------ removing a passkey a slot names

/** An account for `removePasskey`: what it signs (the slot removals), and the hub's answers to the removals in turn. */
function removing(answers: (Error | null)[], slots: Record<string, string> = {}) {
  const log: string[] = [];
  const proofs: Reauth[] = [];
  const acct = {
    api: {
      deletePasskey: async (_t: string, id: string, proof: Reauth) => {
        proofs.push(proof);
        log.push(`DELETE ${id}`);
        const a = answers.shift();
        if (a) throw a;
      },
    },
    passkeySlotFor: (hash: string) => (slots[hash] ? { slot: { slot: slots[hash] } } : null),
    disablePasskeyJoin: async (slot: string) => void log.push(`slotRemove ${slot}`),
    sync: async () => (log.push("sync"), 0),
  };
  return { acct: acct as never, log, proofs };
}

/** SHA-256 of 01 02 03 ("AQID"): the hash a slot names that credential by. */
const AQID_HASH = "039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81";

test("removing a passkey: the hub takes the proof first; a slot that names it goes only then, and the removal goes on with the grant", async () => {
  const p = { id: "pk-1", credentialId: "AQID" };
  // No slot names it: one call, nothing signed.
  const plain = removing([null]);
  await removePasskey(plain.acct, "dev", p, { currentPassword: "pw" });
  assert.deepEqual(plain.log, ["DELETE pk-1"]);
  // A slot names it: proof taken (409 with a grant), the slot that names this very credential goes, then the grant.
  const named = removing([new HubApiError(409, "unlock_on", "on", { grant: "g-1" }), null], { [AQID_HASH]: "p-aaaaaaaaaaaa" });
  await removePasskey(named.acct, "dev", p, { currentPassword: "pw" });
  assert.deepEqual(named.log, ["DELETE pk-1", "slotRemove p-aaaaaaaaaaaa", "DELETE pk-1"]);
  assert.deepEqual(named.proofs, [{ currentPassword: "pw" }, { grant: "g-1" }]);
  // A proof the hub refuses: no slot is touched.
  const wrong = removing([new HubApiError(403, "reauth_failed", "no")], { [AQID_HASH]: "p-aaaaaaaaaaaa" });
  await assert.rejects(removePasskey(wrong.acct, "dev", p, { currentPassword: "nope" }), (e: unknown) => e instanceof HubApiError && e.code === "reauth_failed");
  assert.deepEqual(wrong.log, ["DELETE pk-1"]);
  // 409 with no grant (no proof taken): nothing signed.
  const bare = removing([new HubApiError(409, "unlock_on", "on")], { [AQID_HASH]: "p-aaaaaaaaaaaa" });
  await assert.rejects(removePasskey(bare.acct, "dev", p, { currentPassword: "pw" }));
  assert.deepEqual(bare.log, ["DELETE pk-1"]);
  // The hub's slot is one this view lacks: it looks again; still none, nothing signed.
  const behind = removing([new HubApiError(409, "unlock_on", "on", { grant: "g-1" })]);
  await assert.rejects(removePasskey(behind.acct, "dev", p, { currentPassword: "pw" }));
  assert.deepEqual(behind.log, ["DELETE pk-1", "sync"]);
});

test("the slot a passkey's row shows is the one naming its credential id as this page hashes it, not the hub's cred_hash", () => {
  // The hub says "ab…" for it; the page hashed its credential id itself: the slot that names that hash is the row's.
  const html = page({ password: true, max: 10, passkeys: [passkey({ credHash: "ab".repeat(32) })] }, { slotFor: AQID_HASH }, "en", new Map([["pk-1", AQID_HASH]]));
  assert.match(html, /role="switch" aria-checked="true"/);
  const lying = page({ password: true, max: 10, passkeys: [passkey({ credHash: AQID_HASH })] }, { slotFor: AQID_HASH }, "en", new Map([["pk-1", "cd".repeat(32)]]));
  assert.match(lying, /role="switch" aria-checked="false"/, "the hub's hash is not what decides");
  // Not worked out yet: held, not guessed.
  const early = page({ password: true, max: 10, passkeys: [passkey()] }, { slotFor: AQID_HASH }, "en", new Map());
  assert.match(early, /role="switch" aria-checked="false"[^>]*aria-disabled="true"/);
});

test("the settings open on a refused join's passkey removal, once", () => {
  askRemoval("ef".repeat(32));
  assert.equal(takeAskedRemoval(), "ef".repeat(32));
  assert.equal(takeAskedRemoval(), null);
});

// ------------------------------------------------------------ adding a passkey, its PRF

const CREATE: CreationOptionsJSON = {
  rp: { id: "hub.test", name: "Droppen" },
  user: { id: toBase64Url(new Uint8Array(32).fill(7)), name: "me@example.test", displayName: "me@example.test" },
  challenge: toBase64Url(new Uint8Array([1, 2, 3])),
  pubKeyCredParams: [{ type: "public-key", alg: -7 }],
  excludeCredentials: [],
  extensions: { credProps: true, prf: { eval: { first: toBase64Url(new Uint8Array(32).fill(0xaa)) } } },
};

/** A credential as a browser answers `create()` / `get()`: with a PRF output when `prf` is given. */
function credential(kind: "create" | "get", prf: Uint8Array | null) {
  const response: Record<string, unknown> = { clientDataJSON: new Uint8Array([1]).buffer };
  if (kind === "create") response.attestationObject = new Uint8Array([2]).buffer;
  else Object.assign(response, { authenticatorData: new Uint8Array([3]).buffer, signature: new Uint8Array([4]).buffer, userHandle: null });
  return {
    type: "public-key",
    id: toBase64Url(new Uint8Array([9, 9])),
    rawId: new Uint8Array([9, 9]).buffer,
    response,
    getClientExtensionResults: () => ({ credProps: { rk: true }, prf: prf ? { enabled: true, results: { first: prf.slice().buffer } } : { enabled: false } }),
  };
}

test("adding a passkey can ask its PRF at creation: the output comes back here, and the hub never sees it", async () => {
  const asked: unknown[] = [];
  const out = new Uint8Array(32).fill(5);
  const credentials: CredentialsLike = {
    create: async (o) => (asked.push(o.publicKey), credential("create", out)),
    get: async () => null,
  };
  let sentToHub: unknown = null;
  const api = {
    passkeyAddOptions: async () => ({ challenge: "AQ", options: CREATE }),
    addPasskey: async (_t: string, input: unknown) => ((sentToHub = input), { id: "pk-2" } as AccountPasskey),
  };
  const input = new Uint8Array([7, 7, 7]);
  const added = await passkeyAdd(api, { credentials, browser: null }, { token: "dev", proof: { currentPassword: "pw" }, prfInput: input, label: "Phone" });
  assert.deepEqual([...added.prf!], [...out]);
  const ext = (asked[0] as { extensions: { prf: { eval: { first: Uint8Array } } } }).extensions;
  assert.deepEqual([...new Uint8Array(ext.prf.eval.first)], [...input], "the slot's PRF input replaced the hub's throwaway one");
  const json = JSON.stringify(sentToHub);
  assert.ok(json.includes('"prf":{"enabled":true}'), json);
  assert.ok(!json.includes("results"), "no PRF output to the hub");
  // Without an input: none asked, none returned.
  const plain = await passkeyAdd(api, { credentials, browser: null }, { token: "dev", proof: { currentPassword: "pw" } });
  assert.equal(plain.prf, null);
  // The credential id is the authenticator's, as it answered — whatever the hub's row echoes.
  assert.equal(added.credentialId, toBase64Url(new Uint8Array([9, 9])));
  const page = readFileSync(new URL("../src/features/cloud/account-settings.tsx", import.meta.url), "utf8");
  assert.ok(page.includes("enablePasskeyJoin(fromBase64Url(made.credentialId)"), "the slot names the credential made here");
  assert.ok(!page.includes("made.passkey.credentialId"));
});

test("a PRF-only get() names the passkey, asks the input, and sends nothing anywhere; no output reads as null", async () => {
  const asked: { allowCredentials?: { id: ArrayBuffer }[]; extensions?: { prf?: unknown }; rpId?: string }[] = [];
  const answers = [credential("get", new Uint8Array(32).fill(1)), credential("get", null)];
  const credentials: CredentialsLike = { create: async () => null, get: async (o) => (asked.push(o.publicKey as never), answers.shift()) };
  const one = await passkeyPrf({ credentials, browser: null }, { rpId: "hub.test", credentialId: toBase64Url(new Uint8Array([9, 9])), prfInput: new Uint8Array([1]) });
  assert.equal(one.credentialId, toBase64Url(new Uint8Array([9, 9])));
  assert.equal(one.prf?.length, 32);
  assert.equal(asked[0]!.rpId, "hub.test");
  assert.deepEqual([...new Uint8Array(asked[0]!.allowCredentials![0]!.id)], [9, 9]);
  const none = await passkeyPrf({ credentials, browser: null }, { prfInput: new Uint8Array([1]) });
  assert.equal(none.prf, null);
  assert.equal(asked[1]!.rpId, undefined, "no RP id named: the page's own");
  assert.equal(prfOutputOf({ getClientExtensionResults: () => ({ prf: { results: { first: new Uint8Array(16).buffer } } }) }), null, "only a 32-byte output");
});

test("the passkey a tab signed in with is kept for that tab, for that account", () => {
  rememberPasskeySignIn({ account: "a-1", credentialId: "AQID", rpId: "hub.test" });
  assert.deepEqual(passkeySignIn("a-1"), { account: "a-1", credentialId: "AQID", rpId: "hub.test" });
  assert.equal(passkeySignIn("a-2"), null);
  forgetPasskeySignIn();
  assert.equal(passkeySignIn("a-1"), null);
  assert.deepEqual([...fromBase64Url("AQID")], [1, 2, 3]);
});

// ------------------------------------------------------------ the page

const passkey = (over: Partial<AccountPasskey> = {}): AccountPasskey => ({
  id: "pk-1",
  label: "My laptop",
  provider: "iCloud Keychain",
  credentialId: "AQID",
  credHash: "ab".repeat(32),
  backupEligible: true,
  backupState: true,
  prf: "yes",
  createdMs: Date.UTC(2026, 9, 17),
  createdBy: "d-1",
  lastUsedMs: null,
  unlock: "off",
  ...over,
});

/** A device's account as the page reads it: full or not, a v3 chain with recovery words or not, a slot for `slotFor`. */
function account(over: { admin?: boolean; words?: boolean; slotFor?: string | null; v3?: boolean } = {}) {
  return {
    account: "a-1",
    standing: "active",
    isAdmin: over.admin ?? true,
    chain: { isV3: over.v3 ?? true, currentRecoveryKey: over.words === false ? null : new Uint8Array(32) },
    session: { token: "dev" },
    passkeySlotFor: (hash: string) => (over.slotFor && hash === over.slotFor ? { slot: { slot: "p-aaaaaaaaaaaa" } } : null),
  } as never;
}

/** The settings drawn; `hashes`: the credential hashes as worked out (by default each row's own `credHash`). */
function page(methods: SignInMethods | null, over: Parameters<typeof account>[0] = {}, lng = "en", hashes?: ReadonlyMap<string, string>) {
  const worked = hashes ?? new Map((methods?.passkeys ?? []).map((p) => [p.id, p.credHash]));
  return draw(
    <AccountSettings account={account(over)} email="me@example.test" verified={true} methods={methods} hashes={worked} loadError={null} rpId="hub.test" token="dev" onChanged={() => undefined} onWords={() => undefined} />,
    lng,
  );
}

test("the page: the e-mail, the password, every passkey with what it is, new devices joining, the recovery words and signing out", () => {
  const html = page({ password: true, max: 10, passkeys: [passkey({ credHash: "cd".repeat(32), lastUsedMs: Date.now() }), passkey({ id: "pk-2", label: "", provider: "YubiKey 5", backupEligible: false, backupState: false })] }, { slotFor: "cd".repeat(32) });
  assert.match(html, /data-testid="account-title"[^>]*>my account</);
  assert.match(html, /data-testid="account-email"[^>]*>me@example.test</);
  assert.ok(html.includes(">verified<"));
  assert.ok(html.includes("Set: you can sign in with your e-mail and this password."));
  assert.ok(html.includes(">change password<"));
  assert.equal(html.match(/data-testid="passkey-row"/g)?.length, 2);
  assert.ok(html.includes("iCloud Keychain · synced · added "), "provider, synced, when");
  assert.ok(html.includes(">YubiKey 5<"), "no label: the provider's name");
  assert.ok(html.includes(">not synced: on this device or security key only · added "), "the provider said once, as its name");
  assert.ok(!html.includes("YubiKey 5 · "));
  assert.ok(html.includes("never used"));
  // The first passkey's slot is on the chain: its switch is on; the second's is off.
  const switches = [...html.matchAll(/role="switch" aria-checked="(true|false)"/g)].map((m) => m[1]);
  assert.deepEqual(switches, ["true", "false"]);
  assert.ok(html.includes("data-testid=\"join-explainer\""));
  assert.match(html, /data-variant="primary"[^>]*data-testid="add-passkey"/);
  assert.equal(html.match(/data-variant="primary"/g)?.length, 1, "one primary action on the page");
  assert.ok(html.includes('data-testid="recovery-words-section"') && html.includes(">make new recovery words<"));
  assert.ok(html.includes('data-testid="sign-out-section"'));
});

test("an account made with a passkey can set a password (with a passkey); the last way to sign in cannot be removed", () => {
  const html = page({ password: false, max: 10, passkeys: [passkey()] });
  assert.ok(html.includes("Not set: you sign in with a passkey only."));
  assert.ok(html.includes(">set a password<"));
  assert.match(html, /data-testid="passkey-remove"[^>]*disabled=""[^>]*title="the account&#x27;s last way to sign in/);
  // With a password too, it can go.
  const both = page({ password: true, max: 10, passkeys: [passkey()] });
  assert.doesNotMatch(both, /data-testid="passkey-remove"[^>]*disabled=""/);
  assert.ok(both.includes('data-testid="password-remove"'), "the password can go while a passkey stays");
  // Only a password: it stays (nothing offers to remove it).
  const only = page({ password: true, max: 10, passkeys: [] });
  assert.ok(!only.includes('data-testid="password-remove"'));
  assert.ok(only.includes('data-testid="no-passkeys"'));
});

test("a device that is not full sees the page, and changes nothing", () => {
  const html = page({ password: true, max: 10, passkeys: [passkey()] }, { admin: false });
  assert.ok(html.includes('data-testid="read-only"'));
  assert.ok(!html.includes('data-testid="add-passkey"'));
  assert.ok(!html.includes(">change password<"));
  assert.ok(!html.includes(">rename<"));
  assert.match(html, /role="switch"[^>]*aria-disabled="true"/);
  assert.ok(!html.includes(">make new recovery words<"));
});

test("before the account is upgraded to chain v3, new devices cannot join with a passkey yet: said, not offered", () => {
  const html = page({ password: true, max: 10, passkeys: [passkey()] }, { v3: false });
  assert.match(html, /role="switch"[^>]*aria-disabled="true"[^>]*title="This needs the account upgraded first/);
});

test("a passkey named as its provider: the provider said once; no PRF in the words", () => {
  const html = page({ password: true, max: 10, passkeys: [passkey({ label: "iCloud Keychain" })] });
  assert.ok(html.includes(">iCloud Keychain<"));
  assert.ok(!html.includes("iCloud Keychain · synced"), "not said again in the line under it");
  assert.ok(html.includes(">synced · added "));
  assert.ok(!/\bPRF\b/.test(html.replace(/<[^>]*>/g, "")), "no PRF in what the page says");
  assert.ok(html.includes("Some browsers and password managers do not support this."));
  assert.ok(html.includes("any full device can refuse it"));
});

test("the page in Chinese", () => {
  const html = page({ password: false, max: 10, passkeys: [passkey()] }, {}, "zh-CN");
  for (const s of ["账户", "登录方式", "通行密钥", "设置密码", "新设备可用它加入", "恢复词", "退出登录"]) assert.ok(html.includes(s), s);
});

test("/account is the shell's other page with no device in view; the bar and 「我的设备」 lead to it", () => {
  assert.ok(isAccountPath("/account") && isAccountPath("/account/"));
  assert.ok(!isAccountPath("/devices") && !isAccountPath("/accounts"));
  assert.ok(isShellPagePath("/account") && isShellPagePath("/devices") && !isShellPagePath("/sessions"));
  const button = draw(<AccountButton />);
  assert.match(button, /data-testid="account-settings"/);
  assert.ok(button.includes(">my account<"));
  const bar = readFileSync(new URL("../src/features/cloud/cloud-bar.tsx", import.meta.url), "utf8");
  assert.equal(bar.match(/<AccountButton \/>/g)?.length, 1);
  assert.equal(bar.match(/<PendingPasskeyBanner /g)?.length, 1, "the bar says when a device asks to join");
  const frame = readFileSync(new URL("../src/features/cloud/my-devices.tsx", import.meta.url), "utf8");
  assert.match(frame, /data-testid="my-devices-account"/);
  assert.equal(frame.match(/<PendingPasskeyBanner /g)?.length, 1);
  const app = readFileSync(new URL("../src/features/cloud/cloud-app.tsx", import.meta.url), "utf8");
  assert.match(app, /isAccountPath\(pathname\)\) return <AccountScreen /);
});

// ------------------------------------------------------------ joining with a passkey

test("the new device's tab: what joining with a passkey does, in one button", () => {
  const html = draw(<PasskeyJoin account={{ account: "a-1" } as never} onPending={() => undefined} />);
  assert.ok(html.includes("data-testid=\"passkey-join\""));
  assert.ok(html.includes(">join with a passkey<"));
  assert.ok(html.includes("until you approve it from another device of the account, with the code or QR code it shows"));
  assert.ok(html.includes("Any full device of the account can refuse it."));
  assert.ok(!html.includes("two-digit"));
});

test("the pending device: the ordinary pairing offer, made of its own keys — no number read off the chain", () => {
  const src = readFileSync(new URL("../src/features/cloud/pairing-screen.tsx", import.meta.url), "utf8");
  const screen = src.slice(src.indexOf("export function PasskeyPendingScreen"));
  assert.match(screen, /<PairingScreen account=\{account\} waiting /);
  assert.ok(!screen.includes("deviceRecord"), "nothing on it comes from the chain's record of this device");
  const offer = readFileSync(new URL("../src/lib/hub/pairing.ts", import.meta.url), "utf8");
  assert.ok(offer.includes("signKey: this.deps.signPublic, dhKey: this.deps.noise.publicKey"), "the offer is its own keys");
});

/** A full device's account (or not) with one device waiting to join with a passkey, by slot `p-aaaaaaaaaaaa`. */
const waitingJoin = (admin: boolean) =>
  ({
    standing: "active",
    isAdmin: admin,
    chain: {
      pendingPasskeyJoins: () => [{ id: "d-new-phone-0123456789", joinSeq: 4 }],
      entryAt: () => ({ draft: { createdMs: Date.UTC(2026, 9, 21, 12) } }),
      deviceRecord: () => ({ viaPasskey: "p-aaaaaaaaaaaa" }),
      allSlots: () => [{ slot: { slot: "p-aaaaaaaaaaaa", credHash: new Uint8Array(32).fill(0xef) } }],
    },
    onChange: () => () => undefined,
  }) as never;

test("every full device: a device asking to join with a passkey, approve (the ordinary pairing) and not me — in a status line, not an alert around buttons", () => {
  const html = draw(<PendingPasskeyBanner account={waitingJoin(true)} link={null as never} onRemovePasskey={() => undefined} />);
  assert.match(html, /data-testid="pending-passkey-joins"[^>]*data-signal="alarm"/);
  assert.ok(!html.includes('role="alert"'));
  assert.match(html, /<p role="status"/);
  assert.match(html, /class="font-mono whitespace-nowrap text-fg">d-new-phone-0123456789</, "the id never breaks mid-word");
  assert.ok(html.includes(">approve<") && html.includes(">not me: remove it<"));
  assert.ok(!html.includes("join-code-input"), "no number to type: the pairing binds the approval");
  assert.equal(draw(<PendingPasskeyBanner account={waitingJoin(false)} link={null as never} onRemovePasskey={() => undefined} />), "", "only a full device approves or removes");
  const src = readFileSync(new URL("../src/features/cloud/passkey-join.tsx", import.meta.url), "utf8");
  assert.match(src, /<AddDevice account=\{account\} link=\{link\} initialMode="code"/, "approve opens the ordinary pairing, on the code");
});

test("\"not me\": the warning first — the passkey still signs in; its sync account; remove and remake it — then the way to remove it", () => {
  const src = readFileSync(new URL("../src/features/cloud/passkey-join.tsx", import.meta.url), "utf8");
  for (const s of [
    "someone else can use that passkey — most likely through the account it syncs with",
    "but the passkey still signs in. Secure that account first, then remove the passkey and make a new one.",
    "The passkey it joined with still signs in to your account.",
    'data-testid="remove-that-passkey"',
    "onRemovePasskey(r.credHash!)",
  ]) {
    assert.ok(src.includes(s), s);
  }
  // The passkey it names: the slot that let the device in, by its credential hash.
  assert.ok(src.includes("account.chain?.allSlots().find((s) => s.slot.slot === slot)"));
});

// ------------------------------------------------------------ sign-in changes, said on every other full device

const change = (over: Partial<SignInChange> = {}): SignInChange => ({ id: 1, kind: "passkey_added", device: "d-phone", label: "iCloud Keychain", atMs: Date.now() - 60_000, ...over });

test("the store: a first read is history; what is pushed or read after is said until \"that was me\"", async () => {
  const storage = new MemoryStorage();
  let changes: SignInChange[] = [change({ id: 1 })];
  const store = new MeStore(async () => ({ devices: [], signInChanges: changes }) as never, { storage, account: "a-1" });
  assert.deepEqual(store.signInChanges(), [], "nothing before the first read");
  await store.refresh();
  assert.deepEqual(store.signInChanges(), [], "what changed before this browser first looked is history");
  assert.ok(store.control({ t: "sign_in_changed", id: 2, kind: "password_changed", device: "d-laptop", label: null, at_ms: 5 }));
  assert.deepEqual(store.signInChanges().map((c) => [c.id, c.kind, c.device]), [[2, "password_changed", "d-laptop"]]);
  changes = [change({ id: 1 }), change({ id: 3, kind: "passkey_removed" })];
  await store.refresh();
  assert.deepEqual(store.signInChanges().map((c) => c.id), [2, 3]);
  assert.equal(store.signInChangeHead, 3, "the newest heard of: what the settings page reads its list again on");
  store.dismissSignInChange(2);
  assert.deepEqual(store.signInChanges().map((c) => c.id), [3]);
  // Another page of this browser: the same baseline and dismissals.
  const again = new MeStore(async () => ({ devices: [], signInChanges: changes }) as never, { storage, account: "a-1" });
  await again.refresh();
  assert.deepEqual(again.signInChanges().map((c) => c.id), [3]);
  assert.equal(store.control({ t: "sign_in_changed", id: "x" }), true, "taken (and dropped) even when malformed");
});

test("the notice: what changed, from which device, when — for every kind", () => {
  const t = (key: string, vars?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_m, k: string) => String(vars?.[k] ?? ""));
  const say = (kind: SignInChange["kind"], label: string | null = "Key") => signInChangeText(change({ kind, label }), "iPhone · Safari (d-phone)", "2 min ago", t);
  assert.equal(say("passkey_added"), "A passkey (Key) was added to your account from iPhone · Safari (d-phone), 2 min ago.");
  assert.equal(say("passkey_renamed"), "A passkey was renamed to Key from iPhone · Safari (d-phone), 2 min ago.");
  assert.equal(say("passkey_removed"), "A passkey (Key) was removed from your account from iPhone · Safari (d-phone), 2 min ago.");
  assert.equal(say("password_set", null), "A password was set for your account from iPhone · Safari (d-phone), 2 min ago.");
  assert.equal(say("password_changed", null), "Your password was changed from iPhone · Safari (d-phone), 2 min ago.");
  assert.equal(say("password_removed", null), "Your password was removed from iPhone · Safari (d-phone), 2 min ago.");
  const html = draw(<SignInChangeList changes={[change({ id: 9 })]} names={new Map([["d-phone", "iPhone · Safari"]])} onDismiss={() => undefined} onReview={() => undefined} />);
  assert.match(html, /data-testid="sign-in-change-9"[^>]*data-kind="passkey_added"/);
  assert.ok(html.includes("from iPhone · Safari (d-phone)"));
  assert.ok(html.includes(">that was me<") && html.includes(">sign-in methods<"));
  assert.equal(draw(<SignInChangeList changes={[]} names={new Map()} onDismiss={() => undefined} />), "");
  // Every full device but the one that made it; on all three pages.
  const plan = readFileSync(new URL("../src/features/cloud/plan.tsx", import.meta.url), "utf8");
  assert.ok(plan.includes("store.signInChanges().filter((c) => c.device !== account.deviceId)"));
  assert.ok(plan.includes('account.standing !== "active" || !account.isAdmin'));
  const settings = readFileSync(new URL("../src/features/cloud/account-settings.tsx", import.meta.url), "utf8");
  assert.ok(settings.includes("const head = me?.signInChangeHead ?? 0;"), "the settings read their list again when a change is heard");
  for (const f of ["cloud-bar.tsx", "my-devices.tsx", "account-settings.tsx"]) {
    const src = readFileSync(new URL(`../src/features/cloud/${f}`, import.meta.url), "utf8");
    assert.equal(src.match(/<SignInChangeAlerts /g)?.length, 1, f);
  }
});

test("every string the page and the passkey join use is translated into zh-CN, zh-TW and ja", () => {
  const keys = new Set<string>();
  const files = ["account-settings.tsx", "passkey-join.tsx", "my-devices.tsx", "pairing-screen.tsx", "sign-out-dialog.tsx"].map((f) => readFileSync(new URL(`../src/features/cloud/${f}`, import.meta.url), "utf8"));
  files.push(readFileSync(new URL("../src/lib/hub/passkey-join.ts", import.meta.url), "utf8"));
  // The sign-in change notices, and approving a join by pairing (only these parts of those files).
  const plan = readFileSync(new URL("../src/features/cloud/plan.tsx", import.meta.url), "utf8");
  files.push(plan.slice(plan.indexOf("export function SignInChangeAlerts"), plan.indexOf("Where a session's state stands")));
  const devices = readFileSync(new URL("../src/features/cloud/devices.tsx", import.meta.url), "utf8");
  files.push(devices.slice(devices.indexOf("export function ConfirmCandidate"), devices.indexOf("export function blockedByHold")));
  // The pairing's refusal of an offer from another device than the one waiting.
  keys.add("the device that showed this code is not the one that asked to join with a passkey: nothing was approved");
  // Approving a passkey's join any other way than by pairing (`CloudAccount.approveDevice`).
  keys.add("{{id}} joined with a passkey: approve it by scanning its QR code or typing its code in my devices → add a browser or phone");
  for (const src of files) {
    for (const m of src.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(m[1]!);
    for (const m of src.matchAll(/AccountError\(\s*"[a-zA-Z]+",\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(m[1]!);
  }
  const account = readFileSync(new URL("../src/lib/hub/account.ts", import.meta.url), "utf8");
  const start = account.indexOf("joining with a passkey (docs/passkeys.md §4)");
  for (const m of account.slice(start, account.indexOf("Keep the `UK` epochs a pairing delivered", start)).matchAll(/AccountError\(\s*"[a-zA-Z]+",\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(m[1]!);
  assert.ok(keys.size > 60, String(keys.size));
  for (const [name, table] of [
    ["zh-CN", { ...onboardingZhCN, ...crossDeviceZhCN, ...cloudDevicesZhCN, ...signOutZhCN, ...zhCN }],
    ["zh-TW", { ...onboardingZhTW, ...crossDeviceZhTW, ...cloudDevicesZhTW, ...signOutZhTW, ...zhTW }],
    ["ja", { ...onboardingJa, ...crossDeviceJa, ...cloudDevicesJa, ...signOutJa, ...ja }],
  ] as const) {
    const missing = [...keys].filter((k) => !(k in table));
    assert.deepEqual(missing, [], `${name} lacks these`);
  }
});
