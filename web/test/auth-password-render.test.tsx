// The sign-up form's password (`features/cloud/cloud-app.tsx` `AuthScreen`),
// drawn for real: one shorter than the hub takes (rh-hub `auth::MIN_PASSWORD`,
// 10) is said in words on the form, in the reader's language, and never sent
// — the hub would refuse it with a bare 400. One long enough is sent. And its
// invite code: sign-up is open, as `SIGNUP_CONFIG` says, so the code is
// optional (it may still give a plan) and behind a link — opened for the
// person by a refused invite too.
//
// React renders into a real DOM (jsdom) where effects run.
// `import * as React` because the test directory is outside the tsconfig's
// `include`.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { type FakeNode, find, installLiveDom, waitFor } from "./live-dom.js";

Object.assign(globalThis, { location: { pathname: "/", search: "", hash: "", reload: () => undefined, replace: () => undefined } });
const container = installLiveDom();

const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("./i18n-all.js");
const { AuthScreen } = await import("../src/features/cloud/cloud-app.js");

const noop = () => undefined;
const flush = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));


/** The handlers React gave `node` (`onChange`, `onSubmit`, …). */
function props(node: FakeNode | null): Record<string, (e: unknown) => void> {
  assert.ok(node);
  return Object.entries(node as unknown as Record<string, unknown>).find(([k]) => k.startsWith("__reactProps$"))?.[1] as Record<string, (e: unknown) => void>;
}

const byTestId = (id: string) => find(container, (n) => n.getAttribute?.("data-testid") === id);
const type = (id: string, value: string) => props(byTestId(id)).onChange({ target: { value } });
const submit = () => props(find(container, (n) => n.nodeName === "FORM")).onSubmit({ preventDefault: noop });

test("sign-up: a password under 10 characters is said on the form and never sent; one of 10 is", async () => {
  const sent: { email: string; password: string }[] = [];
  const api = {
    signUp: async (email: string, password: string) => {
      sent.push({ email, password });
      return new Promise(() => undefined); // left waiting: what was sent is what this pins
    },
  };
  await i18n.changeLanguage("zh-CN");
  const root = createRoot(container as never);
  root.render(
    <I18nextProvider i18n={i18n}>
      <AuthScreen api={api as never} onSignedIn={noop} initialMode="signup" />
    </I18nextProvider>,
  );
  try {
    await waitFor(() => byTestId("auth-password") !== null && byTestId("password-submit") !== null);
    type("auth-email", "me@example.test");
    type("auth-password", "short-pw1");
    await flush();
    submit();
    await waitFor(() => (container as FakeNode).textContent.includes("密码至少需要 10 个字符"));
    assert.deepEqual(sent, [], "not sent: the hub would refuse it");
    assert.doesNotMatch((container as FakeNode).textContent, /邮箱地址/, "the e-mail is not blamed");
    // Characters, not bytes: four of them are twelve bytes, and still too few.
    type("auth-password", "密码太短");
    await flush();
    submit();
    await flush();
    assert.deepEqual(sent, []);
    type("auth-password", "long-enough");
    await flush();
    submit();
    await waitFor(() => sent.length === 1);
    assert.deepEqual(sent, [{ email: "me@example.test", password: "long-enough" }]);
  } finally {
    root.unmount();
    await i18n.changeLanguage("en");
  }
});

test("sign-up: the optional invite code waits behind a link; a refused invite opens the field", async () => {
  const { HubApiError } = await import("../src/lib/hub/api.js");
  const sent: (string | undefined)[] = [];
  const api = {
    signUp: async (_email: string, _password: string, opts: { invite?: string }) => {
      sent.push(opts.invite);
      throw new HubApiError(403, "bad_invite", "that invite is unknown, used up or expired");
    },
  };
  await i18n.changeLanguage("en");
  const words = () => (container as FakeNode).textContent;
  const open = async () => {
    const root = createRoot(container as never);
    root.render(
      <I18nextProvider i18n={i18n}>
        <AuthScreen api={api as never} onSignedIn={noop} initialMode="signup" />
      </I18nextProvider>,
    );
    await waitFor(() => byTestId("password-submit") !== null);
    return root;
  };
  try {
    // Open: no field until the person asks for it.
    let root = await open();
    try {
      await waitFor(() => byTestId("show-invite") !== null);
      assert.equal(byTestId("auth-invite"), null);
      assert.ok(words().includes("have an invite code?"));
      props(byTestId("show-invite")).onClick({});
      await waitFor(() => byTestId("auth-invite") !== null);
      assert.equal(byTestId("show-invite"), null);
      assert.ok(words().includes("invite code (optional)"));
      type("auth-invite", "INVITE-1");
      type("auth-email", "me@example.test");
      type("auth-password", "long-enough");
      await flush();
      submit();
      await waitFor(() => sent.length === 1);
      assert.deepEqual(sent, ["INVITE-1"], "an invite given in open mode is sent");
    } finally {
      root.unmount();
    }
    // A refused invite opens the field to try another; the code stays optional.
    root = await open();
    try {
      await waitFor(() => byTestId("show-invite") !== null);
      type("auth-email", "me@example.test");
      type("auth-password", "long-enough");
      await flush();
      submit();
      await waitFor(() => byTestId("auth-invite") !== null);
      assert.deepEqual(sent, ["INVITE-1", undefined], "none was given");
      assert.ok(words().includes("that invite code is unknown, used up or expired"));
      assert.ok(words().includes("invite code (optional)"));
      assert.equal(byTestId("show-invite"), null);
      assert.ok(!byTestId("password-submit")?.hasAttribute("disabled"), "open sign-up needs no code");
    } finally {
      root.unmount();
    }
  } finally {
    await i18n.changeLanguage("en");
  }
});
