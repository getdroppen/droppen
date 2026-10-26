// The public pages and sign-up (`features/landing/signup-mode.ts`), drawn for
// real in a DOM where effects run.
//
// What these pin: sign-up is open, written in `SIGNUP_CONFIG`, so
// `useSignupMode` says open without asking anyone and a mode the caller gives
// wins; the front pages (`front-pages.tsx`) draw the open wording, and a page
// that says nothing of sign-up is the same; and the waitlist form sends what
// the page shows: Pro whatever the address says where sign-up is open, the
// plan chosen where an explicit mode says otherwise.
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

/** A hub that keeps what joins its waitlist (sign-up's answers are not its to give). */
function hub() {
  const joined: { email: string; plan: string; locale?: string }[] = [];
  const api = { joinWaitlist: async (input: { email: string; plan: string; locale?: string }) => void joined.push(input) };
  return { api, joined };
}

function Probe({ given }: { given?: SignupMode }) {
  return <span data-testid="mode">{useSignupMode(given)}</span>;
}

test("useSignupMode: open, as written in the page; a given mode wins", async () => {
  await draw(<Probe />);
  assert.equal(byTestId("mode")?.textContent, "open", "sign-up is open, nothing to ask");

  await draw(<Probe given="closed" />);
  assert.equal(byTestId("mode")?.textContent, "closed", "a given mode wins");
});

test("the front pages draw the open sign-up: Free signs up, Pro's waitlist stays", async () => {
  history.replaceState(null, "", "/pricing");
  const open = hub();
  await draw(<FrontPageView page="pricing" signedIn={false} api={open.api as never} />);
  await until(() => byTestId("plan-free-signup") !== null);
  assert.equal(byTestId("plan-free-signup")?.getAttribute("data-variant"), "primary");
  assert.equal(byTestId("plan-free-waitlist"), null);
  assert.ok(words().includes("Free, with an account"));
});

test("a page that says nothing of sign-up is drawn the same, and nothing is asked", async () => {
  history.replaceState(null, "", "/privacy");
  const quiet = hub();
  await draw(<FrontPageView page="privacy" signedIn={false} api={quiet.api as never} />);
  await until(() => words().includes("If you joined the waitlist"));
  assert.deepEqual(quiet.joined, []);
});

/** Type `value` into `input` as a person does (React reads the native setter's value on `input`). */
function type(input: HTMLInputElement, value: string): void {
  Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!.call(input, value);
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
}

async function joinWith(signup: SignupMode, search: string) {
  history.replaceState(null, "", `/pricing${search}`);
  const { api, joined } = hub();
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
