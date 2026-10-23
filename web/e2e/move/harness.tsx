// The pages of the two-host move run (`run.mjs`): one bundle of the real
// `lib/move` and `/move` page, served on both hosts and on a third, foreign
// site by `server.ts`. Each page picks what it is by its host and path:
//
//   legacy  /seed   makes a device the way the app does (key store, notes,
//                   seen record, session, push record) and registers it
//   legacy  /       "Move this browser": startMove (push)
//   app     /       "Bring it over": the sign-in screen's real offer
//                   (BringItOver), and a bare startPull button (#pull)
//   app     /watch  a second tab of the app host: counts its loads, and
//                   reloads when a move arrives (reloadOnArrival)
//   both    /move   the real MovePage
//   both    /check  what the origin holds now, read on a fresh page; on the
//                   app host also a fresh sign-in with the stored keys
//   evil    /steal  opens legacy /move and asks it for the keys
//   evil    /plant  opens app /move and hands it keys of its own
//
// `?block=1` stands in for a popup blocker (window.open answers null);
// `?target=<origin>` on legacy / (or `?from=<origin>` on app /) asks the move
// for another counterpart than the pinned one (it must refuse).
//   legacy  /remove  the move notice's "Remove this browser"
//                    (prepareRemoval + removeHere, as the notice runs them)
//
// The pinned pair (the hub's origin and its app host) is set as
// `/.well-known/droppen-hub` leaves it on the real hosts: the stand-in's
// names carry a port, which the hub's parser accepts only on loopback.
import { createRoot } from "react-dom/client";
import { hex } from "@/lib/e2e/bytes";
import { indexedDbKeyStore, KEY_STORE_DB, KEY_STORE_STORE, loadOrCreateDeviceKeys, readDeviceKeys } from "@/lib/e2e/keystore";
import { signHubLogin } from "@/lib/e2e/sig";
import { HubApi, loadSession } from "@/lib/hub/api";
import { hubOrigin } from "@/lib/hub/config";
import { SEEN_DB, SEEN_STORE } from "@/lib/hub/seen-store";
import { localDevice, MOVE_PATH, type MoveOutcome, startMove, startPull } from "@/lib/move";
import { readMarker } from "@/lib/move/receiver";
import { idbRecordStore } from "@/lib/move/stores";
import { setHostMode } from "@/lib/hub/hosts";
import { deviceHere } from "@/lib/hub/device-here";
import { prepareRemoval, removeHere } from "@/features/hosts/remove-here";
import { moveUnfinished } from "@/features/move";
import { MovePage } from "@/features/move/move-page";
import { BringItOver, reloadOnArrival } from "@/features/move/bring-it-over";
import "@/i18n";

declare global {
  interface Window {
    __done?: unknown;
    __outcome?: MoveOutcome;
    __steps?: string[];
    __evil?: { received: { origin: string; t: unknown }[]; done: boolean };
    __offerMoved?: boolean;
  }
}

const port = location.port ? `:${location.port}` : "";
const LEGACY = `https://droppen.test${port}`;
const APP = `https://my.droppen.test${port}`;
const origins = { legacy: LEGACY, app: APP };
const params = new URLSearchParams(location.search);
setHostMode({ origin: `wss://droppen.test${port}`, app: APP, site: null, apexMode: "move" });
if (params.get("block") === "1") window.open = () => null;

const root = document.getElementById("root")!;
const show = (text: string) => {
  const pre = document.createElement("pre");
  pre.id = "out";
  pre.textContent = text;
  root.appendChild(pre);
};
const button = (id: string, label: string, onClick: () => void) => {
  const b = document.createElement("button");
  b.id = id;
  b.textContent = label;
  b.addEventListener("click", onClick);
  root.appendChild(b);
};

async function seed(): Promise<unknown> {
  const account = params.get("account") ?? "acct-1";
  const device = params.get("device") ?? "dev-1";
  const store = indexedDbKeyStore();
  const keys = await loadOrCreateDeviceKeys(store);
  await store.put("local-wrap/v1", await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]));
  await store.put(`uk/${account}/0`, { iv: crypto.getRandomValues(new Uint8Array(12)), sealed: crypto.getRandomValues(new Uint8Array(48)) });
  await store.put(`keyring/${account}`, { entries: [1, 2, 3], at: Date.now() });
  await idbRecordStore(indexedDB, SEEN_DB, SEEN_STORE).write([{ key: account, value: { v: 1, genesis: "ab".repeat(32), seen: null } }], "overwrite", 1);
  const signKey = hex(keys.signPublic);
  localStorage.setItem(`rh.hub.device.${account}`, device);
  localStorage.setItem(`rh.hub.devicekey.${account}`, signKey);
  localStorage.setItem("rh.theme", "dark");
  localStorage.setItem("rh.hub.push.subscribed", JSON.stringify({ machine: "host-1", vapid: "vapid", seq: null }));
  localStorage.setItem("rh.draft.note", "stays");
  const res = await fetch("/__test/register", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ account, device, sign_key: signKey }) });
  const { token } = (await res.json()) as { token: string };
  localStorage.setItem("rh.hub.session", JSON.stringify({ account, device, token, expiresMs: Date.now() + 3_600_000 }));
  return { signKey, account, device };
}

async function check(): Promise<unknown> {
  const here = await localDevice();
  const records = await idbRecordStore(indexedDB, KEY_STORE_DB, KEY_STORE_STORE).readAll();
  const seen = await idbRecordStore(indexedDB, SEEN_DB, SEEN_STORE).readAll();
  const local = Object.keys(localStorage).sort();
  const out: Record<string, unknown> = {
    device: here,
    keyRecords: records?.map((r) => r.key).sort() ?? null,
    seenRecords: seen?.map((r) => r.key) ?? null,
    local,
    marker: readMarker(localStorage),
    unfinished: moveUnfinished(),
    pushMoved: localStorage.getItem("rh.hub.push.moved"),
  };
  if (records?.length) {
    const read = await readDeviceKeys(indexedDbKeyStore());
    out.keys = read.kind === "found" ? { found: true, signKey: hex(read.keys.signPublic) } : { found: false, kind: read.kind };
    // The stored keys sign this browser in afresh, here (a page loaded after the move).
    const session = loadSession();
    if (read.kind === "found" && session && hubOrigin()) {
      try {
        const api = new HubApi();
        const nonce = await api.sessionChallenge(session.account, session.device);
        const timeMs = Date.now();
        const signature = await signHubLogin({ role: "deviceSession", account: session.account, id: session.device, hub: hubOrigin()!, nonce, timeMs }, read.keys.sign.privateKey);
        await api.deviceSession({ account: session.account, device: session.device, timeMs, nonce, signature });
        out.signsIn = true;
      } catch (error) {
        out.signsIn = `no: ${error instanceof Error ? error.message : String(error)}`;
      }
    }
  }
  return out;
}

/** A foreign site: open one of the move's pages and try to get keys out of it, or put some in. */
function foreign(target: string, message: (i: number) => unknown) {
  const evil = (window.__evil = { received: [], done: false });
  window.addEventListener("message", (e) => evil.received.push({ origin: e.origin, t: (e.data as { t?: unknown } | null)?.t }));
  button("go", "open", () => {
    const w = window.open(target, "x", "popup");
    let i = 0;
    const spam = setInterval(() => {
      try {
        w?.postMessage(message(i++), "*");
      } catch {
        // gone
      }
    }, 100);
    setTimeout(() => {
      clearInterval(spam);
      evil.done = true;
    }, 3_000);
  });
}

function run(work: () => Promise<unknown>) {
  work().then(
    (v) => {
      window.__done = v;
      show(JSON.stringify(v, null, 1));
    },
    (e: unknown) => {
      window.__done = { error: e instanceof Error ? `${e.name}: ${e.message}` : String(e) };
      show(String(e));
    },
  );
}

const at = location.origin;
const path = location.pathname;
if (path === MOVE_PATH && (at === LEGACY || at === APP)) {
  createRoot(root).render(<MovePage origins={origins} />);
} else if (path === "/seed" && at === LEGACY) {
  run(seed);
} else if (path === "/check") {
  run(check);
} else if (path === "/watch" && at === APP) {
  const loads = Number(sessionStorage.getItem("loads") ?? "0") + 1;
  sessionStorage.setItem("loads", String(loads));
  reloadOnArrival({ isOpen: () => false });
  show(`loads ${loads}`);
} else if (path === "/remove" && at === LEGACY) {
  // The move notice's "Remove this browser", confirmed: local-only after an unfinished move.
  run(async () => {
    const removal = await prepareRemoval(await deviceHere(), hubOrigin() ?? "");
    const result = await removeHere(removal);
    return { localOnly: removal.localOnly ?? null, why: removal.why, result };
  });
} else if (path === "/" && (at === LEGACY || at === APP)) {
  window.__steps = [];
  const go = at === LEGACY ? startMove : startPull;
  const target = params.get("target");
  const from = params.get("from");
  const asked = target ? { legacy: LEGACY, app: target } : from ? { legacy: from, app: APP } : origins;
  // Synchronously from the click, as the interstitial and the host modes call them.
  button(at === LEGACY ? "move" : "pull", at === LEGACY ? "Move this browser" : "Bring it over", () => {
    void go({ origins: asked, onStep: (s) => window.__steps!.push(s) }).then((outcome) => {
      window.__outcome = outcome;
      show(JSON.stringify(outcome));
    });
  });
  if (at === APP) {
    const offer = document.createElement("div");
    root.appendChild(offer);
    createRoot(offer).render(<BringItOver legacy={LEGACY} onMoved={() => (window.__offerMoved = true)} />);
  }
} else if (path === "/steal") {
  foreign(`${LEGACY}${MOVE_PATH}`, () => ({ v: 1, t: "rh-move/request", id: "0".repeat(32) }));
} else if (path === "/plant") {
  foreign(`${APP}${MOVE_PATH}`, () => ({
    v: 1,
    t: "rh-move/bundle",
    id: "0".repeat(32),
    bundle: { v: 1, from: LEGACY, account: "attacker", device: "theirs", stores: [], local: [{ key: "rh.hub.device.attacker", value: "theirs" }], pushWasOn: null, digest: "00" },
  }));
} else {
  show("nothing here");
}
