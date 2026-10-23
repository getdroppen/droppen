import { type ReactNode, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Columns2, Download, ExternalLink, Loader2 } from "lucide-react";
import { hostRefusalText } from "@/api/errors";
import { cn } from "@/lib/cn";
import { type EnrollCode, formatEnrollCode } from "@/lib/e2e";
import { AccountError, type CloudAccount } from "@/lib/hub/account";
import { type AccountView, chainV3Writable } from "@/lib/hub/api";
import { type DeliveryResult, linkCourier } from "@/lib/hub/deliver";
import type { HubLink } from "@/lib/hub/link";
import { atLimit, HOSTS_KEY, type LimitHit, limitOf } from "@/lib/hub/plan";
import { CapsLabel } from "@/components/ui/caps-label";
import type { MachineKey } from "@/lib/machines/key";
import type { MachineInfo } from "@/lib/machines/runtime";
import { type Availability, AVAILABILITY, codeInstallCommand, downloadHref, INSTALL_COMMAND, otherHub, stdinEnrollCommand } from "@/features/landing/get-droppen";
import { fileLink, useLatestRelease } from "@/features/landing/release-files";
import { hubShell, useShellMachines } from "./hub-shell";
import { type NewHost, partnerFor, watchNewHost } from "./onboarding";
import { FirstMachineNote, LimitNote, useMe } from "./plan";
import { makeSideBySide, shellScreenMaker, SideBySideError } from "./side-by-side";
import { CopyButton, ErrorLine, errorText, Note, primary, quiet, secondary } from "./ui";

/*
 * Adding a computer that runs AI (docs/local-aggregator.md §3.7 「添加可运行 AI
 * 的设备」): the first way is the guide (`AddComputer`) — install Droppen
 * there, sign it in from its own page (Settings → Droppen → Sign in to
 * Droppen, `features/link`), watch it come online; the other is an
 * enrollment code for the command line (`AddMachine`). It runs
 * AI because it says so, and does not manage the account: a computer that
 * runs AI cannot, yet (review D1 — its agent neither vouches for the chain
 * nor signs). On an account upgraded to chain v3 a new computer waits until
 * the hub takes v3 entries (it gates new hosts).
 */

type AddState =
  | { kind: "starting" }
  /** The plan's devices that run AI are all in use (or being added): nothing was started. */
  | { kind: "limit"; hit: LimitHit }
  | { kind: "waiting"; code: EnrollCode }
  | { kind: "adding"; code: EnrollCode }
  | { kind: "key"; code: EnrollCode }
  | { kind: "done"; machine: string; via: DeliveryResult; first: boolean }
  | { kind: "error"; message: string };

/**
 * The enrollment-code way: a code for `agent enroll`, the agent's proof
 * checked, the computer added (v2's `addMachine` — or, on a chain upgraded to
 * v3, `addDevice3{host}`), and its first key handed over.
 */
export function AddMachine({ account, link, origin, onDone }: { account: CloudAccount; link: HubLink; origin: string; onDone: () => void }) {
  const { t } = useTranslation();
  const [state, setState] = useState<AddState>({ kind: "starting" });
  const started = useRef(false);
  const me = useMe(account, link);
  const hubUrl = origin.replace(/^ws/, "http");
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    const abort = new AbortController();
    void (async () => {
      try {
        // The plan's limit of devices that run AI first (§3.8): at it, nothing is started.
        // The hub checks too (enrollments in progress count), answered below.
        const store = me;
        if (store) {
          try {
            await store.refresh();
          } catch (error) {
            console.warn("cloud: /v1/me before adding a machine:", error);
          }
          const hit = atLimit(store.plan, HOSTS_KEY);
          if (hit) return setState({ kind: "limit", hit });
        }
        const before = (account.chain?.hosts() ?? []).filter((m) => m.status.type === "active").length;
        // On a v3 chain the hub must take v3 entries (it gates a new host): said before a code is issued.
        if (account.chain?.isV3 && !chainV3Writable(store?.view)) throw new AccountError("notWritable", t("Not available yet: Droppen turns this on once it is updated on every device of the account that runs AI."));
        const code = await account.startEnrollment();
        setState({ kind: "waiting", code });
        // Read again as it is signed: on a v3 chain the hub must take v3 entries (it gates a new host).
        const machine = await account.completeEnrollment(code, abort.signal, undefined, undefined, { writable: chainV3Writable(store?.view) });
        setState({ kind: "key", code });
        // Its first key was committed and put with the machine: hand it over.
        const { result } = await account.handOverMachineKey(machine.id, linkCourier(link, account, undefined, hubShell()?.relays));
        setState({ kind: "done", machine: machine.id, via: result, first: before === 0 });
        // Its usage and copy from now on.
        me?.soon();
        // The content keys of the devices that accept its control (S1-4), once
        // its device role signs in — in the background: if it never does here,
        // it asks for them itself.
        void account.handHostKeys(machine.id).catch((error: unknown) => console.warn(`could not hand ${machine.id} the other devices' keys (it asks for them itself):`, error));
      } catch (err) {
        const hit = limitOf(err);
        if (hit) return setState({ kind: "limit", hit });
        setState({ kind: "error", message: errorText(err) });
      }
    })();
    return () => abort.abort();
    // `me` is the account's one store: the same for as long as `account` is
  }, [account, link]);

  const pad = "px-6 py-5 max-md:px-4";
  if (state.kind === "starting") return <div className={pad}><Loader2 className="size-4 animate-spin text-faint" /></div>;
  if (state.kind === "error") return <div className={pad}><ErrorLine message={state.message} /></div>;
  if (state.kind === "limit") {
    return (
      <div className={pad}>
        <LimitNote hit={state.hit} action={{ label: t("devices"), onClick: onDone }} />
      </div>
    );
  }
  if (state.kind === "done") {
    return (
      <div className={pad}>
        <Note tone="ok">
          <span data-signal="ok" className="font-mono text-xs text-ok">{t("device added: {{id}}", { id: state.machine })}</span>
          <p className="text-xs leading-5 text-muted">
            {state.via.via === "channel"
              ? t("Its key was handed over end to end.")
              : state.via.via === "mailbox"
                ? state.via.reason === "unconfirmed"
                  ? t("A fresh channel could not open yet (waiting for another device to confirm the change): its key waits for it, sealed, in its mailbox.")
                  : t("Its key waits for it, sealed, in its mailbox.")
                : t("It refused its key: {{message}}", { message: hostRefusalText(state.via.code) ?? state.via.message })}
          </p>
          {state.first && <FirstMachineNote plan={me?.plan ?? null} />}
          <p className="text-xs text-muted">{t("The install command starts Droppen there by itself; a server started some other way connects once it restarts.")}</p>
          <button type="button" className={cn(quiet, "self-start")} onClick={onDone}>
            {t("done")}
          </button>
        </Note>
      </div>
    );
  }
  const code = formatEnrollCode(state.code);
  // The code in the installer's environment, never on a command line another
  // user of that device could read (`ps`); the installer hands it to the
  // server on standard input.
  const command = codeInstallCommand(code, hubUrl);
  const stdinCommand = stdinEnrollCommand(code, hubUrl);
  return (
    <div className="grid md:grid-cols-2">
      <div className={cn(pad, "flex flex-col gap-3 md:border-r md:border-border")}>
        <CapsLabel>{t("01 · on that device, run")}</CapsLabel>
        <code data-testid="enroll-command" className="border border-border bg-raised px-2.5 py-2 font-mono text-2xs break-all text-fg select-all">
          {command}
        </code>
        <div className="flex">
          <CopyButton text={command} label={t("copy command")} />
        </div>
        <p className="text-2xs leading-4 text-muted">{t("It installs or updates Droppen there and adds that device to your account with this code.")}</p>
        <p className="text-2xs leading-4 text-muted">{t("Droppen already running there, installed some other way? Give it the code on standard input instead:")}</p>
        <code data-testid="enroll-stdin-command" className="border border-border bg-raised px-2.5 py-2 font-mono text-2xs break-all text-fg select-all">
          {stdinCommand}
        </code>
      </div>
      <div className={cn(pad, "flex flex-col gap-3")}>
        <CapsLabel>{t("02 · waiting for it")}</CapsLabel>
        <p className="flex items-center gap-2 text-xs text-faint">
          <Loader2 className="size-3 shrink-0 animate-spin" />
          {state.kind === "key"
            ? t("added; handing the device its key (end to end if it comes online, else by mailbox)…")
            : state.kind === "adding"
              ? t("proof checked; adding the device…")
              : t("waiting for the device to prove it holds this code (valid for 10 minutes)…")}
        </p>
      </div>
    </div>
  );
}

/** Why chain v3 cannot be written for the account now: the hub itself, or computers that do not read it yet. */
export type V3Blocker = "hub" | "computers";

/**
 * Why chain v3 cannot be written now, by `/v1/me`'s `chain_v3`: `null` when
 * it can (`writable`); `hub` when the hub does not write it (`mode` `read` or
 * `off`, or a hub that predates it) — whatever the computers run; else
 * `computers` (the hub writes it once every computer reads it: `waiting`).
 * Not known yet (no view): `null`, so nothing is blamed.
 */
export function v3Blocker(view: Pick<AccountView, "chainV3"> | null | undefined): V3Blocker | null {
  if (!view || chainV3Writable(view)) return null;
  return view.chainV3?.mode === "write" ? "computers" : "hub";
}

/**
 * Why a chain v3 choice is not open yet (`blocker`): the hub does not take
 * v3 entries for the account — on Droppen's side, not the computers' — or a
 * computer runs an older Droppen (which ones, if the hub says). Nothing
 * while it is not known.
 */
export function V3Pending({ waiting, blocker = "computers" }: { waiting: string[]; blocker?: V3Blocker | null }) {
  const { t } = useTranslation();
  if (!blocker) return null;
  return (
    <p data-testid="v3-pending" data-blocker={blocker} className="text-2xs leading-4 text-muted">
      {blocker === "hub"
        ? waiting.length
          ? t("Not available yet: the hub does not take chain v3 changes for this account yet, and Droppen on {{hosts}} needs an update too.", { hosts: waiting.join(", ") })
          : t("Not available yet: the hub does not take chain v3 changes for this account yet — that is on Droppen's side, not your devices'.")
        : waiting.length
          ? t("Not available yet: update Droppen on {{hosts}} first.", { hosts: waiting.join(", ") })
          : t("Not available yet: Droppen turns this on once it is updated on every device of the account that runs AI.")}
    </p>
  );
}

/** Where the guide is: its three steps, or the one-time code instead. */
type GuideMode = "guide" | "code";

/**
 * 「添加可运行 AI 的设备」 (docs/local-aggregator.md §3.7, §4): a guide that
 * needs nobody to walk the person through it, live from start to end.
 *
 * 1. Install Droppen on that device: the install line (`get-droppen.ts`),
 *    copied in a click, and the download page for the rest — the Droppen
 *    App instead only once it is out and signs itself in
 *    (`APP_AVAILABLE`, `APP_SIGN_IN_AVAILABLE`).
 * 2. On the page it opens, 「登录 Droppen」 (S1-3's `/link`): a droppen.app
 *    window opens on that device, signs in to this account — a browser new
 *    to the account is paired first, from here (`pairWith`) — shows the
 *    fingerprint that page shows, and adds the device.
 * 3. 「正在等新设备上线…」: the shell's machine list (the account's chain,
 *    followed live) gains it, and its presence says it is online
 *    (`watchNewHost`) — nothing is dialled for it. Then, with another device
 *    that runs AI in the account, 「放进同一个屏幕」 makes a screen with both
 *    on the main device and opens it (`side-by-side.ts`).
 *
 * The one-time code for its command line stays, second (`AddMachine`). A
 * device that runs AI is never a full device (review D1: the chain keeps it
 * so, and nothing here says so); on an account upgraded to chain v3, only
 * while the hub takes v3 entries (it gates new hosts).
 */
export function AddComputer({
  account,
  link,
  origin,
  onDone,
  pairWith,
  availability = AVAILABILITY,
}: {
  account: CloudAccount;
  link: HubLink;
  origin: string;
  onDone: () => void;
  /** The pairing that adds a browser or phone (`devices.tsx`), for a sign-in window whose browser is new to the account. */
  pairWith?: (onDone: () => void) => ReactNode;
  /** Whether the Droppen App is out, and whether it signs in to Droppen (`get-droppen.ts`; tests pass others). */
  availability?: Availability;
}) {
  const { t } = useTranslation();
  const me = useMe(account, link);
  const blocked = account.chain?.isV3 === true && !chainV3Writable(me?.view);
  const [mode, setMode] = useState<GuideMode>("guide");
  const [pairing, setPairing] = useState(false);
  const machines = useShellMachines();
  // The devices that run AI as the guide opened: a new one is any other.
  const [before] = useState<ReadonlySet<MachineKey>>(() => new Set(hubShell()?.machines().get().map((m) => m.key) ?? activeHosts(account)));
  const watch = useNewHost(before, machines);
  // The App's file, as the hub names it: asked only when the guide offers the App.
  const latest = useLatestRelease(availability.app && availability.appSignIn ? undefined : null);
  const full = atLimit(me?.plan, HOSTS_KEY);
  if (mode === "code") return <AddMachine account={account} link={link} origin={origin} onDone={onDone} />;
  if (watch.kind === "ready") return <NewDeviceReady account={account} added={watch.info} machines={machines} onDone={onDone} />;
  const hub = otherHub(origin.replace(/^ws/, "http")) ? origin.replace(/^ws/, "http") : null;
  const pad = "flex min-w-0 flex-col gap-3 px-6 py-5 max-md:px-4";
  // The App only once it can sign itself in (`APP_SIGN_IN_AVAILABLE`): this guide is how a device joins the account.
  const viaApp = availability.app && availability.appSignIn;
  return (
    <div data-testid="add-computer" data-step={watch.kind}>
      {(full || blocked) && (
        <div className="flex flex-col gap-2 border-b border-border px-6 py-3 max-md:px-4">
          {full && <LimitNote hit={full} />}
          {blocked && <V3Pending waiting={me?.view?.chainV3?.waiting ?? []} blocker={v3Blocker(me?.view)} />}
        </div>
      )}
      <ol className="grid md:grid-cols-3">
        <li data-testid="add-step-install" data-app={viaApp} className={cn(pad, "border-border max-md:border-b md:border-r")}>
          {viaApp ? (
            <>
              <CapsLabel>{t("01 · install the Droppen App on that device")}</CapsLabel>
              <p className="text-xs leading-5 text-muted">
                {t("On the device your AI runs on (a Mac, for now), download the Droppen App and open it: Droppen starts there by itself, with no login page.")}
              </p>
              <div className="flex flex-wrap items-center gap-2">
                {/* The latest release's .dmg itself when the hub names it, downloaded in place; else the release page. */}
                <a {...fileLink(latest?.files.macos)} data-testid="add-app-download" className={cn(secondary, "inline-flex items-center gap-1.5")}>
                  <Download className="size-3.5" />
                  {t("Download Droppen App (macOS)")}
                </a>
                <a href={downloadHref("macos-app")} target="_blank" rel="noopener" data-testid="add-download-link" className={cn(quiet, "inline-flex items-center gap-1.5")}>
                  {t("download page")}
                  <ExternalLink className="size-3" />
                </a>
              </div>
            </>
          ) : (
            <>
              <CapsLabel>{t("01 · install Droppen on that device")}</CapsLabel>
              <p data-testid="add-app-soon" className="text-2xs leading-4 text-muted">
                {availability.app
                  ? t("The Droppen App cannot sign in to Droppen yet (that is coming in an update): to add a Mac now, run Droppen in the background.")
                  : t("The Droppen App for Mac is coming soon; until then, Droppen runs in the background.")}
              </p>
              <p className="text-xs leading-5 text-muted">{t("On the device your AI runs on (a Mac, for now), open Terminal and run:")}</p>
              <code data-testid="install-command" className="border border-border bg-raised px-2.5 py-2 font-mono text-2xs whitespace-pre-wrap text-fg select-all [overflow-wrap:anywhere]">
                {INSTALL_COMMAND}
              </code>
              <div className="flex flex-wrap items-center gap-2">
                <CopyButton text={INSTALL_COMMAND} label={t("copy command")} />
                <a href={downloadHref("macos-bg")} target="_blank" rel="noopener" data-testid="add-download-link" className={cn(quiet, "inline-flex items-center gap-1.5")}>
                  {t("download page")}
                  <ExternalLink className="size-3" />
                </a>
              </div>
              <p className="text-2xs leading-4 text-muted">{t("It starts Droppen there, makes a local login for it, and opens its page.")}</p>
            </>
          )}
        </li>
        <li data-testid="add-step-sign-in" className={cn(pad, "border-border max-md:border-b md:border-r")}>
          <CapsLabel>{viaApp ? t("02 · in the App, click Sign in to Droppen") : t("02 · on its page, sign in to Droppen")}</CapsLabel>
          <p className="text-xs leading-5 text-muted">
            {viaApp
              ? t("In the Droppen App on that device, click Sign in to Droppen (it is also under Settings → Droppen).")
              : t("On the page that opens on that device, sign in with its local login and click Sign in to Droppen (it is also under Settings → Droppen).")}
          </p>
          <p className="text-xs leading-5 text-muted">
            {t("A droppen.app window opens there: sign in to this account, check that the fingerprint matches the one its page shows, and click add this device.")}
          </p>
          {hub && (
            <p data-testid="add-other-hub" className="text-xs leading-5 text-muted">
              {t("This account is on {{hub}}, not droppen.app: on that page, open advanced first and set the hub address to {{hub}}.", { hub })}
            </p>
          )}
          {pairWith &&
            (pairing ? (
              pairWith(() => setPairing(false))
            ) : (
              <p className="text-2xs leading-4 text-muted">
                {t("Is that window's browser new to your account? It shows a code first: add it from here.")}{" "}
                <button type="button" data-testid="add-pair-browser" className="text-fg underline underline-offset-2 hover:no-underline" onClick={() => setPairing(true)}>
                  {t("add a browser or phone")}
                </button>
              </p>
            ))}
        </li>
        <li data-testid="add-step-wait" className={pad}>
          <CapsLabel>{t("03 · it comes online here")}</CapsLabel>
          <p data-testid="add-waiting" className="flex items-center gap-2 text-xs text-fg">
            <Loader2 className="size-3 shrink-0 animate-spin text-faint" />
            {watch.kind === "joined"
              ? t("{{name}} joined the account; waiting for it to come online…", { name: watch.info.name })
              : t("waiting for the new device to come online…")}
          </p>
          <p className="text-2xs leading-4 text-muted">{t("Keep this page open: it notices the new device by itself.")}</p>
        </li>
      </ol>
      <div data-testid="add-server" className="flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-border px-6 py-3 max-md:px-4">
        <p className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1 text-2xs leading-4 text-muted">
          <span className="font-medium text-fg">{t("Server or running in the background?")}</span>
          <span>{t("Install it with one line:")}</span>
          <a href={downloadHref("macos-bg")} target="_blank" rel="noopener" data-testid="add-server-macos" className="text-fg underline underline-offset-2 hover:no-underline">
            {t("Mac in the background")}
          </a>
          <span aria-hidden="true">·</span>
          <a href={downloadHref("linux")} target="_blank" rel="noopener" data-testid="add-server-linux" className="text-fg underline underline-offset-2 hover:no-underline">
            {t("Linux server")}
          </a>
          <span>{t("— or add it from its command line with a one-time code:")}</span>
        </p>
        <button type="button" className={quiet} disabled={blocked} onClick={() => setMode("code")} data-testid="add-computer-code">
          {t("show a one-time code")}
        </button>
        <button type="button" className={quiet} onClick={onDone}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/** The account's active devices that run AI, by the chain (before the shell has listed them). */
function activeHosts(account: CloudAccount): MachineKey[] {
  return (account.chain?.hosts() ?? []).filter((m) => m.status.type === "active").map((m) => m.info.id);
}

/** {@link watchNewHost} as the list changes, with the time a new device joined and a look again once patience runs out. */
function useNewHost(before: ReadonlySet<MachineKey>, machines: readonly MachineInfo[]): NewHost {
  const [joinedAt, setJoinedAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const watch = watchNewHost({ before, machines, joinedAt, now });
  const joined = watch.kind === "joined";
  useEffect(() => {
    if (!joined) return;
    const at = Date.now();
    setJoinedAt((was) => was ?? at);
    // One look again once the patience runs out: its presence may never come.
    const timer = setTimeout(() => setNow(Date.now()), PATIENCE_MS + 50);
    return () => clearTimeout(timer);
  }, [joined]);
  return watch;
}

const PATIENCE_MS = 20_000;

/**
 * The new device is in the account and up. With another device that runs AI,
 * 「现在可以把两台设备放进同一个屏幕」 and the button that does it; with none,
 * the way to open it — and to add the next one.
 */
export function NewDeviceReady({ account, added, machines, onDone }: { account: CloudAccount; added: MachineInfo; machines: readonly MachineInfo[]; onDone: () => void }) {
  const { t } = useTranslation();
  const shell = hubShell();
  const keys = machines.map((m) => m.key);
  const partner = partnerFor(added.key, { lastShown: shell?.lastShown ?? null, home: shell ? shell.home : null, machines: keys });
  const partnerInfo = machines.find((m) => m.key === partner) ?? null;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const online = added.state === "online";
  const sideBySide = async () => {
    if (!shell || !partner) return;
    setBusy(true);
    setError(null);
    try {
      // Each pane on something of its own device: its newest session, else a terminal of it (`side-by-side.ts`).
      await makeSideBySide(shellScreenMaker(shell), { title: `${partnerInfo?.name ?? partner} · ${added.name}`, left: partner, right: added.key });
    } catch (err) {
      const home = machines.find((m) => m.key === shell.home)?.name ?? shell.home;
      setError(
        err instanceof SideBySideError && err.kind === "unreachable"
          ? t("{{name}} keeps your screens and cannot be reached right now. Try again once it is online.", { name: home })
          : t("The screen could not be made: {{message}}", { message: errorText(err) }),
      );
      setBusy(false);
    }
  };
  return (
    <div data-testid="add-computer" data-step="ready" className="px-6 py-5 max-md:px-4">
      <Note tone="ok" className="max-w-xl">
        <span data-signal="ok" data-testid="add-ready" data-device={added.key} className="flex items-center gap-2 font-mono text-xs text-ok">
          <Check className="size-3 shrink-0" />
          {online ? t("{{name}} is online", { name: added.name }) : t("{{name}} is in your account", { name: added.name })}
        </span>
        {partner ? (
          <>
            <p data-testid="side-by-side-ready" className="text-[15px] leading-6 font-semibold text-fg">
              {t("Now you can put the two devices on one screen.")}
            </p>
            <p className="text-xs leading-5 text-muted">
              {t("{{a}} and {{b}}, side by side: each pane opens on its own device's latest session, or a terminal on it, and you can change what each one shows.", {
                a: partnerInfo?.name ?? partner,
                b: added.name,
              })}
            </p>
          </>
        ) : (
          <p className="text-xs leading-5 text-muted">{t("Open it to start a session there. Add another device the same way to see both on one screen.")}</p>
        )}
        <ErrorLine message={error} />
        <div className="flex flex-wrap gap-2">
          {partner ? (
            <button type="button" data-variant="primary" data-testid="side-by-side" className={primary} disabled={busy} onClick={() => void sideBySide()}>
              {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Columns2 className="size-3.5" />}
              {t("put them on one screen")}
            </button>
          ) : null}
          <button
            type="button"
            data-variant={partner ? undefined : "primary"}
            data-testid="open-new-device"
            className={partner ? quiet : primary}
            disabled={busy}
            onClick={() => shell?.select(added.key)}
          >
            {t("open {{name}}", { name: added.name })}
          </button>
          <button type="button" className={quiet} disabled={busy} onClick={onDone}>
            {t("done")}
          </button>
        </div>
      </Note>
    </div>
  );
}
