import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CheckIcon, CircleNotchIcon } from "@phosphor-icons/react";
import type { CloudAccount } from "@/lib/hub/account";
import { CODE_PAIRING_WINDOW_MS } from "@/lib/hub/api";
import { DELIVERY_TIMEOUT_MS, type PairingCause, PairingResponder, type ResponderState } from "@/lib/hub/pairing";
import { meStore } from "@/lib/hub/me";
import { atLimit, DEVICES_KEY, type LimitHit } from "@/lib/hub/plan";
import { LimitNote } from "./plan";
import { QrCode } from "./qr";
import { canJoinWithPasskey, PasskeyJoin } from "./passkey-join";
import { RecoverForm } from "./recovery";
import { clearPairingResume, pairingResume, type PairingResume } from "./sign-out";
import { Centered, CopyButton, Fingerprint, Note, Panel, primary, quiet, Tabs } from "./ui";

/*
 * A browser signed in to the account but not in its chain: pair it from a
 * device that is (QR code, its text, or an 8-digit code — `lib/hub/pairing.ts`),
 * or come back with the recovery words — or, where the account lets new
 * devices join with a passkey, ask to join with one (`passkey-join.tsx`: the
 * first tab then). Any pairing failure starts over with a fresh offer and a
 * fresh code; nothing is ever reused.
 *
 * A browser that joined with a passkey and waits (`waiting`) offers itself
 * the same way: that pairing is how a full device approves it — bound to the
 * keys on this screen, not to an id (`CloudAccount.pairDevice`). It opens
 * on the code, which works whichever device approves it.
 *
 * A browser the chain lists already but that cannot read the account — its
 * keys never arrived, or it never had reason to trust this account's chain —
 * pairs AGAIN the same way (`NoKeysScreen`): a new offer or code, and the
 * device that reads it sends its keys without adding it twice. It takes keys
 * only from that delivery; mailed keys only if it trusted the account before.
 *
 * The numeric code has a window (rh-hub `pairings`): a device that is not a
 * current admin of the chain opens one only within 30 minutes of signing in
 * (`CODE_PAIRING_WINDOW_MS`, from its registration in `/v1/me`). The screen
 * counts it down, stops offering codes once it has shut, and says so —
 * "sign in again" (a fresh sign-in registers this browser afresh, and the
 * window opens again) or the QR code, which needs no window. Every way a
 * pairing stops is said in words, with the way on that can work; the hub's
 * own message is never shown.
 */

type Mode = "qr" | "code" | "recover" | "passkey";

/** What a stopped pairing offers: start again as this device, sign in again (a new device: a new window), or the QR code (no window at all). */
export type PairingAction = "restart" | "signInAgain" | "useQr";

/** How close to the window's end the screen stops opening codes (a request on its way, a clock a little off). */
export const WINDOW_MARGIN_MS = 15_000;

/** The window's length in whole minutes, as the words say it. */
const WINDOW_MINUTES = Math.round(CODE_PAIRING_WINDOW_MS / 60_000);

/** `ms` as `m:ss` (never below `0:00`). */
export function countdown(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/**
 * The ways on from a stopped pairing, the first the main one. Starting again
 * as this device is offered only where it can work: a code needs the window
 * open — past it, only a fresh sign-in (a new device) opens it again, which a
 * device the chain names already cannot do — while the QR code needs no
 * window, nor one of the account's few open codes.
 */
export function pairingActions(cause: PairingCause, opts: { again: boolean; codePath: boolean; codeClosed: boolean }): PairingAction[] {
  if (cause === "windowClosed" || (opts.codePath && opts.codeClosed)) return opts.again ? ["useQr"] : ["signInAgain", "useQr"];
  if (cause === "tooMany") return ["restart", "useQr"];
  return ["restart"];
}

/** A responder for this mount (a remount — a new attempt — makes a new one: a new secret, a new code). */
function useResponder(account: CloudAccount, startMode: Mode): { responder: PairingResponder | null; state: ResponderState } {
  const [responder, setResponder] = useState<PairingResponder | null>(null);
  const [state, setState] = useState<ResponderState>({ kind: "idle" });
  const first = useRef(startMode);
  useEffect(() => {
    // Pairing mail is this screen's while it waits.
    account.pairing = true;
    const r = new PairingResponder({
      api: account.api,
      token: account.session.token,
      account: account.account,
      device: account.deviceId,
      signPublic: account.keys.signPublic,
      noise: account.noiseKeys,
      chain: async () => {
        await account.sync();
        return account.chain;
      },
      storeUserKeys: (keys) => account.storeUserKeys(keys),
      // Keys mailed to it (purpose 1) only for a device that trusted this account before:
      // one that never pinned it takes keys from a delivery MAC'd with this screen's secret alone.
      adopted: account.pinned
        ? async () => {
            await account.sync();
            if (account.standing !== "active" || !account.pinned) return null;
            await account.receiveMail();
            const epochs = await account.vault.epochs();
            return epochs.length ? epochs : null;
          }
        : undefined,
    });
    const off = r.onState(setState);
    setResponder(r);
    // Opening on the code (back from signing in again): the code is opened once it is chosen, below.
    if (first.current !== "code") void r.startOffer();
    return () => {
      off();
      r.stop();
      account.pairing = false;
    };
  }, [account]);
  return { responder, state };
}

/** How often the new device reads `/v1/me` while it waits: the account's device count, and when it registered. */
const LIMIT_POLL_MS = 15_000;

/**
 * What the screen reads of `/v1/me` while it waits, now and every so often:
 * the plan's device limit for a new device (§4.3, §3.8 — at the limit the
 * existing device will not add it, and only removing an old device there
 * helps; nothing for a device pairing again, in the chain already), and
 * when this device registered (its code window), where that applies.
 */
type PairingFacts = { hit: LimitHit | null; registeredMs: number | null; read: boolean };

function usePairingFacts(account: CloudAccount, opts: { limit: boolean; window: boolean }): PairingFacts {
  const [facts, setFacts] = useState<PairingFacts>({ hit: null, registeredMs: null, read: false });
  const { limit, window } = opts;
  useEffect(() => {
    if (!limit && !window) return;
    const store = meStore(account);
    let live = true;
    const read = () => {
      if (!live) return;
      const row = window ? store.view?.devices.find((d) => d.id === account.deviceId) : undefined;
      setFacts({ hit: limit ? atLimit(store.plan, DEVICES_KEY) : null, registeredMs: row?.createdMs ?? null, read: true });
    };
    if (store.view) read();
    const look = () => {
      store.refresh().then(read, (error: unknown) => {
        console.warn("cloud: /v1/me while pairing:", error);
        // Not known: the hub, which keeps the window, says it if a code is refused.
        if (live) setFacts((f) => ({ ...f, read: true }));
      });
    };
    look();
    const timer = setInterval(look, LIMIT_POLL_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [account, limit, window]);
  return facts;
}

/** The time now, a second at a time while `on`. */
function useNow(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [on]);
  return now;
}

export function PairingScreen({
  account,
  again = false,
  waiting = false,
  onPaired,
  onPending,
  onSignOut,
  onSignInAgain,
}: {
  account: CloudAccount;
  /** The chain lists this device already: it pairs again for its keys (no recovery words here). */
  again?: boolean;
  /** It joined with a passkey and waits: the pairing a full device runs with it approves it (no recovery words, no passkey here). */
  waiting?: boolean;
  onPaired: () => void;
  onPending: () => void;
  onSignOut: () => void;
  /** Sign this browser out to sign in again — registered afresh, a new code window — and come back here on `mode`. */
  onSignInAgain?: (mode: PairingResume) => void;
}) {
  const [attempt, setAttempt] = useState(0);
  // Back from signing in again: the tab it was on (read once, then forgotten).
  // An account that lets new devices join with a passkey: that first (for a new device, not one pairing again).
  const [startMode, setStartMode] = useState<Mode>(() => (waiting ? "code" : (pairingResume() ?? (!again && canJoinWithPasskey(account) ? "passkey" : "qr"))));
  useEffect(() => clearPairingResume(), []);
  // A fresh responder per attempt: a new secret, a new code.
  return (
    <PairingAttempt
      key={attempt}
      account={account}
      again={again}
      waiting={waiting}
      startMode={startMode}
      onPaired={onPaired}
      onPending={onPending}
      onSignOut={onSignOut}
      onSignInAgain={(mode) => (onSignInAgain ?? (() => onSignOut()))(mode)}
      onRestart={(mode) => {
        setStartMode(mode);
        setAttempt((n) => n + 1);
      }}
    />
  );
}

function PairingAttempt({
  account,
  again,
  waiting,
  startMode,
  onPaired,
  onPending,
  onSignOut,
  onSignInAgain,
  onRestart,
}: {
  account: CloudAccount;
  again: boolean;
  waiting: boolean;
  startMode: Mode;
  onPaired: () => void;
  onPending: () => void;
  onSignOut: () => void;
  onSignInAgain: (mode: PairingResume) => void;
  onRestart: (mode: Mode) => void;
}) {
  const { t } = useTranslation();
  const { responder, state } = useResponder(account, startMode);
  const [mode, setMode] = useState<Mode>(startMode);
  // The code window: a device that is not a current admin of the chain (a new one; chain v3, one that does not manage the account).
  const windowApplies = !again || !account.isAdmin;
  // In the chain already (pairing again, or waiting after a passkey join): no device limit, no new sign-in to offer.
  const listed = again || waiting;
  const { hit: limit, registeredMs, read: factsRead } = usePairingFacts(account, { limit: !listed, window: windowApplies });
  const windowEndMs = windowApplies && registeredMs !== null ? registeredMs + CODE_PAIRING_WINDOW_MS : null;
  const settled = state.kind === "paired" || state.kind === "verifying" || state.kind === "failed";
  const now = useNow(!settled || state.kind === "failed");
  const windowLeftMs = windowEndMs === null ? null : windowEndMs - now;
  const codeClosed = windowLeftMs !== null && windowLeftMs <= WINDOW_MARGIN_MS;
  const codeOpen = state.kind === "code" || state.kind === "answered";
  // A code still open at the hub (the tab left for the QR code and back) is shown again, window or not: it works until it lapses.
  const holdsCode = responder?.holdsCode ?? false;
  // The code tab opens a code once chosen — once it is known whether the window is open, and never once it has shut (the hub would refuse it).
  const wantsCode = mode === "code" && (holdsCode || (!codeClosed && (!windowApplies || factsRead)));
  useEffect(() => {
    if (!responder || !wantsCode) return;
    if (state.kind !== "idle" && state.kind !== "offer") return;
    responder.startCode().catch(() => undefined); // its failure is the responder's state
  }, [responder, wantsCode, state.kind]);

  const choose = useCallback(
    (next: Mode) => {
      setMode(next);
      if (next === "qr" && responder) void responder.startOffer();
    },
    [responder],
  );
  const act = (action: PairingAction, failed: boolean) => {
    if (action === "signInAgain") return onSignInAgain(mode === "code" ? "code" : "qr");
    if (action === "useQr") return failed ? onRestart("qr") : choose("qr");
    onRestart(mode === "recover" || mode === "passkey" ? "qr" : mode);
  };

  const body = (() => {
    if (state.kind === "paired") {
      return (
        <div data-signal="ok" className="flex flex-col gap-2 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)]">
          <span className="flex items-center gap-1.5 font-mono text-xs text-ok">
            <CheckIcon className="size-3.5" />
            {t("paired")}
          </span>
          <p className="text-xs leading-5 text-muted">{t("This browser is now a device of the account, with its keys.")}</p>
          <button type="button" data-variant="primary" className={primary} onClick={onPaired}>
            {t("continue")}
          </button>
        </div>
      );
    }
    if (state.kind === "verifying") {
      return (
        <p className="flex items-center gap-2 text-2xs text-faint">
          <CircleNotchIcon className="size-3 animate-spin" />
          {t("keys received; checking the account's device chain…")}
        </p>
      );
    }
    if (state.kind === "failed") {
      const actions = pairingActions(state.cause, { again: listed, codePath: mode === "code", codeClosed });
      return (
        <PairingStopped
          cause={state.cause}
          retryAfterMs={state.retryAfterMs}
          actions={actions}
          again={listed}
          windowLeftMs={mode === "code" && actions[0] === "restart" ? windowLeftMs : null}
          onAction={(a) => act(a, true)}
        />
      );
    }
    if (mode === "recover") return <RecoverForm account={account} onPending={onPending} />;
    if (mode === "passkey") return <PasskeyJoin account={account} onPending={onPending} />;
    if (mode === "code") {
      if (codeOpen) {
        // (`now` may lag the code's opening by up to a second: never more than its whole life.)
        const codeLeftMs = state.kind === "code" ? Math.min(state.expiresMs - now, DELIVERY_TIMEOUT_MS) : null;
        return <CodeShown state={state} again={listed} codeLeftMs={codeLeftMs} windowLeftMs={windowLeftMs} onRestart={() => onRestart("code")} />;
      }
      // Shut before a code was opened: said up front, never asked of the hub.
      if (codeClosed && !holdsCode) return <PairingStopped cause="windowClosed" actions={pairingActions("windowClosed", { again: listed, codePath: true, codeClosed })} again={listed} windowLeftMs={null} onAction={(a) => act(a, false)} />;
      return <CircleNotchIcon className="size-4 animate-spin text-faint" />;
    }
    if (state.kind === "offer") {
      return (
        <div className="flex flex-col gap-2">
          <p className="text-xs leading-5 text-muted">{t("On a device already in this account, open my devices → add a browser or phone, and scan this code — or paste its text there.")}</p>
          <QrCode text={state.text} label={t("pairing code for this device")} />
          <code data-testid="pairing-text" className="max-h-16 overflow-y-auto border border-border bg-raised p-2 font-mono text-2xs break-all text-fg select-all">
            {state.text}
          </code>
          <CopyButton text={state.text} label={t("copy the text")} />
          <p className="text-2xs text-faint">
            {t("this device")}: <Fingerprint value={state.fingerprint} />
          </p>
          <p className="flex items-center gap-2 text-2xs text-faint">
            <CircleNotchIcon className="size-3 animate-spin" />
            {t("waiting for the other device…")}
          </p>
        </div>
      );
    }
    return <CircleNotchIcon className="size-4 animate-spin text-faint" />;
  })();

  const options: { id: Mode; label: string }[] = [
    { id: "qr", label: t("QR code") },
    { id: "code", label: t("use a code instead") },
  ];
  if (!listed) options.push({ id: "recover", label: t("recovery words") });
  // Joining with a passkey, where the account lets new devices: the first way in (docs/passkeys.md §7.2).
  if (!listed && canJoinWithPasskey(account)) options.unshift({ id: "passkey", label: t("passkey") });
  return (
    <Centered>
      <Panel title={waiting ? t("waiting for approval") : again ? t("pair this device again") : t("add this browser to your account")}>
        {waiting && !settled && (
          <p data-testid="passkey-waiting" className="text-xs leading-5 text-muted">
            {t(
              "This browser asked to join with a passkey. To let it in, on another device of the account tap “approve” in the notice at the top, then type this code there — or scan the QR code. Until then it reads nothing and controls none of your devices; any full device can refuse it.",
            )}
          </p>
        )}
        {again && !settled && (
          <Note tone="warn">
            <span data-testid="no-keys" className="flex items-center gap-2 font-mono text-xs text-warn-fg">
              <span className="inline-flex h-4 items-center bg-warn px-1 text-[10px] leading-none font-semibold text-on-signal">{t("needs you")}</span>
              {t("in the account, but no keys here yet")}
            </span>
            <p className="text-xs leading-5 text-muted">
              {t("This browser is already a device of the account, but it cannot read it: its keys never arrived, or it has not confirmed this account yet. Pair it again from a device that can — it is not added twice, and it takes its keys only from that pairing.")}
            </p>
          </Note>
        )}
        {account.lostDevice && state.kind !== "paired" && (
          // This browser held another device of the account and lost its keys (`CloudAccount.open`): say which, so it is removed.
          <Note tone="warn">
            <span data-testid="lost-device" className="font-mono text-xs text-warn-fg">
              {t("this browser lost the keys of {{id}}", { id: account.lostDevice })}
            </span>
            <p className="text-xs leading-5 text-muted">
              {t("It signed in as a new device instead. Pair it below, then remove {{id}} in my devices on another device: it can no longer be used, and while it is listed it still counts as a device of the account.", {
                id: account.lostDevice,
              })}
            </p>
            {/* What the key store held instead, as this browser found it (for a report: not translated). */}
            {account.lostWhy && <span data-testid="lost-why" className="font-mono text-2xs break-words text-faint">{account.lostWhy}</span>}
          </Note>
        )}
        {!settled && <Tabs value={mode} onChange={choose} options={options} />}
        {/* Coming back with the recovery words is never held back by the limit (§4.3). */}
        {limit && state.kind !== "paired" && mode !== "recover" && <LimitNote hit={limit} />}
        {body}
        <div className="flex items-center justify-between text-2xs text-faint">
          <span className="truncate font-mono">{account.deviceId}</span>
          <button type="button" className={quiet} onClick={onSignOut}>
            {t("sign out")}
          </button>
        </div>
      </Panel>
    </Centered>
  );
}

/**
 * The code on screen, for the other device to type: how long it has left
 * (it lapses at the hub ten minutes after it opened, used or not), and —
 * when the window to make codes shuts before it lapses — that a new one
 * would need a fresh sign-in (or, for a device pairing again, the QR code).
 */
export function CodeShown({
  state,
  again,
  codeLeftMs,
  windowLeftMs,
  onRestart,
}: {
  state: Extract<ResponderState, { kind: "code" | "answered" }>;
  again: boolean;
  /** Until the code lapses (`null`: typed already). */
  codeLeftMs: number | null;
  /** Until this device may no longer open a code (`null`: no window, or not known). */
  windowLeftMs: number | null;
  onRestart: () => void;
}) {
  const { t } = useTranslation();
  const lastCode = codeLeftMs !== null && windowLeftMs !== null && windowLeftMs < codeLeftMs;
  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs leading-5 text-muted">{t("On a device already in this account, open my devices → add a browser or phone → enter a code, and type:")}</p>
      <div data-testid="pairing-code" className="border border-border p-4 text-center font-mono text-[40px] leading-none font-semibold tracking-[0.12em] text-fg select-all max-md:text-[32px]">
        {state.code.slice(0, 4)} {state.code.slice(4)}
      </div>
      <p className="text-2xs text-faint">
        {t("this device")}: <Fingerprint value={state.fingerprint} />
      </p>
      <p data-testid="pairing-code-left" className="flex items-center gap-2 text-2xs text-faint">
        <CircleNotchIcon className="size-3 animate-spin" />
        {state.kind === "answered"
          ? t("code entered on the other device; waiting for it to confirm…")
          : t("waiting for the code to be entered: {{time}} left, once", { time: countdown(codeLeftMs ?? 0) })}
      </p>
      {lastCode && (
        <p data-testid="pairing-window-left" className="text-2xs leading-4 text-warn-fg">
          {windowLeftMs > 0
            ? again
              ? t("In {{time}} this device can no longer make a code: use the QR code after that.", { time: countdown(windowLeftMs) })
              : t("In {{time}} a new code needs a fresh sign-in: this one still works until it lapses.", { time: countdown(windowLeftMs) })
            : again
              ? t("This code still works; after it, use the QR code.")
              : t("This code still works; a new one needs a fresh sign-in.")}
        </p>
      )}
      {state.kind === "answered" && (
        <button type="button" className={quiet} onClick={onRestart}>
          {t("the other device said the code was wrong? start again")}
        </button>
      )}
    </div>
  );
}

/** What stopped, in words (`cause`), and the ways on (`actions`, the first the main one). */
export function PairingStopped({
  cause,
  retryAfterMs,
  actions,
  again,
  windowLeftMs,
  onAction,
}: {
  cause: PairingCause;
  retryAfterMs?: number;
  actions: PairingAction[];
  again: boolean;
  /** Shown under "start again" on the code tab: how long a new code can still be made here (`null`: not shown). */
  windowLeftMs: number | null;
  onAction: (action: PairingAction) => void;
}) {
  const { t } = useTranslation();
  const n = WINDOW_MINUTES;
  const closed = cause === "windowClosed";
  const title = closed
    ? again
      ? t("use the QR code instead")
      : t("sign in again to use a code")
    : cause === "tooMany"
      ? t("too many codes are open")
      : cause === "expired"
        ? t("the code expired")
        : cause === "full"
          ? t("this code was tried too often")
          : cause === "wrongCode"
            ? t("the code did not work")
            : cause === "badDelivery"
              ? t("the keys that arrived do not check out: not using them")
              : t("pairing stopped");
  const why = closed
    ? again
      ? t("This device signed in more than {{n}} minutes ago. To keep the account safe, it can no longer pair with a code: use the QR code instead.", { n })
      : t("This browser signed in more than {{n}} minutes ago. To keep the account safe, sign in again before adding it with a code — or use the QR code, which needs no new sign-in.", { n })
    : cause === "tooMany"
      ? t("This account has as many codes open as it may. Each lapses 10 minutes after it was made: start again then — or use the QR code, which needs none.")
      : cause === "expired"
        ? t("Nobody entered it within 10 minutes.")
        : cause === "full"
          ? t("It can no longer be used.")
          : cause === "wrongCode"
            ? t("The other device entered a different code, or did not confirm it in time. This code is spent.")
            : cause === "badDelivery"
              ? t("Start again, and check that the other device shows the same fingerprint as this one.")
              : cause === "rate"
                ? retryAfterMs === undefined
                  ? t("too many attempts: try again shortly")
                  : t("too many attempts: try again in {{n}} s", { n: Math.max(1, Math.ceil(retryAfterMs / 1000)) })
                : cause === "offline"
                  ? t("Droppen could not be reached. Check the connection, then start again.")
                  : cause === "unavailable"
                    ? t("Droppen is not answering properly right now. Start again in a moment.")
                    : t("Something went wrong while pairing. Start again.");
  // The window shut although this is not what stopped it: say why starting again is not offered.
  const after =
    !closed && actions[0] === "signInAgain"
      ? t("This browser signed in more than {{n}} minutes ago: sign in again for a new code.", { n })
      : !closed && actions[0] === "useQr"
        ? t("This device signed in more than {{n}} minutes ago and can no longer make a code: use the QR code instead.", { n })
        : null;
  const label = (a: PairingAction) => (a === "signInAgain" ? t("sign in again") : a === "useQr" ? t("use the QR code") : t("start again"));
  const [main, other] = actions;
  return (
    <Note tone={closed ? "warn" : "danger"}>
      <div data-testid={closed ? "pairing-window-closed" : "pairing-failed"} data-cause={cause} className="flex flex-col gap-2">
        <span data-signal={closed ? "warn" : "failed"} className={closed ? "font-mono text-xs text-warn-fg" : "font-mono text-xs text-danger"}>
          {title}
        </span>
        <p className="text-xs leading-5 text-muted">{why}</p>
        {after && <p className="text-xs leading-5 text-muted">{after}</p>}
        {main && (
          <button type="button" data-variant="primary" data-action={main} className={primary} onClick={() => onAction(main)}>
            {label(main)}
          </button>
        )}
        {other && (
          <button type="button" data-action={other} className={quiet} onClick={() => onAction(other)}>
            {label(other)}
          </button>
        )}
        {windowLeftMs !== null && windowLeftMs > WINDOW_MARGIN_MS && (
          <p data-testid="pairing-window-left" className="text-2xs text-faint">
            {t("a new code can be made here for another {{time}}", { time: countdown(windowLeftMs) })}
          </p>
        )}
      </div>
    </Note>
  );
}

/**
 * An active device of the account that cannot read it — added, but its keys
 * never arrived; or it never had reason to trust this account's chain: it
 * pairs again (a new offer or code; the device that reads it sends its keys
 * without adding it twice). Watches for it being removed meanwhile.
 */
export function NoKeysScreen({ account, onKeys, onRemoved, onSignOut }: { account: CloudAccount; onKeys: () => void; onRemoved: () => void; onSignOut: () => void }) {
  const done = useRef({ onRemoved });
  done.current = { onRemoved };
  useEffect(() => {
    let stopped = false;
    const poll = async () => {
      try {
        await account.sync();
        if (!stopped && account.standing === "removed") done.current.onRemoved();
      } catch {
        // the next round
      }
    };
    const timer = setInterval(() => void poll(), 5000);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [account]);
  return <PairingScreen account={account} again onPaired={onKeys} onPending={() => undefined} onSignOut={onSignOut} />;
}

/** How long an approved device waits for its keys before it goes on regardless (they can still come later, or it pairs again). */
const KEYS_WAIT_MS = 30_000;

/**
 * This device joined with a passkey and waits: the pairing offer (its own
 * keys — never anything read off the chain's record of it) for a full device
 * to approve it with. Once a pairing delivered its keys it goes on; removed
 * (refused), it says so. Approved but with no delivery after a while (the
 * mail did not go), it goes on too: the page then pairs again for the keys.
 */
export function PasskeyPendingScreen({ account, onActive, onRemoved, onSignOut }: { account: CloudAccount; onActive: () => void; onRemoved: () => void; onSignOut: () => void }) {
  const done = useRef({ onActive, onRemoved });
  done.current = { onActive, onRemoved };
  useEffect(() => {
    let stopped = false;
    let activeSince: number | null = null;
    const poll = async () => {
      try {
        await account.sync();
      } catch {
        // the next round
      }
      if (stopped) return;
      if (account.standing === "removed") return done.current.onRemoved();
      if (account.standing !== "active") return;
      activeSince ??= Date.now();
      if (Date.now() - activeSince > KEYS_WAIT_MS) done.current.onActive();
    };
    const timer = setInterval(() => void poll(), 3000);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [account]);
  return <PairingScreen account={account} waiting onPaired={() => done.current.onActive()} onPending={() => undefined} onSignOut={onSignOut} />;
}
