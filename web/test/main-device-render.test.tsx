// The main device is the account's (`docs/local-aggregator.md` §6.5, kept on
// the hub: `docs/hub-api/http.md` `/v1/main-device`), drawn for real: React
// renders into a real DOM (jsdom) a tree keyed by the home as
// `app/root-layout.tsx` is, under a real `HubShell` told by a stand-in link
// what the hub says. Pinned: what the page shows before the hub answers (this
// browser's copy), the account's arriving, a change from another device
// starting the tree over once and only on a real change, and the question
// 「设为主设备」 asks in each language.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { makeI18n } from "./render.js";
import { type FakeNode, installLiveDom, waitFor } from "./live-dom.js";

const place = { pathname: "/sessions", search: "", reload: () => undefined };
Object.assign(globalThis, {
  location: place,
  history: {
    state: null,
    replaceState(_state: unknown, _title: string, url: string) {
      const to = new URL(url, "https://droppen.test");
      place.pathname = to.pathname;
      place.search = to.search;
    },
  },
});
const container = installLiveDom();
const flush = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));
/** Wait until `done()` holds (React renders when it gets to it: a loaded machine takes longer), or fail. */

const { createRoot } = await import("react-dom/client");
const { QueryClient } = await import("@tanstack/react-query");
const { I18nextProvider } = await import("react-i18next");
const { HubShell } = await import("../src/features/cloud/hub-shell.js");
const { MainDeviceConfirm } = await import("../src/features/cloud/cloud-bar.js");
const { MachineProvider, useHomeMachine, useMachine } = await import("../src/lib/machines/context.js");
const { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } = await import("../src/i18n/cross-device.js");
type AccountMain = import("../src/lib/hub/api.js").AccountMain;

function mount(node: React.ReactNode) {
  const errors: unknown[] = [];
  const root = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  root.render(node);
  return { root, errors, text: () => (container as FakeNode).textContent };
}

/** The hub link as the shell uses it: control messages and its state, told by the test. */
class StandInLink {
  state = "open" as const;
  presence = new Map<string, { state: string }>();
  carriesChannels = true;
  private controls = new Set<(m: Record<string, unknown>) => void>();
  onPresence() {
    return () => undefined;
  }
  onState() {
    return () => undefined;
  }
  onControl(fn: (m: Record<string, unknown>) => void) {
    this.controls.add(fn);
    return () => void this.controls.delete(fn);
  }
  control(message: Record<string, unknown>) {
    for (const fn of [...this.controls]) fn(message);
  }
}

/** A page of account a-1 whose chain has `hosts`, its hub's `/v1/main-device` answering `answer`. */
function page(hosts: string[], answer: (machine: string, ifUnset: boolean) => Promise<AccountMain>) {
  const link = new StandInLink();
  const writes: { machine: string; ifUnset: boolean }[] = [];
  const account = {
    account: "a-1",
    deviceId: "d-1",
    isAdmin: true,
    session: { token: "t" },
    chain: { hosts: () => hosts.map((id) => ({ info: { id }, status: { type: "active" } })), latestMachineEpoch: () => null },
    machine: () => undefined,
    machineKeys: async () => new Map(),
    refreshKeys: async () => undefined,
    onChange: () => () => undefined,
  };
  const api = {
    setMainDevice: (_token: string, machine: string, opts: { ifUnset?: boolean } = {}) => {
      writes.push({ machine, ifUnset: opts.ifUnset === true });
      return answer(machine, opts.ifUnset === true);
    },
  };
  const shell = new HubShell({
    api: api as never,
    account: account as never,
    link: link as never,
    relays: { online: () => false, onChange: () => () => undefined } as never,
    store: {} as never,
    queryClient: new QueryClient(),
  });
  return { link, shell, writes };
}

/** What `app/root-layout.tsx` does with the home: a tree keyed by it. Counts the trees it started. */
function Keyed({ trees }: { trees: string[] }) {
  const home = useHomeMachine();
  return (
    <MachineProvider key={home} machine={home}>
      <Tree trees={trees} />
    </MachineProvider>
  );
}

function Tree({ trees }: { trees: string[] }) {
  const machine = useMachine();
  React.useEffect(() => void trees.push(machine), []);
  return <span data-tree={machine}>{`screens of ${machine}`}</span>;
}

test("the page draws this browser's copy until the hub says the account's main device, then the account's, and starts over only on a real change", async (t) => {
  localStorage.setItem("rh.hub.home.a-1", "m-a");
  place.pathname = "/terminals";
  const { link, shell, writes } = page(["m-a", "m-b"], async (machine) => ({ machine, setMs: 1 }));
  shell.start();
  t.after(() => shell.stop());
  const trees: string[] = [];
  const { root, errors, text } = mount(<Keyed trees={trees} />);
  t.after(() => root.unmount());
  await waitFor(() => text() !== "");
  assert.deepEqual(errors, []);
  assert.equal(text(), "screens of m-a", "before the hub answers: this browser's copy");
  // Signed in on the link: the account's is m-b (chosen on another device).
  link.control({ t: "main_device", machine: "m-b", set_ms: 10 });
  await waitFor(() => text() === "screens of m-b");
  assert.deepEqual(trees, ["m-a", "m-b"], "one new tree");
  assert.equal(place.pathname, "/m/m-a/terminals", "the page in view kept: m-a's terminals, under m-a's own address now");
  assert.equal(localStorage.getItem("rh.hub.home.a-1"), "m-b", "the browser's copy follows");
  // The same again (the next connection's snapshot), and an older word: nothing starts over.
  link.control({ t: "main_device", machine: "m-b", set_ms: 10 });
  link.control({ t: "main_device", machine: "m-a", set_ms: 4 });
  await flush();
  assert.deepEqual(trees, ["m-a", "m-b"]);
  // Another device of the account moves it back: followed.
  link.control({ t: "main_device", machine: "m-a", set_ms: 12 });
  await waitFor(() => text() === "screens of m-a");
  assert.deepEqual(trees, ["m-a", "m-b", "m-a"]);
  assert.equal(place.pathname, "/terminals");
  assert.deepEqual(writes, [], "the hub's word is not written back");
});

test("upgrading: the hub has none, so this browser's main device is offered for the account; the one that stands is drawn", async (t) => {
  localStorage.setItem("rh.hub.home.a-1", "m-b");
  place.pathname = "/sessions";
  // Another browser of the account was first, with m-c.
  const { link, shell, writes } = page(["m-a", "m-b", "m-c"], async () => ({ machine: "m-c", setMs: 3 }));
  shell.start();
  t.after(() => shell.stop());
  const trees: string[] = [];
  const { root, text } = mount(<Keyed trees={trees} />);
  t.after(() => root.unmount());
  await waitFor(() => text() !== "");
  assert.equal(text(), "screens of m-b");
  link.control({ t: "main_device", machine: null, set_ms: null });
  await waitFor(() => text() === "screens of m-c");
  assert.deepEqual(writes, [{ machine: "m-b", ifUnset: true }]);
  assert.deepEqual(trees, ["m-b", "m-c"]);
});

test("offline or an older hub: no word from it, and the page keeps this browser's main device", async (t) => {
  localStorage.setItem("rh.hub.home.a-1", "m-b");
  const { shell, writes } = page(["m-a", "m-b"], async () => {
    throw new TypeError("Failed to fetch");
  });
  shell.start();
  t.after(() => shell.stop());
  const trees: string[] = [];
  const { root, text } = mount(<Keyed trees={trees} />);
  t.after(() => root.unmount());
  await waitFor(() => text() !== "");
  await flush(60);
  assert.equal(text(), "screens of m-b");
  assert.deepEqual(trees, ["m-b"]);
  assert.deepEqual(writes, [], "nothing to seed without the hub's word");
});

test("「设为主设备」 says the change is made on all of the person's devices, in every language, and never says machine or computer", async (t) => {
  const i18n = makeI18n({
    lng: "en",
    resources: {
      en: { translation: {} },
      "zh-CN": { translation: crossDeviceZhCN },
      "zh-TW": { translation: crossDeviceZhTW },
      ja: { translation: crossDeviceJa }
    },
    keySeparator: false,
    nsSeparator: false
  });
  const said: Record<string, string> = {};
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    await i18n.changeLanguage(lng);
    const { root, text } = mount(
      <I18nextProvider i18n={i18n}>
        <MainDeviceConfirm next="studio" old="laptop" onConfirm={() => undefined} onCancel={() => undefined} />
      </I18nextProvider>,
    );
    await waitFor(() => text() !== "");
    said[lng] = text();
    root.unmount();
  }
  t.diagnostic(JSON.stringify(said));
  assert.match(said.en!, /same on all your devices/);
  assert.match(said["zh-CN"]!, /所有设备/);
  assert.match(said["zh-TW"]!, /所有裝置/);
  assert.match(said.ja!, /すべてのデバイス/);
  for (const [lng, text] of Object.entries(said)) {
    assert.ok(text.includes("studio") && text.includes("laptop"), `${lng}: both names`);
    assert.doesNotMatch(text, /machine|computer|机器|電腦|电脑|機器|マシン|コンピュータ|端末/i, lng);
  }
});
