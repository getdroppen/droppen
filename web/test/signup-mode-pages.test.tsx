// The public pages and the hub's sign-up mode (`features/landing/signup-mode.ts`),
// drawn for real in a DOM where effects run.
//
// What these pin: `useSignupMode` is open until the hub answers, then the
// hub's mode, and a mode the caller gives wins without asking; the front
// pages (`front-pages.tsx`) ask the hub only where a page says how to sign up,
// and hand the answer on — the pricing page draws open, invite and closed
// sign-up each its own way; and the waitlist form sends what the page shows:
// Pro whatever the address says where sign-up is open, the plan chosen where
// it is not.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document, container } = installJsdom();
// The pages follow `popstate` (`route.ts`): the window's listeners, as in a browser.
Object.assign(globalThis, { addEventListener: window.addEventListener.bind(window), removeEventListener: window.removeEventListener.bind(window) });
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("../src/i18n/index.js");
const { useSignupMode } = await import("../src/features/landing/signup-mode.js");
const { FrontPageView } = await import("../src/features/landing/front-pages.js");
const { default: PricingPage } = await import("../src/features/landing/pricing.js");
type SignupMode = import("../src/lib/hub/api.js").SignupMode;
type HubApi = import("../src/lib/hub/api.js").HubApi;

await i18n.changeLanguage("en");

let root: ReturnType<typeof createRoot> | null = null;

async function unmount(): Promise<void> {
  const was = root;
  root = null;
  if (was) await act(async () => was.unmount());
  container.innerHTML = "";
}
afterEach(unmount);

/** Draw `node` afresh (whatever was drawn before goes first). */
async function draw(node: React.ReactNode): Promise<void> {
  await unmount();
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

/** Until `done`, letting React and the lazy chunks run. */
async function until(done: () => boolean, ms = 10_000): Promise<void> {
  const end = Date.now() + ms;
  while (!done()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await act(async () => void (await new Promise((r) => setTimeout(r, 10))));
  }
}

const byTestId = (id: string) => container.querySelector(`[data-testid="${id}"]`);
const words = () => container.textContent ?? "";

/** A hub that says `mode` (or cannot be asked) and keeps what joins its waitlist. */
function hub(mode: SignupMode | Error) {
  const joined: { email: string; plan: string; locale?: string }[] = [];
  let asked = 0;
  const api = {
    signupConfig: async () => {
      asked++;
      if (mode instanceof Error) throw mode;
      return { signup: mode, inviteRequired: mode === "invite", turnstile: null, verification: "required" as const, passkeys: null };
    },
    joinWaitlist: async (input: { email: string; plan: string; locale?: string }) => void joined.push(input),
  };
  return { api, joined, asked: () => asked };
}

function Probe({ api, given }: { api?: Pick<HubApi, "signupConfig">; given?: SignupMode }) {
  return <span data-testid="mode">{useSignupMode(api, given)}</span>;
}

test("useSignupMode: open until the hub answers, then its mode; a given mode wins and asks nothing", async () => {
  const invite = hub("invite");
  await draw(<Probe api={invite.api} />);
  await until(() => byTestId("mode")?.textContent === "invite");
  assert.equal(invite.asked(), 1);

  const given = hub("invite");
  await draw(<Probe api={given.api} given="closed" />);
  await until(() => byTestId("mode")?.textContent === "closed");
  assert.equal(given.asked(), 0, "a given mode asks nothing");

  await draw(<Probe />);
  assert.equal(byTestId("mode")?.textContent, "open", "no hub to ask: open");

  const failing = hub(new TypeError("Failed to fetch"));
  await draw(<Probe api={failing.api} />);
  await until(() => failing.asked() === 1);
  await act(async () => void (await new Promise((r) => setTimeout(r, 10))));
  assert.equal(byTestId("mode")?.textContent, "open", "a hub that cannot be asked: open");
});

test("the front pages hand the hub's answer on: the pricing page draws open, invite and closed sign-up each its own way", async () => {
  history.replaceState(null, "", "/pricing");
  const open = hub("open");
  await draw(<FrontPageView page="pricing" signedIn={false} api={open.api as never} />);
  await until(() => byTestId("plan-free-signup") !== null && open.asked() === 1);
  assert.equal(byTestId("plan-free-signup")?.getAttribute("data-variant"), "primary");
  assert.equal(byTestId("plan-free-waitlist"), null);
  assert.ok(words().includes("Free, with an account"));

  const invite = hub("invite");
  await draw(<FrontPageView page="pricing" signedIn={false} api={invite.api as never} />);
  await until(() => byTestId("plan-free-waitlist") !== null);
  assert.ok(words().includes("Have an invite code? Sign up"));
  assert.ok(words().includes("Free; sign-up by invite for now"));

  const closed = hub("closed");
  await draw(<FrontPageView page="pricing" signedIn={false} api={closed.api as never} />);
  await until(() => byTestId("plan-free-waitlist") !== null);
  assert.ok(words().includes("Sign-up is closed for now"));
  assert.ok(words().includes("Free; sign-up is closed for now"));
  assert.equal(byTestId("plan-free-signup"), null, "closed: no way to sign up, an invite code neither");
  assert.doesNotMatch(words(), /invite/i);
});

test("a page that says nothing of sign-up asks the hub nothing", async () => {
  history.replaceState(null, "", "/privacy");
  const quiet = hub("invite");
  await draw(<FrontPageView page="privacy" signedIn={false} api={quiet.api as never} />);
  await until(() => words().includes("If you joined the waitlist"));
  assert.equal(quiet.asked(), 0);
});

/** Type `value` into `input` as a person does (React reads the native setter's value on `input`). */
function type(input: HTMLInputElement, value: string): void {
  Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!.call(input, value);
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
}

async function joinWith(signup: SignupMode, search: string) {
  history.replaceState(null, "", `/pricing${search}`);
  const { api, joined } = hub(signup);
  await draw(<PricingPage api={api} signup={signup} />);
  const email = document.getElementById("waitlist-email") as HTMLInputElement;
  await act(async () => type(email, "ada@example.com"));
  await act(async () => void byTestId("waitlist-form")!.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })));
  await until(() => joined.length === 1 && byTestId("waitlist-done") !== null);
  return joined[0]!;
}

test("the waitlist form joins Pro where sign-up is open, whatever the address says; the plan chosen where it is not", async () => {
  assert.deepEqual(await joinWith("open", "?plan=free"), { email: "ada@example.com", plan: "pro", locale: "en" });
  assert.ok(words().includes("You're on the list: we'll e-mail you when Pro opens."));
  assert.equal((await joinWith("invite", "?plan=free")).plan, "free");
  assert.ok(words().includes("You're on the list: we'll e-mail you an invite."));
  assert.equal((await joinWith("closed", "")).plan, "free");
  assert.ok(words().includes("You're on the list: we'll e-mail you when sign-up opens."));
  assert.equal((await joinWith("closed", "?plan=pro")).plan, "pro");
});
