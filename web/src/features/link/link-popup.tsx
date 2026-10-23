import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { Check, Loader2 } from "lucide-react";
import { base32, formatEnrollCode, fromHex, hex, type KeyStore, sealLinkReturn, utf8 } from "@/lib/e2e";
import type { CloudAccount } from "@/lib/hub/account";
import { chainV3Writable } from "@/lib/hub/api";
import { linkCourier } from "@/lib/hub/deliver";
import { addHost, type AddStep } from "./add-host";
import type { HubLink } from "@/lib/hub/link";
import { atLimit, HOSTS_KEY, limitOf, type LimitHit } from "@/lib/hub/plan";
import { hubShell } from "@/features/cloud/hub-shell";
import { DOWNLOAD_PATH } from "@/features/landing/get-droppen";
import { publicHref } from "@/features/landing/route";
import { hubHosts } from "@/lib/hub/hosts";
import { LimitNote, useMe } from "@/features/cloud/plan";
import { Centered, ErrorLine, errorText, field, Note, Panel, primary, quiet } from "@/features/cloud/ui";
import { callbackTier, confirmsHost, type HandoffRequest, hostToType, LINK_PATH, type LinkMessage, machineFingerprintHex, parseLinkMessage, returnAddress } from "./handoff";
import { browserEnv, forgetHandoff, pendingHandoff } from "./fragment-boot";

/*
 * droppen.app/link — the popup a computer's local settings open to sign that
 * computer in (docs/local-aggregator.md §3.3–3.4). It signs in (or up) and
 * sets this browser up as a device exactly like the app (`cloud-app.tsx`
 * runs those screens); then, instead of the machine list, the person sees
 * which computer asks — the origin the browser says the request came from,
 * its host name and its key fingerprint — and consents. The enrollment code
 * goes back to that origin only, by postMessage; the computer proves it,
 * and this device adds it to the chain with exactly the keys the request
 * showed, hands it its key and vouches for the new head.
 *
 * The computer runs AI (it says so), so it is added as a device that runs
 * AI and never as a full device (review D1: its key is in a file every
 * program of that user can read, local-aggregator §3.3, and its agent
 * neither vouches nor signs) — there is nothing to choose here.
 * Once added (`addHost`: §4.3 step 7, `[addDevice3{host, admin: false},
 * keyEpochs]` on a v3 chain, v2's `[addMachine, keyEpochs]` before) it is
 * handed its own content key and then those of the devices that accept its
 * control (S1-4: it controls them with its one identity). An older local
 * server's `aggregator` fields are still read, and ignored.
 *
 * Without an opener — the Droppen desktop app's window opened the system
 * browser, or the device's page could not open a popup — the request comes in
 * the address instead (`#droppen-link=…`, the browser hand-off, §4.9;
 * `fragment-boot.ts` took it out of the address). Then `cb` is the origin the
 * request names, under the same rules, and the code goes back sealed to the
 * attempt's one-off key through a new tab at `<cb>/link/return` (opened in
 * the consenting click, so no popup blocker stops it; a link if one does);
 * this tab stays, waits for the proof and adds the device as above.
 */

/** Whether the page is the popup. */
export function isLinkPath(pathname: string): boolean {
  return (pathname.replace(/\/+$/, "") || "/") === LINK_PATH;
}

type Request = Extract<LinkMessage, { t: "rh-link/request" }>;

interface Handshake {
  /** `none`: no opener and no hand-off — the page was not opened by a computer's settings. */
  state: "none" | "waiting" | "ready";
  /** The browser hand-off this tab answers (no opener), or `null`: the popup's handshake. */
  handoff: HandoffRequest | null;
  /** Where the request came from (`event.origin`). */
  cb: string | null;
  request: Request | null;
  /** The computer could not finish (its message), or it cancelled. */
  opener: { kind: "error"; message: string } | { kind: "cancel" } | null;
}

let handshake: Handshake = { state: "waiting", handoff: null, cb: null, request: null, opener: null };
const listeners = new Set<() => void>();
let started = false;

function setHandshake(next: Handshake): void {
  handshake = next;
  for (const listener of listeners) listener();
}

/**
 * Say `ready` to the opener (repeated until it answers: the computer's page
 * may still be making its attempt) and take its `request` — from the opener
 * window only; the origin it came from becomes `cb`, the only origin this
 * page ever talks to again.
 */
export function startHandshake(win: Window = window): void {
  if (started) return;
  started = true;
  const opener = win.opener as Window | null;
  if (!opener) {
    const env = browserEnv();
    const handoff = env ? pendingHandoff(env) : null;
    setHandshake(handoff ? handoffHandshake(handoff) : { ...handshake, state: "none" });
    return;
  }
  const ready = () => {
    try {
      // Carries nothing: any origin may read it.
      opener.postMessage({ t: "rh-link/ready", v: 1 }, "*");
    } catch {
      // The opener went away.
    }
  };
  ready();
  const timer = setInterval(() => {
    if (handshake.request) clearInterval(timer);
    else ready();
  }, 1000);
  win.addEventListener("message", (event: MessageEvent) => {
    if (event.source !== opener) return;
    const message = parseLinkMessage(event.data);
    if (!message) return;
    if (message.t === "rh-link/request") {
      // The first request decides the origin; a second from elsewhere is ignored.
      if (handshake.cb !== null && handshake.cb !== event.origin) return;
      setHandshake({ ...handshake, state: "ready", cb: event.origin, request: message });
      return;
    }
    if (handshake.cb !== event.origin || !handshake.request || !("nonce" in message) || message.nonce !== handshake.request.nonce) return;
    if (message.t === "rh-link/error") setHandshake({ ...handshake, opener: { kind: "error", message: message.message } });
    if (message.t === "rh-link/cancel") setHandshake({ ...handshake, opener: { kind: "cancel" } });
  });
}

/**
 * The handshake for a browser hand-off: ready at once, `cb` the origin the
 * address names (§4.4's rules apply to it as to an opener's), and the request
 * the consent page shows made from the address (its `nonce` is the attempt's
 * id: no message ever carries it).
 */
export function handoffHandshake(handoff: HandoffRequest): Handshake {
  return {
    state: "ready",
    handoff,
    cb: handoff.origin,
    request: { t: "rh-link/request", v: 1, nonce: handoff.attempt, machine: handoff.machine, aggregator: null, aggregate: false },
    opener: null,
  };
}

/** For the render tests: the handshake as it would stand. */
export function setHandshakeForTest(next: Handshake): void {
  setHandshake(next);
}

/** Tell the computer's page (at `cb` only); nothing to tell without an opener. */
function toOpener(message: LinkMessage): void {
  if (handshake.handoff) return;
  const opener = window.opener as Window | null;
  if (!opener || !handshake.cb) return;
  try {
    opener.postMessage(message, handshake.cb);
  } catch {
    // The opener navigated away.
  }
}

function useHandshake(): Handshake {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => handshake,
    () => handshake,
  );
}

/** Before anything else on `/link`: no opener, nothing to do here. */
export function NoOpenerScreen() {
  const { t } = useTranslation();
  return (
    <Centered>
      <Panel title={t("add a device")}>
        <p className="text-xs leading-5 text-muted" data-testid="link-no-opener">
          {t("Open this from Droppen's local settings on the device that runs it: Settings → Access → Droppen → Sign in to Droppen.")}
        </p>
        <p className="text-xs leading-5 text-muted">
          {t("Droppen is not on that device yet?")}{" "}
          <a href={publicHref(DOWNLOAD_PATH)} data-testid="link-download" className="text-fg underline underline-offset-2 hover:no-underline">
            {t("download page")}
          </a>
        </p>
      </Panel>
    </Centered>
  );
}

/** Whether the handshake found no opener (render {@link NoOpenerScreen} then). */
export function useNoOpener(): boolean {
  return useHandshake().state === "none";
}

type Step =
  | { kind: "consent" }
  | { kind: "adding"; what: AddStep }
  | { kind: "done"; machine: string; already: boolean }
  | { kind: "limit"; hit: LimitHit }
  | { kind: "error"; message: string }
  /** Given up here (a browser hand-off: there is no window to close for the person). */
  | { kind: "cancelled" };

/** How long past the device's own expiry a hand-off is still tried here (the two clocks may differ). */
const HANDOFF_SKEW_MS = 2 * 60_000;

/** What the tab that hands the code over says while it waits for its address. */
function primeHandoffTab(tab: Window | null, text: string): void {
  try {
    if (!tab) return;
    tab.document.title = "Droppen";
    tab.document.body.style.cssText = "font:13px/1.5 -apple-system,system-ui,sans-serif;padding:32px;color:#525252";
    tab.document.body.textContent = text;
  } catch {
    // Not ours to write to: it shows the address it goes to soon enough.
  }
}

/** The consent page and what follows it, once this browser is a device of the account. */
export function LinkFlow({ account, link }: { account: CloudAccount; link: HubLink; store: KeyStore }) {
  const { t } = useTranslation();
  const hs = useHandshake();
  const me = useMe(account, link);
  const [step, setStep] = useState<Step>({ kind: "consent" });
  const [confirmHost, setConfirmHost] = useState(false);
  // A public address: its host name, typed out (`confirmsHost`).
  const [typedHost, setTypedHost] = useState("");
  const [fingerprint, setFingerprint] = useState<string | null>(null);
  const [mismatch, setMismatch] = useState<string | null>(null);
  const abort = useRef<AbortController | null>(null);
  // An "add this device" under way (set in the click, before any await).
  const adding = useRef(false);
  // The browser hand-off: the address that hands the code over, when no tab could be opened for it.
  const [manual, setManual] = useState<string | null>(null);
  const handoff = hs.handoff;
  const expired = handoff !== null && Date.now() > handoff.expiresMs + HANDOFF_SKEW_MS;
  const request = hs.request;
  const tier = useMemo(() => (hs.cb ? callbackTier(hs.cb, location.origin, hubHosts()) : "refuse"), [hs.cb]);
  const email = me?.view?.email.address ?? "";

  useEffect(() => {
    if (!request) return;
    void machineFingerprintHex(request.machine.signKey, request.machine.dhKey).then(setFingerprint);
  }, [request]);

  // The "already in an account" variant: it must be this account's, with these keys.
  const enrolled = request?.machine.enrolled ?? null;
  useEffect(() => {
    if (!enrolled || !request) return;
    void (async () => {
      const ours = hex(await account.fingerprint());
      if (ours !== enrolled.accountFp) {
        // The account key as the computer's settings show it (`display_fingerprint`).
        const fp = base32(fromHex(enrolled.accountFp)).match(/.{1,4}/g)!.join("-");
        return setMismatch(t("This device is already in another Droppen account (account key {{fp}}).", { fp }));
      }
      const info = account.machine(enrolled.id);
      if (!info || hex(info.signKey) !== request.machine.signKey || hex(info.dhKey) !== request.machine.dhKey) {
        return setMismatch(t("This device says it is {{id}} in this account, but the account does not list it with these keys.", { id: enrolled.id }));
      }
      setMismatch(null);
    })();
  }, [account, enrolled, request, t]);

  // The computer could not finish, or gave up.
  useEffect(() => {
    if (hs.opener?.kind === "cancel") {
      abort.current?.abort();
      window.close();
    } else if (hs.opener?.kind === "error" && step.kind === "adding") {
      abort.current?.abort();
      setStep({ kind: "error", message: t("The device could not finish: {{message}}", { message: hs.opener.message }) });
    }
  }, [hs.opener, step.kind, t]);

  if (hs.state !== "ready" || !request || !hs.cb) {
    return (
      <Centered>
        <Panel title={t("add a device")}>
          <p className="flex items-center gap-2 text-xs text-faint">
            <Loader2 className="size-3 animate-spin" />
            {t("waiting for the device's page…")}
          </p>
        </Panel>
      </Centered>
    );
  }

  const cb = hs.cb;
  const host = (() => {
    try {
      return new URL(cb).host;
    } catch {
      return cb;
    }
  })();
  const env = browserEnv();
  const finish = (machine: string, already: boolean) => {
    toOpener({ t: "rh-link/done", v: 1, nonce: request.nonce, account: account.account, email, machine, aggregator: null });
    setStep({ kind: "done", machine, already });
    if (handoff) {
      // The system browser's tab: not this page's to close.
      if (env) forgetHandoff(env);
      setManual(null);
    } else {
      setTimeout(() => window.close(), 2000);
    }
  };
  /**
   * The browser hand-off's way back: the code sealed to the attempt's
   * one-off key, in the fragment of `<cb>/link/return`, loaded in `tab`.
   */
  const handBack = async (code: Parameters<typeof formatEnrollCode>[0], tab: Window | null) => {
    if (!handoff) return;
    const sealed = await sealLinkReturn(
      fromHex(handoff.key),
      fromHex(handoff.state),
      { attempt: handoff.attempt, origin: handoff.origin, signKey: fromHex(handoff.machine.signKey), dhKey: fromHex(handoff.machine.dhKey) },
      utf8(JSON.stringify({ code: formatEnrollCode(code), email })),
    );
    const address = returnAddress(handoff.origin, handoff.attempt, sealed);
    if (tab && !tab.closed) {
      try {
        // Nothing of this page for the device's page to reach.
        tab.opener = null;
        tab.location.replace(address);
        return;
      } catch {
        // A tab that cannot go there is no use: the link instead.
        tab.close();
      }
    }
    setManual(address);
  };
  const fail = (error: unknown) => {
    const hit = limitOf(error);
    if (hit) return setStep({ kind: "limit", hit });
    const message = errorText(error);
    toOpener({ t: "rh-link/error", v: 1, nonce: request.nonce, code: "popup_failed", message });
    setStep({ kind: "error", message });
  };
  const add = async () => {
    // One code per consent: a second click while the first is under way does nothing.
    if (adding.current) return;
    adding.current = true;
    setStep({ kind: "adding", what: "code" });
    const controller = new AbortController();
    abort.current = controller;
    // The browser hand-off's tab, opened in this click itself (a popup
    // blocker lets only that through); it goes to the device's page once the
    // code is sealed — and closes on every way out before that.
    const tab = handoff ? window.open("", "_blank") : null;
    primeHandoffTab(tab, t("handing the code to the device…"));
    let handed = false;
    try {
      if (me) {
        try {
          await me.refresh();
        } catch {
          // The hub checks too.
        }
        // Cancelled meanwhile (the browser hand-off can be): nothing more.
        if (controller.signal.aborted) return;
        const hit = atLimit(me.plan, HOSTS_KEY);
        if (hit) return setStep({ kind: "limit", hit });
      }
      // Read just now: on a v3 chain the hub must take v3 entries (it gates a new host) — said before any code is issued.
      const writable = chainV3Writable(me?.view);
      if (account.chain?.isV3 && !writable) {
        const waiting = me?.view?.chainV3?.waiting ?? [];
        return fail(new Error(waiting.length ? t("Not available yet: update Droppen on {{hosts}} first.", { hosts: waiting.join(", ") }) : t("Not available yet: Droppen turns this on once it is updated on every device of the account that runs AI.")));
      }
      const added = await addHost(account, {
        keys: { signKey: fromHex(request.machine.signKey), dhKey: fromHex(request.machine.dhKey) },
        // Every local install runs AI: never a full device (review D1).
        admin: false,
        writable,
        courier: linkCourier(link, account, undefined, hubShell()?.relays),
        // To the computer's page only (the origin the browser named — or, for a
        // browser hand-off, sealed to that page's attempt), never to the hub.
        sendCode: async (code) => {
          // Cancelled while the code was issued: it goes nowhere.
          controller.signal.throwIfAborted();
          if (handoff) await handBack(code, tab);
          else toOpener({ t: "rh-link/code", v: 1, nonce: request.nonce, code: formatEnrollCode(code), email });
          handed = true;
        },
        onStep: (what) => setStep({ kind: "adding", what }),
        signal: controller.signal,
      });
      me?.soon();
      finish(added.machine, false);
    } catch (error) {
      if (controller.signal.aborted) return;
      fail(error);
    } finally {
      // The plan's limit, a chain the hub cannot take yet, a failure or a
      // cancel before the code was sealed: no tab left saying it hands one over.
      if (!handed) tab?.close();
      adding.current = false;
    }
  };
  const cancel = () => {
    if (handoff) {
      abort.current?.abort();
      if (env) forgetHandoff(env);
      setManual(null);
      return setStep({ kind: "cancelled" });
    }
    toOpener({ t: "rh-link/cancel", v: 1, nonce: request.nonce });
    window.close();
  };

  if (tier === "refuse") {
    return (
      <Centered>
        <Panel title={t("add a device")}>
          <Note tone="danger">
            <p data-signal="failed" className="text-xs text-danger" data-testid="link-refused">
              {t("This request did not come from a device's own page ({{origin}}); nothing was added.", { origin: cb })}
            </p>
          </Note>
        </Panel>
      </Centered>
    );
  }

  if (step.kind === "done") {
    return (
      <Centered>
        <Panel title={t("add a device")}>
          <Note tone="ok">
            <span data-signal="ok" className="flex items-center gap-2 font-mono text-xs text-ok" data-testid="link-done">
              <Check className="size-3" />
              {step.already
                ? t("This device is already in your account.")
                : handoff
                  ? t("Added {{id}}. Go back to Droppen on that device: it comes online in a moment. You can close this tab.", { id: step.machine })
                  : t("Added {{id}}. It comes online in a moment; this window closes.", { id: step.machine })}
            </span>
          </Note>
        </Panel>
      </Centered>
    );
  }

  // Expired only stops a consent not given yet: an adding under way finishes.
  if (step.kind === "cancelled" || (expired && step.kind === "consent")) {
    return (
      <Centered>
        <Panel title={t("add a device")}>
          <p className="text-xs leading-5 text-fg" data-testid={step.kind === "cancelled" ? "link-cancelled" : "link-expired"}>
            {step.kind !== "cancelled"
              ? t("This sign-in link has expired. Start again from Droppen on the device: Settings → Access → Droppen → Sign in to Droppen.")
              : t("Cancelled: nothing was added. You can close this tab.")}
          </p>
        </Panel>
      </Centered>
    );
  }

  const ready = mismatch === null && (tier === "local" || (tier === "warn" && confirmHost) || (tier === "public" && confirmsHost(typedHost, cb)));
  return (
    <Centered>
      <Panel
        title={
          enrolled && mismatch
            ? t("This device cannot be added to {{email}}", { email: email || account.account })
            : enrolled
              ? t("This device is in the Droppen account {{email}}", { email: email || account.account })
              : t("Add this device to the Droppen account {{email}}", { email: email || account.account })
        }
      >
        <div className="flex flex-col gap-3" data-testid="link-consent">
          <div className="flex flex-col gap-1">
            <span className="text-2xs text-faint">{t("asked by")}</span>
            <code className="font-mono text-base break-all text-fg" data-testid="link-origin">
              {cb}
            </code>
          </div>
          {tier === "warn" && (
            <Note tone="danger">
              <p data-signal="failed" className="text-xs text-danger" data-testid="link-warning">
                {t("This address does not look like Droppen on a device of your own.")}
              </p>
              <label className="flex items-center gap-2 text-xs text-fg">
                <input type="checkbox" checked={confirmHost} onChange={(e) => setConfirmHost(e.target.checked)} data-testid="link-confirm-host" />
                {t("I confirm {{host}} is my own device", { host })}
              </label>
            </Note>
          )}
          {tier === "public" && (
            <Note tone="danger">
              <p data-signal="failed" className="text-xs text-danger" data-testid="link-warning">
                {t("This is a public address on the internet. A device of your own is rarely there, and a page pretending to be its settings may be: add it only if you set up {{host}} yourself.", { host: hostToType(cb) })}
              </p>
              <label className="flex flex-col gap-1 text-xs text-fg">
                {t("To confirm, type its host name: {{host}}", { host: hostToType(cb) })}
                <input
                  type="text"
                  value={typedHost}
                  onChange={(e) => setTypedHost(e.target.value)}
                  autoComplete="off"
                  autoCapitalize="off"
                  spellCheck={false}
                  className={field}
                  data-testid="link-type-host"
                />
              </label>
            </Note>
          )}
          <div className="flex flex-col gap-1">
            <span className="text-2xs text-faint">{t("host name (as the device says)")}</span>
            <span className="font-mono text-xs text-fg">{request.machine.hostname}</span>
          </div>
          <div className="flex flex-col gap-1">
            <span className="text-2xs text-faint">{t("fingerprint — the device's settings show the same")}</span>
            <span className="font-mono text-sm tracking-wide text-fg" data-testid="link-fingerprint">
              {fingerprint ?? "…"}
            </span>
          </div>
          <div className="flex flex-col gap-1">
            <span className="text-2xs text-faint">{t("can run AI (as this device says)")}</span>
            <span className="font-mono text-xs text-fg" data-testid="link-runs-ai">
              {t("yes")}
            </span>
          </div>
          {handoff && !enrolled && (
            <p className="text-2xs leading-4 text-faint" data-testid="link-handoff-note">
              {t("Check that Droppen on that device shows the same fingerprint. When you add it, a new tab hands the code to that device, then closes.")}
            </p>
          )}
          {manual && (
            <Note tone="warn">
              <p className="text-xs text-fg">{t("The browser did not open the tab that hands the code to the device. Open it here:")}</p>
              <a href={manual} target="_blank" rel="noopener noreferrer" className={primary} data-testid="link-hand-back">
                {t("hand the code to the device")}
              </a>
            </Note>
          )}
          {mismatch && (
            <Note tone="danger">
              <p data-signal="failed" className="text-xs text-danger" data-testid="link-mismatch">
                {mismatch}
              </p>
            </Note>
          )}
          {step.kind === "limit" && <LimitNote hit={step.hit} />}
          {step.kind === "error" && <ErrorLine message={step.message} />}
          {step.kind === "adding" ? (
            <>
              <p className="flex items-center gap-2 text-xs text-faint" data-testid="link-adding">
                <Loader2 className="size-3 animate-spin" />
                {step.what === "code"
                  ? t("issuing a one-time code…")
                  : step.what === "proof"
                    ? t("waiting for the device to prove it holds the code…")
                    : step.what === "key"
                      ? t("added; handing the device its key…")
                      : t("handing it the content keys of the devices that accept its control…")}
              </p>
              {/* The browser hand-off: no window tells this tab the device refused the code (it says so in its own tab), so the person can stop waiting. */}
              {handoff && (step.what === "code" || step.what === "proof") && (
                <div className="flex flex-col gap-2" data-testid="link-handoff-waiting">
                  <p className="text-2xs leading-4 text-faint">
                    {t("If the device's tab says it did not take the code, cancel here and start again from Droppen on the device.")}
                  </p>
                  <div className="flex gap-2">
                    <button type="button" className={quiet} onClick={cancel} data-testid="link-cancel">
                      {t("cancel")}
                    </button>
                  </div>
                </div>
              )}
            </>
          ) : (
            <div className="flex gap-2">
              {enrolled ? (
                <button type="button" data-variant="primary" className={primary} disabled={!ready} onClick={() => finish(enrolled.id, true)} data-testid="link-confirm">
                  {t("done")}
                </button>
              ) : (
                <button type="button" data-variant="primary" className={primary} disabled={!ready} onClick={() => void add()} data-testid="link-confirm">
                  {t("add this device")}
                </button>
              )}
              <button type="button" className={quiet} onClick={cancel} data-testid="link-cancel">
                {t("cancel")}
              </button>
            </div>
          )}
        </div>
      </Panel>
    </Centered>
  );
}
