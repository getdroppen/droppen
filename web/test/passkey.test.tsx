// Passkey sign-in and sign-up (docs/passkeys.md §3, K1): `lib/hub/passkey.ts`,
// the hub API's four passkey calls, and the sign-in page's choice of form.
//
// What these pin: WebAuthn's JSON is base64url without padding both ways, with
// the browser's own parse / toJSON where it has them and the same result
// without; what goes back to the hub is the response's own fields and, of the
// extension results, only `credProps.rk` and `prf.enabled` (a PRF output never
// leaves the page); every capability probe is a plain boolean whatever the
// browser does (missing, throwing, never answering); the hub calls send what
// the contract says and a refused passkey reads as a refused passkey, not an
// expired sign-in; a sign-up the hub refuses never calls `create()`, and a new
// passkey the hub then refuses is signalled unknown; the autofill asks afresh
// before the 2-minute challenge runs out, stops on its signal, and is withdrawn
// before a modal `get()` (one request at a time); and a page without WebAuthn,
// or a hub without passkeys, gets the password form with no passkey wording.
//
// `react-dom/server` with no DOM, like the other render tests.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { authErrorKind, HubApi, HubApiError, knownClientKey, type SignupConfig } from "../src/lib/hub/api.js";
import {
  AUTOFILL_REFRESH_MS,
  authenticationToJSON,
  conditionalPasskeyGet,
  type CreationOptionsJSON,
  type CredentialsLike,
  creationOptionsFromJSON,
  finishPasskeyLogin,
  fromBase64Url,
  type PasskeyBrowser,
  PasskeyAutofill,
  passkeyCancelled,
  passkeyLogin,
  passkeySignUp,
  probePasskeys,
  registrationToJSON,
  type RequestOptionsJSON,
  requestOptionsFromJSON,
  safeExtensionResults,
  signalUnknownCredential,
  type Timers,
  toBase64Url,
  webAuthnAvailable,
} from "../src/lib/hub/passkey.js";
import { authErrorText } from "../src/features/cloud/ui.js";
import { AuthScreen, authMethod } from "../src/features/cloud/cloud-app.js";

void i18next.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

const bytes = (...b: number[]) => new Uint8Array(b);
const buf = (...b: number[]) => bytes(...b).buffer;
const same = (a: unknown, b: Uint8Array) => {
  assert.ok(a instanceof Uint8Array || a instanceof ArrayBuffer, "bytes");
  assert.deepEqual([...new Uint8Array(a as ArrayBuffer)], [...b]);
};
/** Let every settled promise run on. */
const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((r) => setImmediate(r));
};

// ------------------------------------------------------------ base64url

test("base64url: no padding out, the url alphabet, every length, and only a view's own bytes", () => {
  assert.equal(toBase64Url(bytes()), "");
  assert.equal(toBase64Url(bytes(0)), "AA");
  assert.equal(toBase64Url(bytes(0, 0)), "AAA");
  assert.equal(toBase64Url(bytes(0, 0, 0)), "AAAA");
  assert.equal(toBase64Url(bytes(0xfb, 0xff)), "-_8", "- and _ where base64 has + and /");
  assert.equal(toBase64Url(buf(0xfb, 0xef, 0xbe)), "----");
  assert.equal(toBase64Url(buf(0xff, 0xff, 0xff)), "____");
  const big = new Uint8Array(70_000).map((_, i) => (i * 7 + 3) & 0xff);
  assert.deepEqual([...fromBase64Url(toBase64Url(big))], [...big], "past one chunk of String.fromCharCode");
  for (let n = 0; n < 12; n++) {
    const b = new Uint8Array(n).map((_, i) => 0xf0 + i);
    const text = toBase64Url(b);
    assert.doesNotMatch(text, /[=+/]/);
    assert.deepEqual([...fromBase64Url(text)], [...b], `length ${n}`);
  }
  const whole = bytes(1, 2, 3, 4, 5, 6, 7, 8);
  assert.equal(toBase64Url(whole.subarray(2, 5)), toBase64Url(bytes(3, 4, 5)));
  assert.equal(toBase64Url(new DataView(whole.buffer, 6, 2)), toBase64Url(bytes(7, 8)));
});

test("base64url: padding is tolerated when right; anything else is refused", () => {
  assert.deepEqual([...fromBase64Url("AA==")], [0]);
  assert.deepEqual([...fromBase64Url("AAA=")], [0, 0]);
  assert.deepEqual([...fromBase64Url("-_8")], [0xfb, 0xff]);
  for (const bad of ["A", "AAAAA", "AA=", "AA===", "+/8", "AA AA", "AA\n", "AA==AA", "é", "AA=A"]) {
    assert.throws(() => fromBase64Url(bad), TypeError, JSON.stringify(bad));
  }
  assert.throws(() => fromBase64Url(42 as unknown as string), TypeError);
});

// ------------------------------------------------------------ options from JSON

const CREATE: CreationOptionsJSON = {
  rp: { id: "hub.test", name: "Droppen" },
  user: { id: toBase64Url(new Uint8Array(32).fill(7)), name: "me@example.test", displayName: "me@example.test" },
  challenge: toBase64Url(bytes(1, 2, 3, 0xfb, 0xff)),
  pubKeyCredParams: [
    { type: "public-key", alg: -8 },
    { type: "public-key", alg: -7 },
    { type: "public-key", alg: -257 },
  ],
  timeout: 120_000,
  excludeCredentials: [{ type: "public-key", id: toBase64Url(bytes(9, 9)), transports: ["internal"] }],
  authenticatorSelection: { residentKey: "required", requireResidentKey: true, userVerification: "required" },
  attestation: "none",
  extensions: { credProps: true, prf: { eval: { first: toBase64Url(new Uint8Array(32).fill(0xaa)), second: toBase64Url(bytes(5)) } } },
};

const REQUEST: RequestOptionsJSON = {
  challenge: toBase64Url(bytes(4, 5, 6)),
  rpId: "hub.test",
  timeout: 120_000,
  userVerification: "required",
  allowCredentials: [{ type: "public-key", id: toBase64Url(bytes(0xfe, 0xed)) }],
  extensions: { prf: { evalByCredential: { [toBase64Url(bytes(0xfe, 0xed))]: { first: toBase64Url(bytes(1)) } } } },
};

test("creation options without the browser's parser: every binary field as bytes, the rest as given", () => {
  const o = creationOptionsFromJSON(CREATE, null) as unknown as Record<string, any>;
  same(o.challenge, bytes(1, 2, 3, 0xfb, 0xff));
  same(o.user.id, new Uint8Array(32).fill(7));
  assert.equal(o.user.name, "me@example.test");
  assert.equal(o.user.displayName, "me@example.test");
  same(o.excludeCredentials[0].id, bytes(9, 9));
  assert.deepEqual(o.excludeCredentials[0].transports, ["internal"]);
  same(o.extensions.prf.eval.first, new Uint8Array(32).fill(0xaa));
  same(o.extensions.prf.eval.second, bytes(5));
  assert.equal(o.extensions.credProps, true);
  assert.deepEqual(o.rp, { id: "hub.test", name: "Droppen" });
  assert.deepEqual(o.pubKeyCredParams, CREATE.pubKeyCredParams);
  assert.deepEqual(o.authenticatorSelection, CREATE.authenticatorSelection);
  assert.equal(o.attestation, "none");
  assert.equal(o.timeout, 120_000);
  // The JSON it came from is untouched.
  assert.equal(typeof CREATE.user.id, "string");
  // No exclude list, no extensions: none made up.
  const bare = creationOptionsFromJSON({ ...CREATE, excludeCredentials: undefined, extensions: undefined }, null) as unknown as Record<string, unknown>;
  assert.equal(bare.excludeCredentials, undefined);
  assert.equal(bare.extensions, undefined);
});

test("request options without the browser's parser: the challenge, the allowed ids and evalByCredential's values as bytes", () => {
  const o = requestOptionsFromJSON(REQUEST, null) as unknown as Record<string, any>;
  same(o.challenge, bytes(4, 5, 6));
  same(o.allowCredentials[0].id, bytes(0xfe, 0xed));
  assert.equal(o.rpId, "hub.test");
  assert.equal(o.userVerification, "required");
  const key = toBase64Url(bytes(0xfe, 0xed));
  assert.deepEqual(Object.keys(o.extensions.prf.evalByCredential), [key], "keys stay base64url ids");
  same(o.extensions.prf.evalByCredential[key].first, bytes(1));
  // The hub's sign-in options name no credential: none made up.
  const empty = requestOptionsFromJSON({ challenge: "AQ", rpId: "hub.test", allowCredentials: [] }, null) as unknown as Record<string, any>;
  assert.deepEqual(empty.allowCredentials, []);
});

test("the browser's own parser is used where it has one; where it refuses, the built-in conversion decides", () => {
  const seen: unknown[] = [];
  const parsed = { from: "the browser" };
  const browser: PasskeyBrowser = {
    parseCreationOptionsFromJSON(json) {
      seen.push(json);
      return parsed as unknown as PublicKeyCredentialCreationOptions;
    },
    parseRequestOptionsFromJSON() {
      throw new TypeError("this browser's parser does not know an extension");
    },
  };
  assert.equal(creationOptionsFromJSON(CREATE, browser), parsed);
  assert.equal(seen[0], CREATE, "handed the hub's JSON as it is");
  same((requestOptionsFromJSON(REQUEST, browser) as unknown as { challenge: ArrayBuffer }).challenge, bytes(4, 5, 6));
  // A bad binary field is refused either way.
  assert.throws(() => creationOptionsFromJSON({ ...CREATE, challenge: "not base64url!" }, null), TypeError);
});

// ------------------------------------------------------------ credentials to JSON

const SECRET = new Uint8Array(32).fill(0x5e);

/** A credential `create()` might answer with: `toJSON` as given (absent: none). */
function made(toJSON?: () => unknown): Record<string, unknown> {
  return {
    id: toBase64Url(bytes(0xc0, 0xff, 0xee)),
    rawId: buf(0xc0, 0xff, 0xee),
    type: "public-key",
    authenticatorAttachment: "platform",
    response: {
      clientDataJSON: new TextEncoder().encode('{"type":"webauthn.create"}').buffer,
      attestationObject: buf(0xa3, 1, 2),
      getTransports: () => ["internal", "hybrid"],
      getPublicKey: () => buf(4, 4),
      getAuthenticatorData: () => buf(5, 5),
    },
    getClientExtensionResults: () => ({ credProps: { rk: true }, prf: { enabled: true, results: { first: SECRET.buffer } }, appid: false }),
    ...(toJSON ? { toJSON } : {}),
  };
}

/** A credential `get()` might answer with. */
function asserted(over: { userHandle?: ArrayBuffer | null; toJSON?: () => unknown } = {}): Record<string, unknown> {
  return {
    id: toBase64Url(bytes(0xc0, 0xff, 0xee)),
    rawId: buf(0xc0, 0xff, 0xee),
    type: "public-key",
    authenticatorAttachment: "cross-platform",
    response: {
      clientDataJSON: new TextEncoder().encode('{"type":"webauthn.get"}').buffer,
      authenticatorData: buf(0xad, 0),
      signature: buf(0x51, 0x6e),
      userHandle: over.userHandle === undefined ? new Uint8Array(32).fill(7).buffer : over.userHandle,
    },
    getClientExtensionResults: () => ({ prf: { results: { first: SECRET.buffer } } }),
    ...(over.toJSON ? { toJSON: over.toJSON } : {}),
  };
}

const EXPECTED_REGISTRATION = {
  id: "wP_u",
  rawId: "wP_u",
  type: "public-key",
  response: { clientDataJSON: toBase64Url(new TextEncoder().encode('{"type":"webauthn.create"}')), attestationObject: toBase64Url(bytes(0xa3, 1, 2)), transports: ["internal", "hybrid"] },
  clientExtensionResults: { credProps: { rk: true }, prf: { enabled: true } },
  authenticatorAttachment: "platform",
};

test("a new passkey as JSON, built by hand: base64url fields, its transports, and only credProps.rk and prf.enabled", () => {
  const json = registrationToJSON(made());
  assert.deepEqual(json, EXPECTED_REGISTRATION);
  assert.ok(!JSON.stringify(json).includes(toBase64Url(SECRET)), "no PRF output");
  assert.ok(!JSON.stringify(json).includes("results"));
});

test("a new passkey as JSON from its own toJSON(): the same fields, the extras and any PRF result dropped", () => {
  const json = registrationToJSON(
    made(() => ({
      ...EXPECTED_REGISTRATION,
      response: { ...EXPECTED_REGISTRATION.response, publicKey: "BAQ", publicKeyAlgorithm: -7, authenticatorData: "BQU" },
      clientExtensionResults: { credProps: { rk: true }, prf: { enabled: true, results: { first: toBase64Url(SECRET) } }, largeBlob: { supported: true } },
    })),
  );
  assert.deepEqual(json, EXPECTED_REGISTRATION);
  assert.ok(!JSON.stringify(json).includes(toBase64Url(SECRET)), "no PRF output, even from toJSON()");
  // A toJSON() that throws, or answers without the fields the hub needs: built by hand.
  assert.deepEqual(registrationToJSON(made(() => { throw new Error("broken"); })), EXPECTED_REGISTRATION);
  assert.deepEqual(registrationToJSON(made(() => ({ id: "x", response: {} }))), EXPECTED_REGISTRATION);
  // Not a credential at all.
  assert.throws(() => registrationToJSON(null), TypeError);
  assert.throws(() => registrationToJSON({ type: "password", rawId: buf(1), response: {} }), TypeError);
  // An assertion is not a registration.
  assert.throws(() => registrationToJSON(asserted()), TypeError);
});

test("a passkey's answer as JSON, by hand and from toJSON(): base64url fields, the user handle when there is one, no PRF output", () => {
  const expected = {
    id: "wP_u",
    rawId: "wP_u",
    type: "public-key",
    response: {
      clientDataJSON: toBase64Url(new TextEncoder().encode('{"type":"webauthn.get"}')),
      authenticatorData: toBase64Url(bytes(0xad, 0)),
      signature: toBase64Url(bytes(0x51, 0x6e)),
      userHandle: toBase64Url(new Uint8Array(32).fill(7)),
    },
    clientExtensionResults: {},
    authenticatorAttachment: "cross-platform",
  };
  assert.deepEqual(authenticationToJSON(asserted()), expected);
  assert.deepEqual(authenticationToJSON(asserted({ toJSON: () => ({ ...expected, clientExtensionResults: { prf: { results: { first: toBase64Url(SECRET) } } } }) })), expected);
  // No user handle: none sent (the hub refuses it).
  const { userHandle: _, ...withoutHandle } = expected.response;
  assert.deepEqual(authenticationToJSON(asserted({ userHandle: null })).response, withoutHandle);
  assert.deepEqual(authenticationToJSON(asserted({ userHandle: null, toJSON: () => ({ ...expected, response: { ...withoutHandle, userHandle: null } }) })).response, withoutHandle);
  assert.throws(() => authenticationToJSON(made()), TypeError, "a registration is not an assertion");
});

test("the extension whitelist: credProps.rk and prf.enabled as booleans, nothing else", () => {
  assert.deepEqual(safeExtensionResults({ credProps: { rk: false }, prf: { enabled: false } }), { credProps: { rk: false }, prf: { enabled: false } });
  assert.deepEqual(safeExtensionResults({ prf: { enabled: true, results: { first: "c2VjcmV0" } } }), { prf: { enabled: true } });
  assert.deepEqual(safeExtensionResults({ prf: { results: { first: "c2VjcmV0" } } }), {});
  assert.deepEqual(safeExtensionResults({ credProps: { rk: "yes" }, prf: { enabled: 1 }, appid: true, hmacCreateSecret: true, largeBlob: { blob: "eA" } }), {});
  for (const junk of [null, undefined, "x", 5, [], { credProps: null, prf: "on" }]) assert.deepEqual(safeExtensionResults(junk), {});
});

// ------------------------------------------------------------ probes and signals

const creds = { create: async () => null, get: async () => null };

test("probes: no WebAuthn (no PublicKeyCredential, no navigator.credentials, not a secure context) is no to everything", async () => {
  const none = { webauthn: false, platform: false, conditional: false, prf: null };
  const yes = async () => true;
  const pkc = { isUserVerifyingPlatformAuthenticatorAvailable: yes, isConditionalMediationAvailable: yes };
  assert.deepEqual(await probePasskeys({}), none);
  assert.deepEqual(await probePasskeys({ navigator: { credentials: creds } }), none);
  assert.deepEqual(await probePasskeys({ PublicKeyCredential: pkc, navigator: {} }), none);
  assert.deepEqual(await probePasskeys({ PublicKeyCredential: pkc, navigator: { credentials: { get: creds.get } } }), none);
  assert.deepEqual(await probePasskeys({ PublicKeyCredential: pkc, navigator: { credentials: creds }, isSecureContext: false }), none);
  assert.equal(webAuthnAvailable({ PublicKeyCredential: pkc, navigator: { credentials: creds } }), true);
  // This test process: Node has no WebAuthn.
  assert.equal(webAuthnAvailable(), false);
  const throwing = {
    get PublicKeyCredential(): unknown {
      throw new Error("blocked");
    },
  };
  assert.equal(webAuthnAvailable(throwing), false);
});

test("probes: each answer a plain boolean, whether the function is missing, throws, rejects or never answers", async () => {
  const env = (pkc: unknown) => ({ PublicKeyCredential: pkc, navigator: { credentials: creds } });
  assert.deepEqual(await probePasskeys(env({})), { webauthn: true, platform: false, conditional: false, prf: null }, "an old browser: WebAuthn, nothing more");
  assert.deepEqual(
    await probePasskeys(
      env({
        isUserVerifyingPlatformAuthenticatorAvailable: () => {
          throw new DOMException("not here", "SecurityError");
        },
        isConditionalMediationAvailable: async () => {
          throw new Error("nope");
        },
        getClientCapabilities: () => new Promise(() => undefined),
      }),
      { timeoutMs: 10 },
    ),
    { webauthn: true, platform: false, conditional: false, prf: null },
  );
  assert.deepEqual(
    await probePasskeys(env({ isUserVerifyingPlatformAuthenticatorAvailable: async () => "yes", isConditionalMediationAvailable: () => new Promise(() => undefined) }), { timeoutMs: 10 }),
    { webauthn: true, platform: false, conditional: false, prf: null },
    "not a boolean, or no answer in time: no",
  );
  assert.deepEqual(
    await probePasskeys(
      env({ isUserVerifyingPlatformAuthenticatorAvailable: async () => true, isConditionalMediationAvailable: async () => true, getClientCapabilities: async () => ({ "extension:prf": false }) }),
    ),
    { webauthn: true, platform: true, conditional: true, prf: false },
  );
  // Only getClientCapabilities answers: it says.
  assert.deepEqual(
    await probePasskeys(env({ getClientCapabilities: async () => ({ conditionalGet: true, userVerifyingPlatformAuthenticator: true, "extension:prf": true }) })),
    { webauthn: true, platform: true, conditional: true, prf: true },
  );
  // The older probe's own no stands over a capability's yes.
  assert.deepEqual(
    await probePasskeys(env({ isConditionalMediationAvailable: async () => false, getClientCapabilities: async () => ({ conditionalGet: true }) })),
    { webauthn: true, platform: false, conditional: false, prf: null },
  );
});

test("signalUnknownCredential: called where the function exists (and only its existence counts), never throws", async () => {
  const calls: unknown[] = [];
  const browser: PasskeyBrowser = {
    async signalUnknownCredential(o) {
      calls.push(o);
    },
  };
  assert.equal(await signalUnknownCredential("hub.test", "wP_u", browser), true);
  assert.deepEqual(calls, [{ rpId: "hub.test", credentialId: "wP_u" }]);
  assert.equal(await signalUnknownCredential("hub.test", "wP_u", {}), false, "Firefox: no such function");
  assert.equal(await signalUnknownCredential("hub.test", "wP_u", null), false);
  assert.equal(await signalUnknownCredential("", "wP_u", browser), false, "no RP: nothing to say");
  assert.equal(calls.length, 1);
  const failing: PasskeyBrowser = {
    async signalUnknownCredential() {
      throw new DOMException("no", "SecurityError");
    },
  };
  assert.equal(await signalUnknownCredential("hub.test", "wP_u", failing), false);
  assert.equal(await signalUnknownCredential("hub.test", "wP_u", { signalUnknownCredential: () => { throw new Error("sync"); } } as PasskeyBrowser), false);
});

test("a closed prompt is a cancel, not a failure", () => {
  assert.equal(passkeyCancelled(new DOMException("closed", "NotAllowedError")), true);
  assert.equal(passkeyCancelled(new DOMException("aborted", "AbortError")), true);
  assert.equal(passkeyCancelled(new DOMException("wrong origin", "SecurityError")), false);
  assert.equal(passkeyCancelled(new HubApiError(401, "bad_passkey", "no")), false);
  assert.equal(passkeyCancelled(null), false);
});

// ------------------------------------------------------------ the hub's calls

type Sent = { url: string; method: string; headers: Record<string, string>; body: unknown };

function fakeHub(...answers: ({ status?: number; body?: unknown } | Error)[]) {
  const sent: Sent[] = [];
  const fetch = async (input: string, init?: RequestInit): Promise<Response> => {
    sent.push({
      url: input,
      method: init?.method ?? "GET",
      headers: (init?.headers ?? {}) as Record<string, string>,
      body: init?.body ? JSON.parse(init.body as string) : undefined,
    });
    const a = answers.shift() ?? { status: 500, body: { error: "no_answer" } };
    if (a instanceof Error) throw a;
    return new Response(a.body === undefined ? null : JSON.stringify(a.body), { status: a.status ?? 200, headers: { "content-type": "application/json" } });
  };
  const items = new Map<string, string>();
  const storage = { getItem: (k: string) => items.get(k) ?? null, setItem: (k: string, v: string) => void items.set(k, v) };
  return { api: new HubApi("https://hub.test", fetch, storage), sent, items };
}

const SIGNED = { account: "a-1", token: "login-token", expires_ms: 1_000, email_verified: false, verification: "required", known_client: "kc1.a-1.9.tag" };
const REG_CREDENTIAL = EXPECTED_REGISTRATION as Parameters<HubApi["signUpWithPasskey"]>[0]["credential"];

test("the passkey sign-up calls: the hub's checks first, then the account, as the contract says", async () => {
  const { api, sent, items } = fakeHub({ body: { challenge: CREATE.challenge, options: CREATE } }, { body: SIGNED }, { body: { challenge: "AQ", options: CREATE } });
  const got = await api.passkeySignupOptions({ email: "me@example.test", invite: "INVITE-1", turnstile: "tok", lang: "ja" });
  assert.deepEqual(got, { challenge: CREATE.challenge, options: CREATE });
  assert.equal(sent[0]!.url, "https://hub.test/v1/passkeys/options");
  assert.equal(sent[0]!.method, "POST");
  assert.equal(sent[0]!.headers.authorization, undefined);
  assert.deepEqual(sent[0]!.body, { purpose: "signup", email: "me@example.test", invite: "INVITE-1", turnstile: "tok", lang: "ja" });
  const session = await api.signUpWithPasskey({ challenge: CREATE.challenge, email: "me@example.test", invite: "INVITE-1", lang: "ja", credential: REG_CREDENTIAL });
  assert.equal(sent[1]!.url, "https://hub.test/v1/signup/passkey");
  assert.deepEqual(sent[1]!.body, { challenge: CREATE.challenge, email: "me@example.test", invite: "INVITE-1", lang: "ja", credential: EXPECTED_REGISTRATION });
  assert.deepEqual(session, { account: "a-1", token: "login-token", expiresMs: 1_000, emailVerified: false, verification: "required" });
  assert.equal(items.get((await knownClientKey("me@example.test"))!), "kc1.a-1.9.tag", "the e-mail is known here: its known-client token is kept (under its hash)");
  // No invite, no widget, no language: none sent.
  await api.passkeySignupOptions({ email: "me@example.test", turnstile: null });
  assert.deepEqual(sent[2]!.body, { purpose: "signup", email: "me@example.test" });
});

test("the passkey sign-in calls: an empty body for options, the answer back, the passkey it was, and no known-client token", async () => {
  const { api, sent, items } = fakeHub(
    { body: { challenge: REQUEST.challenge, options: REQUEST } },
    { body: { ...SIGNED, email_verified: true, passkey: { id: "pk-1", label: "iCloud Keychain", unlock: "none" } } },
    { body: { ...SIGNED, passkey: { id: "pk-2", unlock: "sideways" } } },
    { body: SIGNED },
  );
  assert.deepEqual(await api.passkeyLoginOptions(), { challenge: REQUEST.challenge, options: REQUEST });
  assert.equal(sent[0]!.url, "https://hub.test/v1/passkeys/login/options");
  assert.equal(sent[0]!.method, "POST");
  assert.deepEqual(sent[0]!.body, {});
  assert.equal(sent[0]!.headers.authorization, undefined);
  const credential = authenticationToJSON(asserted());
  const session = await api.loginWithPasskey(REQUEST.challenge, credential);
  assert.equal(sent[1]!.url, "https://hub.test/v1/login/passkey");
  assert.deepEqual(sent[1]!.body, { challenge: REQUEST.challenge, credential });
  assert.deepEqual(session, {
    account: "a-1",
    token: "login-token",
    expiresMs: 1_000,
    emailVerified: true,
    verification: "required",
    passkey: { id: "pk-1", label: "iCloud Keychain", unlock: "none" },
  });
  assert.equal(items.size, 0, "a passkey sign-in names no e-mail here: no known-client token is kept");
  assert.deepEqual((await api.loginWithPasskey("AQ", credential)).passkey, { id: "pk-2", label: "", unlock: "none" });
  assert.equal((await api.loginWithPasskey("AQ", credential)).passkey, null);
});

test("options the browser could not use are the hub's fault (502), and nothing is asked of the browser", async () => {
  const bad = [null, {}, { challenge: "AQ" }, { challenge: "AQ", options: {} }, { challenge: "", options: CREATE }, { challenge: "AQ", options: { ...CREATE, user: { name: "x" } } }, { challenge: "AQ", options: { ...CREATE, pubKeyCredParams: null } }];
  for (const body of bad) {
    const { api } = fakeHub({ body });
    const error = await api.passkeySignupOptions({ email: "me@example.test" }).catch((e: unknown) => e);
    assert.ok(error instanceof HubApiError && error.status === 502 && error.code === "bad_options", JSON.stringify(body));
  }
  const { api } = fakeHub({ body: { challenge: "AQ", options: { rpId: "hub.test" } } });
  assert.equal(((await api.passkeyLoginOptions().catch((e: unknown) => e)) as HubApiError).code, "bad_options");
});

test("passkey refusals by kind: a refused passkey is not an expired sign-in", async () => {
  const cases: [number, string, ReturnType<typeof authErrorKind>, string][] = [
    [401, "bad_passkey", "badPasskey", "this passkey could not sign you in: try again"],
    [401, "unknown_passkey", "unknownPasskey", "this passkey is no longer registered here: use another passkey or your password"],
    [403, "passkeys_off", "passkeysOff", "passkeys are turned off on this hub: use your password"],
    [409, "email_taken", "emailTaken", "an account with this e-mail exists already: sign in instead"],
    [403, "turnstile_failed", "turnstileFailed", "the human check failed: try again"],
    [401, "unauthorized", "unauthorized", "your sign-in expired: sign in again"],
  ];
  for (const [status, code, kind, words] of cases) {
    const { api } = fakeHub({ status, body: { error: code, message: `the hub's words for ${code}` } });
    const error = await api.loginWithPasskey("AQ", authenticationToJSON(asserted())).catch((e: unknown) => e);
    assert.equal(authErrorKind(error), kind, code);
    assert.equal(authErrorText(error, "login"), words, code);
  }
  const { api } = fakeHub({ status: 401, body: { error: "bad_passkey", message: "no" } });
  const error = await api.signUpWithPasskey({ challenge: "AQ", email: "me@example.test", credential: REG_CREDENTIAL }).catch((e: unknown) => e);
  assert.equal(authErrorText(error, "signup"), "the hub could not accept this passkey: try again");
});

// ------------------------------------------------------------ sign-up and sign-in

/** `navigator.credentials` that keeps what it was asked; `create` and `get` answer as told. */
function fakeCredentials(answer: { create?: () => Promise<unknown>; get?: () => Promise<unknown> } = {}) {
  const created: { publicKey: PublicKeyCredentialCreationOptions }[] = [];
  const got: { publicKey: PublicKeyCredentialRequestOptions; mediation?: string }[] = [];
  const credentials: CredentialsLike = {
    create: async (o) => {
      created.push(o);
      return answer.create ? answer.create() : made();
    },
    get: async (o) => {
      got.push(o);
      return answer.get ? answer.get() : asserted();
    },
  };
  return { credentials, created, got };
}

/** A browser with `signalUnknownCredential`, keeping what it was told (and no parser: the built-in conversion). */
function signalling() {
  const signals: unknown[] = [];
  const browser: PasskeyBrowser = {
    async signalUnknownCredential(o) {
      signals.push(o);
    },
  };
  return { browser, signals };
}

test("sign-up: the hub refuses the options (the e-mail is taken, the human check failed), so create() is never called", async () => {
  for (const [status, code] of [
    [409, "email_taken"],
    [403, "turnstile_failed"],
    [403, "bad_invite"],
    [400, "disposable_email"],
    [429, "rate"],
    [403, "passkeys_off"],
  ] as const) {
    const { api, sent } = fakeHub({ status, body: { error: code, message: code } });
    const { credentials, created } = fakeCredentials();
    const { browser, signals } = signalling();
    const error = await passkeySignUp(api, { credentials, browser }, { email: "taken@example.test", turnstile: "tok", lang: "en" }).catch((e: unknown) => e);
    assert.ok(error instanceof HubApiError && error.code === code, code);
    assert.equal(created.length, 0, `${code}: create() never called`);
    assert.equal(sent.length, 1, `${code}: nothing more asked of the hub`);
    assert.equal(signals.length, 0, `${code}: no passkey, nothing to hide`);
  }
});

test("sign-up: options, create() with them as bytes, then the account with the whitelisted credential", async () => {
  const { api, sent } = fakeHub({ body: { challenge: "Y2hhbGxlbmdl", options: CREATE } }, { body: SIGNED });
  const { credentials, created } = fakeCredentials();
  const { browser, signals } = signalling();
  const session = await passkeySignUp(api, { credentials, browser }, { email: "me@example.test", invite: "INVITE-1", turnstile: "tok", lang: "zh-CN" });
  assert.equal(session.token, "login-token");
  assert.equal(created.length, 1);
  same(created[0]!.publicKey.challenge, bytes(1, 2, 3, 0xfb, 0xff));
  same(created[0]!.publicKey.user.id, new Uint8Array(32).fill(7));
  assert.deepEqual(sent[1]!.body, { challenge: "Y2hhbGxlbmdl", email: "me@example.test", invite: "INVITE-1", lang: "zh-CN", credential: EXPECTED_REGISTRATION });
  assert.ok(!JSON.stringify(sent).includes(toBase64Url(SECRET)), "no PRF output reaches the hub");
  assert.equal(signals.length, 0);
});

test("sign-up: the hub refuses the new passkey (lost a race for the e-mail), so the provider is told to hide it", async () => {
  const { api } = fakeHub({ body: { challenge: "AQ", options: CREATE } }, { status: 409, body: { error: "email_taken", message: "taken" } });
  const { credentials } = fakeCredentials();
  const { browser, signals } = signalling();
  const error = await passkeySignUp(api, { credentials, browser }, { email: "me@example.test" }).catch((e: unknown) => e);
  assert.equal(authErrorKind(error), "emailTaken");
  assert.deepEqual(signals, [{ rpId: "hub.test", credentialId: "wP_u" }]);
  // The options named no RP: the hub's config's.
  const noRp = fakeHub({ body: { challenge: "AQ", options: { ...CREATE, rp: { name: "Droppen" } } } }, { status: 401, body: { error: "bad_passkey", message: "no" } });
  const s2 = signalling();
  await passkeySignUp(noRp.api, { credentials, browser: s2.browser }, { email: "me@example.test", rpId: "config.test" }).catch(() => undefined);
  assert.deepEqual(s2.signals, [{ rpId: "config.test", credentialId: "wP_u" }]);
});

test("sign-up: a lost connection or a hub failure after create() hides nothing (the account may exist)", async () => {
  for (const last of [new TypeError("Failed to fetch"), { status: 500, body: { error: "internal", message: "oops" } }, { status: 503, body: { error: "unavailable", message: "later" } }]) {
    const { api } = fakeHub({ body: { challenge: "AQ", options: CREATE } }, last);
    const { credentials } = fakeCredentials();
    const { browser, signals } = signalling();
    await assert.rejects(passkeySignUp(api, { credentials, browser }, { email: "me@example.test" }));
    assert.equal(signals.length, 0);
  }
});

test("sign-up: the person closes the prompt (or create() answers nothing): a cancel, and the hub is not asked to make the account", async () => {
  for (const create of [() => Promise.reject(new DOMException("closed", "NotAllowedError")), () => Promise.resolve(null)]) {
    const { api, sent } = fakeHub({ body: { challenge: "AQ", options: CREATE } });
    const { credentials } = fakeCredentials({ create });
    const error = await passkeySignUp(api, { credentials, browser: null }, { email: "me@example.test" }).catch((e: unknown) => e);
    assert.equal(passkeyCancelled(error), true);
    assert.equal(sent.length, 1);
  }
});

test("sign-in: a modal get() naming no passkey, then the hub; an unknown passkey is signalled, a bad one is not", async () => {
  const ok = fakeHub({ body: { challenge: "Y2g", options: { ...REQUEST, allowCredentials: [] } } }, { body: { ...SIGNED, passkey: { id: "pk-1", label: "x", unlock: "none" } } });
  const { credentials, got } = fakeCredentials();
  const session = await passkeyLogin(ok.api, { credentials, browser: null });
  assert.equal(session.passkey?.id, "pk-1");
  assert.equal(got[0]!.mediation, undefined, "modal");
  assert.deepEqual((got[0]!.publicKey as unknown as { allowCredentials: unknown[] }).allowCredentials, []);
  assert.equal((ok.sent[1]!.body as { challenge: string }).challenge, "Y2g");

  const unknown = fakeHub({ body: { challenge: "AQ", options: REQUEST } }, { status: 401, body: { error: "unknown_passkey", message: "who?" } });
  const s1 = signalling();
  const e1 = await passkeyLogin(unknown.api, { credentials, browser: s1.browser }).catch((e: unknown) => e);
  assert.equal(authErrorKind(e1), "unknownPasskey");
  assert.deepEqual(s1.signals, [{ rpId: "hub.test", credentialId: "wP_u" }]);

  const bad = fakeHub({ body: { challenge: "AQ", options: REQUEST } }, { status: 401, body: { error: "bad_passkey", message: "no" } });
  const s2 = signalling();
  const e2 = await passkeyLogin(bad.api, { credentials, browser: s2.browser }).catch((e: unknown) => e);
  assert.equal(authErrorKind(e2), "badPasskey");
  assert.equal(s2.signals.length, 0);

  // The autofill's answer goes the same way.
  const viaFill = fakeHub({ status: 401, body: { error: "unknown_passkey", message: "who?" } });
  const s3 = signalling();
  await finishPasskeyLogin(viaFill.api, { browser: s3.browser }, { challenge: "AQ", credential: authenticationToJSON(asserted()), rpId: "hub.test" }).catch(() => undefined);
  assert.deepEqual(s3.signals, [{ rpId: "hub.test", credentialId: "wP_u" }]);
});

// ------------------------------------------------------------ the autofill

/** Timers that move only when told. */
function fakeClock() {
  let now = 0;
  let next = 1;
  const pending = new Map<number, { at: number; fn: () => void }>();
  const timers: Timers = {
    setTimeout: (fn, ms) => {
      const id = next++;
      pending.set(id, { at: now + ms, fn });
      return id;
    },
    clearTimeout: (id) => void pending.delete(id as number),
    now: () => now,
  };
  return {
    timers,
    async advance(ms: number) {
      now += ms;
      for (const [id, t] of [...pending].sort((a, b) => a[1].at - b[1].at)) {
        if (t.at > now || !pending.has(id)) continue;
        pending.delete(id);
        t.fn();
      }
      await flush();
    },
  };
}

/** A browser's `get()`: each request pending until answered, rejected when its signal aborts; at most one pending at a time. */
function pendingCredentials({ ignoreAbort = false }: { ignoreAbort?: boolean } = {}) {
  const calls: { options: { mediation?: string; signal?: AbortSignal; publicKey: PublicKeyCredentialRequestOptions }; resolve: (v: unknown) => void; reject: (e: unknown) => void; settled: boolean }[] = [];
  const overlaps: number[] = [];
  const credentials: CredentialsLike = {
    create: async () => null,
    get: (options) =>
      new Promise((resolve, reject) => {
        const open = calls.filter((c) => !c.settled).length;
        if (open > 0) overlaps.push(calls.length);
        const call = {
          options,
          settled: false,
          resolve: (v: unknown) => {
            call.settled = true;
            resolve(v);
          },
          reject: (e: unknown) => {
            call.settled = true;
            reject(e);
          },
        };
        calls.push(call);
        if (!ignoreAbort) options.signal?.addEventListener("abort", () => call.reject(options.signal!.reason ?? new DOMException("aborted", "AbortError")), { once: true });
      }),
  };
  return { credentials, calls, overlaps };
}

/** Sign-in options with a fresh challenge each time. */
function freshOptions() {
  let n = 0;
  const asked: string[] = [];
  const options = async () => {
    n++;
    const challenge = toBase64Url(bytes(n));
    asked.push(challenge);
    return { challenge, options: { ...REQUEST, challenge } };
  };
  return { options, asked };
}

test("autofill: a conditional get(), withdrawn and asked again with a fresh challenge before the 2-minute one runs out", async () => {
  assert.ok(AUTOFILL_REFRESH_MS < 120_000 && AUTOFILL_REFRESH_MS >= 60_000);
  const clock = fakeClock();
  const { credentials, calls } = pendingCredentials();
  const { options, asked } = freshOptions();
  const stop = new AbortController();
  const result = conditionalPasskeyGet({ options, credentials, browser: null, signal: stop.signal, timers: clock.timers });
  await flush();
  assert.equal(calls.length, 1);
  assert.equal(calls[0]!.options.mediation, "conditional");
  same(calls[0]!.options.publicKey.challenge, bytes(1));
  await clock.advance(AUTOFILL_REFRESH_MS - 1);
  assert.equal(calls.length, 1, "not yet");
  assert.equal(calls[0]!.options.signal!.aborted, false);
  await clock.advance(1);
  assert.equal(calls[0]!.options.signal!.aborted, true, "withdrawn in time");
  assert.equal(calls.length, 2);
  same(calls[1]!.options.publicKey.challenge, bytes(2));
  await clock.advance(AUTOFILL_REFRESH_MS);
  assert.equal(calls.length, 3);
  // The person picks a passkey: the answer, with the challenge it answered.
  calls[2]!.resolve(asserted());
  const answer = await result;
  assert.equal(answer?.challenge, asked[2]);
  assert.equal(answer?.rpId, "hub.test");
  assert.equal(answer?.credential.id, "wP_u");
  assert.deepEqual(answer?.credential.clientExtensionResults, {});
});

test("autofill: its signal stops it at once, wherever it is, and nothing more is asked", async () => {
  const clock = fakeClock();
  const { credentials, calls } = pendingCredentials();
  const { options, asked } = freshOptions();
  const stop = new AbortController();
  const result = conditionalPasskeyGet({ options, credentials, browser: null, signal: stop.signal, timers: clock.timers });
  await flush();
  stop.abort();
  assert.equal(await result, null);
  assert.equal(calls[0]!.options.signal!.aborted, true);
  await clock.advance(10 * AUTOFILL_REFRESH_MS);
  assert.equal(asked.length, 1);
  assert.equal(calls.length, 1);
  // Aborted while the options were on their way: no get() at all.
  const early = new AbortController();
  let release!: () => void;
  const slow = conditionalPasskeyGet({
    options: () => new Promise((r) => (release = () => r({ challenge: "AQ", options: REQUEST }))),
    credentials,
    browser: null,
    signal: early.signal,
    timers: clock.timers,
  });
  early.abort();
  release();
  assert.equal(await slow, null);
  assert.equal(calls.length, 1);
});

test("autofill: a closed prompt is offered again; a browser that refuses at once again and again, or options that fail, end it", async () => {
  const clock = fakeClock();
  const { credentials, calls } = pendingCredentials();
  const { options } = freshOptions();
  const stop = new AbortController();
  // Caught at once: it rejects while the browser is still being driven below.
  const result = conditionalPasskeyGet({ options, credentials, browser: null, signal: stop.signal, timers: clock.timers }).then(
    () => null,
    (e: unknown) => e,
  );
  await flush();
  await clock.advance(30_000);
  calls[0]!.reject(new DOMException("closed", "NotAllowedError"));
  await flush();
  assert.equal(calls.length, 2, "offered again");
  for (let i = 1; i <= 3; i++) {
    calls[i]!.reject(new DOMException("refused", "NotAllowedError"));
    await flush();
  }
  const error = await result;
  assert.equal((error as DOMException).name, "NotAllowedError");
  assert.equal(calls.length, 4, "three quick refusals, then no more");

  // Another error from the browser: it ends there.
  const other = pendingCredentials();
  const r2 = conditionalPasskeyGet({ options, credentials: other.credentials, browser: null, signal: new AbortController().signal, timers: clock.timers });
  await flush();
  other.calls[0]!.reject(new DOMException("bad rp", "SecurityError"));
  assert.equal(((await r2.catch((e: unknown) => e)) as DOMException).name, "SecurityError");

  // The hub's options fail: no get(), the refusal comes back.
  const third = pendingCredentials();
  const r3 = conditionalPasskeyGet({
    options: () => Promise.reject(new HubApiError(429, "rate", "slow down")),
    credentials: third.credentials,
    browser: null,
    signal: new AbortController().signal,
    timers: clock.timers,
  });
  assert.equal(authErrorKind(await r3.catch((e: unknown) => e)), "rate");
  assert.equal(third.calls.length, 0);
});

test("a modal sign-in withdraws the autofill's request first (one at a time), and offers it again only if it did not sign in", async () => {
  const clock = fakeClock();
  const browser = pendingCredentials();
  const { options } = freshOptions();
  const answers: unknown[] = [];
  const fill = new PasskeyAutofill({
    options,
    credentials: browser.credentials,
    browser: null,
    timers: clock.timers,
    onAnswer: async (a) => {
      answers.push(a);
      return false;
    },
  });
  fill.start();
  await flush();
  assert.equal(fill.active, true);
  assert.equal(browser.calls.length, 1);
  assert.equal(browser.calls[0]!.options.mediation, "conditional");

  // The button: the autofill's request withdrawn before the modal one, which the person closes.
  const hub = fakeHub({ body: { challenge: "AQ", options: REQUEST } }, { body: { challenge: "Ag", options: REQUEST } }, { body: SIGNED });
  const modal = passkeyLogin(hub.api, { credentials: browser.credentials, browser: null }, { autofill: fill });
  await flush();
  assert.equal(browser.calls[0]!.settled, true, "withdrawn");
  assert.equal(browser.calls.length, 2);
  assert.equal(browser.calls[1]!.options.mediation, undefined, "the modal one");
  browser.calls[1]!.reject(new DOMException("closed", "NotAllowedError"));
  assert.equal(passkeyCancelled(await modal.catch((e: unknown) => e)), true);
  await flush();
  assert.equal(browser.calls.length, 3, "the autofill offered again");
  assert.equal(browser.calls[2]!.options.mediation, "conditional");

  // The button again, and this time it signs in: the autofill stays withdrawn.
  const signed = passkeyLogin(hub.api, { credentials: browser.credentials, browser: null }, { autofill: fill });
  await flush();
  assert.equal(browser.calls[2]!.settled, true);
  browser.calls[3]!.resolve(asserted());
  assert.equal((await signed).token, "login-token");
  await flush();
  assert.equal(browser.calls.length, 4);
  assert.equal(fill.active, false);
  assert.deepEqual(browser.overlaps, [], "never two requests pending at once");
  assert.equal(answers.length, 0);
});

test("the autofill: a refused answer is offered again, a signed-in one ends it, and closing it withdraws it for good", async () => {
  const clock = fakeClock();
  const browser = pendingCredentials();
  const { options } = freshOptions();
  const verdicts = [false, true];
  const answers: string[] = [];
  const fill = new PasskeyAutofill({
    options,
    credentials: browser.credentials,
    browser: null,
    timers: clock.timers,
    onAnswer: async (a) => {
      answers.push(a.challenge);
      return verdicts.shift()!;
    },
  });
  fill.start();
  fill.start();
  await flush();
  assert.equal(browser.calls.length, 1, "started once");
  browser.calls[0]!.resolve(asserted());
  await flush();
  assert.equal(browser.calls.length, 2, "refused by the hub: offered again");
  browser.calls[1]!.resolve(asserted());
  await flush();
  assert.equal(answers.length, 2);
  assert.equal(fill.active, false, "signed in: it ends");
  assert.equal(browser.calls.length, 2);

  // Closed: withdrawn, and never started again.
  const other = pendingCredentials();
  const closed = new PasskeyAutofill({ options, credentials: other.credentials, browser: null, timers: clock.timers, onAnswer: async () => false });
  closed.start();
  await flush();
  closed.close();
  await flush();
  assert.equal(other.calls[0]!.settled, true);
  closed.start();
  await flush();
  assert.equal(other.calls.length, 1);

  // It stops for good on a failure, and says so.
  const errors: unknown[] = [];
  const failing = new PasskeyAutofill({
    options: () => Promise.reject(new HubApiError(403, "passkeys_off", "off")),
    credentials: other.credentials,
    browser: null,
    timers: clock.timers,
    onAnswer: async () => false,
    onError: (e) => errors.push(e),
  });
  failing.start();
  await flush();
  assert.equal(authErrorKind(errors[0]), "passkeysOff");
  assert.equal(failing.active, false);
});

test("the autofill's withdrawal waits for the browser to let go, but not for ever", async () => {
  const clock = fakeClock();
  const stuck = pendingCredentials({ ignoreAbort: true });
  const { options } = freshOptions();
  const fill = new PasskeyAutofill({ options, credentials: stuck.credentials, browser: null, timers: clock.timers, onAnswer: async () => true });
  fill.start();
  await flush();
  let stopped = false;
  void fill.stop().then(() => (stopped = true));
  await flush();
  assert.equal(stopped, false, "waiting for the browser");
  await clock.advance(1_000);
  assert.equal(stopped, true, "gave up waiting");
  assert.equal(fill.active, false);
  fill.start();
  await flush();
  assert.equal(stuck.calls.length, 2, "a new one may start");
});

// ------------------------------------------------------------ the form

const CONFIG: SignupConfig = { signup: "open", inviteRequired: false, turnstile: null, verification: "required", passkeys: { rpId: "hub.test" } };

test("the form: a passkey first only where both the browser and the hub can; the password otherwise, or when asked for", () => {
  assert.equal(authMethod(true, CONFIG, false), "passkey");
  assert.equal(authMethod(true, CONFIG, true), "password");
  assert.equal(authMethod(false, CONFIG, false), "password", "no WebAuthn: the password form");
  assert.equal(authMethod(true, { ...CONFIG, passkeys: null }, false), "password", "a hub without passkeys: the password form");
  assert.equal(authMethod(true, "failed", false), "password", "the hub did not say: the password form");
  assert.equal(authMethod(true, null, false), "pending", "not known yet");
  assert.equal(authMethod(false, null, false), "password", "no WebAuthn: the password form at once");
});

test("a browser without WebAuthn gets the password form as it was, with no word of passkeys", () => {
  const draw = (mode: "signin" | "signup") =>
    renderToStaticMarkup(
      <I18nextProvider i18n={i18next}>
        <AuthScreen api={{} as never} onSignedIn={() => undefined} initialMode={mode} />
      </I18nextProvider>,
    );
  for (const mode of ["signin", "signup"] as const) {
    const html = draw(mode);
    assert.doesNotMatch(html, /passkey/i, mode);
    assert.doesNotMatch(html, /webauthn/, mode);
    assert.match(html, /data-testid="auth-email"/);
    assert.match(html, /data-testid="auth-password"/);
    assert.match(html, /data-testid="password-submit"/);
    assert.match(html, /autoComplete="username"|autocomplete="username"/);
  }
});

test("a browser with WebAuthn gets the passkey form at once: the answers are written in the page", () => {
  const g = globalThis as unknown as Record<string, unknown>;
  const before = { pkc: g.PublicKeyCredential, nav: Object.getOwnPropertyDescriptor(globalThis, "navigator") };
  g.PublicKeyCredential = function PublicKeyCredential() {};
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { credentials: creds, language: "en" } });
  try {
    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18next}>
        <AuthScreen api={{} as never} onSignedIn={() => undefined} />
      </I18nextProvider>,
    );
    assert.doesNotMatch(html, /data-testid="auth-password"/);
    assert.match(html, /data-testid="passkey-signin"/);
    assert.ok(html.toLowerCase().includes("passkey"));
  } finally {
    if (before.pkc === undefined) delete g.PublicKeyCredential;
    else g.PublicKeyCredential = before.pkc;
    if (before.nav) Object.defineProperty(globalThis, "navigator", before.nav);
  }
});
