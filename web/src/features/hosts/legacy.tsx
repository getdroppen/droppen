import { StrictMode, useEffect, useMemo, useState } from "react";
import type { Root } from "react-dom/client";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { HubApi } from "@/lib/hub/api";
import { BOOT_URL, bootPath } from "@/lib/hub/boot-url";
import { hubOrigin } from "@/lib/hub/config";
import { type DeviceHere, deviceHere } from "@/lib/hub/device-here";
import { hubHosts, notInstallable } from "@/lib/hub/hosts";
import { uaLabelOf } from "@/lib/hub/ua-label";
import { removeBrowserText, StillListedNote } from "@/features/cloud/sign-out-dialog";
import type { SignOutResult } from "@/features/cloud/sign-out";
import { Centered, danger, ErrorLine, Note, Panel, primary, quiet } from "@/features/cloud/ui";
import { isLinkPath } from "@/features/link/link-popup";
import { moveSupported, moveUnfinished, startMove } from "@/features/move";
import { prepareRemoval, type Removal, removeHere } from "./remove-here";

/*
 * The hub's origin once the app has moved to its app host (`droppen.app` →
 * `my.droppen.app`; `lib/hub/hosts.ts` `legacy`): the move notice, in place
 * of the app, before anything of the app starts or the key store opens.
 *
 * - A browser that keeps no device here (`lib/hub/device-here.ts`) goes on
 *   to the app host at once, to the same path, query and fragment it came
 *   with (`lib/hub/boot-url.ts`) — a bookmark, a notification, a
 *   verification mail's `/verify#t=…`, a browser hand-off's
 *   `/link#droppen-link=…` — with `location.replace`, so back does not
 *   return here.
 * - A browser that keeps one is told so, and offers: **move this browser**
 *   (its keys go to the app host: `features/move`), **remove this browser**
 *   (the app's "sign out and remove this browser", here: `remove-here.ts`),
 *   or **continue without moving** (on to the app host; the keys stay here).
 * - A home-screen web app of Safari on iPhone, iPad or a Mac keeps its own
 *   storage, which nothing on the app host can reach: it offers removing this
 *   browser, and adding the app host to the Home Screen and pairing it as a
 *   new device, in place of the move.
 * - `/link` opened by a device's local settings (a window with an opener):
 *   that page takes an answer only from this origin, so the sign-in cannot
 *   finish on the app host. The window says to close it and pick "continue
 *   in a browser tab instead" in Droppen (the browser hand-off, which goes
 *   through here to the app host whole), or to update Droppen first — after
 *   the move, if this browser keeps a device here.
 */

/** Whether this page runs as a web app added to the Home Screen (or the Dock) from Safari: its storage is its own, apart from Safari's. */
export function appleHomeScreenApp(env: { navigator?: Navigator; matchMedia?: (q: string) => { matches: boolean } } = globalThis): boolean {
  const nav = env.navigator as (Navigator & { standalone?: boolean }) | undefined;
  if (!nav) return false;
  if (nav.standalone === true) return true;
  let standalone = false;
  try {
    standalone = env.matchMedia?.("(display-mode: standalone)").matches === true;
  } catch {
    standalone = false;
  }
  return standalone && /Macintosh/.test(nav.userAgent ?? "") && nav.vendor === "Apple Computer, Inc.";
}

/** What the notice is for (see the module notes). */
export interface LegacyCase {
  /** The app host's origin (`https://my.droppen.app`). */
  app: string;
  /** The path, query and fragment this page loaded with. */
  continueTo: string;
  here: DeviceHere;
  /** `/link` in a window a device's local settings opened. */
  popup: boolean;
  /** A home-screen web app of Safari. */
  standalone: boolean;
}

/** Whether a browser in `c` goes straight on to the app host, with nothing to say. */
export function goesStraightOn(c: Pick<LegacyCase, "here" | "popup" | "standalone">): boolean {
  return !c.here.held && !c.popup && !c.standalone;
}

/** The move notice in `root` — or, for a browser with nothing here, straight on to the app host. */
export async function startLegacy(root: Root): Promise<void> {
  const app = hubHosts()?.app;
  if (!app) return;
  notInstallable();
  const c: LegacyCase = {
    app,
    continueTo: bootPath(BOOT_URL),
    here: await deviceHere(),
    popup: isLinkPath(BOOT_URL?.pathname ?? location.pathname) && !!window.opener,
    standalone: appleHomeScreenApp(),
  };
  if (goesStraightOn(c)) return location.replace(`${c.app}${c.continueTo}`);
  root.render(
    <StrictMode>
      <MoveNotice {...c} />
    </StrictMode>,
  );
}

type Step =
  | { kind: "choose"; error?: string }
  | { kind: "moving" }
  /** Moved, in a window a device's local settings opened: it is closed, not sent on. */
  | { kind: "moved" }
  | { kind: "opening" }
  | { kind: "confirm"; removal: Removal; again: boolean }
  | { kind: "removing" }
  | { kind: "removed"; result: SignOutResult; localOnly?: boolean };

/** Whose device this browser is, as `/v1/me` says (a kept session that still works), or `null`. */
function useWho(here: DeviceHere): { email: string; device: string | null } | null {
  const [who, setWho] = useState<{ email: string; device: string | null } | null>(null);
  useEffect(() => {
    const session = here.session;
    if (!session || session.expiresMs <= Date.now()) return;
    let live = true;
    new HubApi()
      .me(session.token, { noRenew: true })
      .then((view) => {
        if (!live) return;
        const device = uaLabelOf(view.devices.find((d) => d.id === session.device)?.uaLabel);
        setWho({ email: view.email.address, device });
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [here]);
  return who;
}

/** The notice for a browser that keeps a device here, or came as a device's sign-in window ({@link LegacyCase}). */
export function MoveNotice({
  app,
  continueTo,
  here,
  popup,
  standalone,
  go = (url: string) => location.replace(url),
  canMove = moveSupported(),
  move = startMove,
}: LegacyCase & {
  /** Where the tab goes on (tests: recorded). */
  go?: (url: string) => void;
  /** Whether moving is offered (`features/move` `moveSupported`). */
  canMove?: boolean;
  /** The move (`features/move` `startMove`). */
  move?: typeof startMove;
}) {
  const { t } = useTranslation();
  const host = useMemo(() => new URL(app).host, [app]);
  const apex = typeof location === "undefined" ? "droppen.app" : location.host;
  const [step, setStep] = useState<Step>({ kind: "choose" });
  const who = useWho(here);
  const onward = () => go(`${app}${continueTo}`);
  const offerMove = canMove && !standalone;
  // A move of this browser's device that did not finish: the app host may be using it, so Move finishes it and Remove
  // deletes only this copy (`remove-here.ts`). Read again whenever the step changes (a move may have just left it).
  const unfinished = useMemo(() => {
    const u = moveUnfinished();
    return u && here.accounts.includes(u.account) ? u : null;
  }, [here, step.kind]);

  const startMoving = () => {
    setStep({ kind: "moving" });
    // Straight from the click: the move opens its window before it waits for anything.
    void move({ app, continueTo, account: here.accounts[0] ?? null }).then((outcome) => {
      if (outcome.kind === "moved") return popup ? setStep({ kind: "moved" }) : onward();
      if (outcome.kind === "cancelled") return setStep({ kind: "choose" });
      const error =
        outcome.kind === "blocked"
          ? t("The browser blocked the {{host}} window. Allow pop-ups for {{apex}} and try again.", { host, apex })
          : outcome.kind === "failed"
            ? t("Moving did not go through, and nothing was deleted here: {{message}}", { message: outcome.message })
            : t("This browser cannot move its keys. Remove it here, or continue and add it at {{host}} as a new device.", { host });
      setStep({ kind: "choose", error });
    });
  };
  const askRemove = () => {
    setStep({ kind: "opening" });
    void prepareRemoval(here, hubOrigin() ?? "").then((removal) => setStep({ kind: "confirm", removal, again: false }));
  };
  const remove = (removal: Removal) => {
    setStep({ kind: "removing" });
    void removeHere(removal).then((result) => setStep({ kind: "removed", result, localOnly: !!removal.localOnly }));
  };

  const busy = step.kind === "moving" || step.kind === "opening" || step.kind === "removing";
  return (
    <Centered>
      <Panel title={t("Droppen moved to {{host}}", { host })} label={apex} wide>
        <div className="flex flex-col gap-4" data-testid="move-notice" data-step={step.kind}>
          {popup && (
            <Note tone="warn">
              <p data-testid="move-notice-popup" className="text-xs leading-5 text-fg">
                {t("This window was opened by Droppen on one of your devices to sign it in, and it cannot finish here any more. Close it, then pick “continue in a browser tab instead” in Droppen. If Droppen does not offer that, update Droppen on that device, or add it with a one-time code from {{host}} (my devices → add a device that runs AI).", { host })}
              </p>
            </Note>
          )}
          {here.held && (
            <p data-testid="move-notice-who" className="text-sm leading-6 text-fg">
              {who?.device
                ? t("This browser is set up as {{device}} for {{email}}, here at {{apex}}.", { device: who.device, email: who.email, apex })
                : who
                  ? t("This browser is set up as a device of {{email}}'s account, here at {{apex}}.", { email: who.email, apex })
                  : t("This browser is set up as a device of your Droppen account, here at {{apex}}.", { apex })}
            </p>
          )}
          {here.held && step.kind === "choose" && unfinished && (
            <Note tone="warn">
              <p data-testid="move-notice-unfinished" data-state={unfinished.state} className="text-xs leading-5 text-fg">
                {unfinished.state === "verified"
                  ? t("{{host}} is already using this browser's device: a move did not finish. Press Move this browser to finish it; its copy here is then deleted.", { host })
                  : t("{{host}} may already be using this browser's device: a move was cut off. Press Move this browser to finish it; its copy here is then deleted.", { host })}
              </p>
            </Note>
          )}
          {here.held && step.kind === "choose" && (
            <div className="flex flex-col gap-4">
              {standalone && (
                <p data-testid="move-notice-standalone" className="text-xs leading-5 text-muted">
                  {t("This app was added to the Home Screen from {{apex}}, and its keys cannot move to {{host}}. Remove this browser here, then open {{host}} in Safari, add it to the Home Screen, and add it to your account as a new device.", { apex, host })}
                </p>
              )}
              {offerMove && (
                <div className="flex flex-col gap-1.5">
                  <button type="button" data-testid="move-notice-move" className={primary} onClick={startMoving}>
                    {t("Move this browser")}
                  </button>
                  <p className="text-2xs leading-4 text-muted">
                    {t("Recommended. A small {{host}} window opens, takes this browser's keys and closes; this tab then carries on at {{host}}.", { host })}
                  </p>
                </div>
              )}
              <div className="flex flex-col gap-1.5">
                <button type="button" data-testid="move-notice-remove" className={danger} onClick={askRemove}>
                  {offerMove ? t("Remove this browser instead") : t("Remove this browser")}
                </button>
                <p className="text-2xs leading-4 text-muted">
                  {unfinished
                    ? t("Deletes only this browser's copy of its keys here. The device stays on your account, as {{host}} may be using it.", { host })
                    : t("Takes this browser off your account's devices where it can, and deletes its keys here.")}
                </p>
              </div>
              {!standalone && (
                <div className="flex flex-col gap-1.5">
                  <button type="button" data-testid="move-notice-continue" className={quiet} onClick={onward}>
                    {t("Continue without moving")}
                  </button>
                  <p className="text-2xs leading-4 text-muted">
                    {offerMove
                      ? t("Its keys stay here for now: {{host}} can still bring them over later.", { host })
                      : t("Its keys stay here. At {{host}}, sign in and add this browser as a new device, then remove this one in my devices.", { host })}
                  </p>
                </div>
              )}
              <ErrorLine message={step.error} />
            </div>
          )}
          {busy && (
            <p className="flex items-center gap-2 text-xs text-muted">
              <Loader2 className="size-3.5 animate-spin" />
              {step.kind === "moving" ? t("moving this browser…") : step.kind === "opening" ? t("opening your account…") : t("removing this browser…")}
            </p>
          )}
          {step.kind === "moved" && (
            <Note tone="ok">
              <p data-testid="move-notice-moved" className="text-xs leading-5 text-fg">
                {t("This browser is now at {{host}}. Close this window, then pick “continue in a browser tab instead” in Droppen.", { host })}
              </p>
            </Note>
          )}
          {step.kind === "confirm" && (
            <div className="flex flex-col gap-3" data-testid="move-notice-confirm" data-why={step.removal.why} data-local-only={step.removal.localOnly ?? ""}>
              <p className="text-xs leading-5 text-muted">
                {step.removal.localOnly
                  ? t("Only this copy of the keys is deleted here. The device stays on your account and is not taken off it, as {{host}} may be using it.", { host })
                  : removeBrowserText(step.removal.why, t, step.removal.recoveryWords)}
              </p>
              {step.again && step.removal.localOnly && (
                <Note tone="danger">
                  <p data-testid="move-notice-local-only-warning" className="text-xs leading-5 text-danger">
                    {t("The move was cut off before {{host}} said it has the keys. If it does not, they are lost with this copy. Move this browser instead to be sure.", { host })}
                  </p>
                </Note>
              )}
              {step.again && !step.removal.localOnly && (
                <Note tone="danger">
                  <p className="text-xs leading-5 text-danger">
                    {step.removal.recoveryWords
                      ? t("This is your account's last full device. Once its keys are deleted, only your recovery words bring back a full device.")
                      : t("This is your account's last full device, and the account has no recovery words. Once its keys are deleted, no device can add or remove devices, or become a full device, ever again.")}
                  </p>
                </Note>
              )}
              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  data-testid="move-notice-remove-confirm"
                  className={danger}
                  onClick={() =>
                    // Asked twice: the last full device, or a copy the app host may not have taken after all.
                    (step.removal.why === "lastAdmin" || step.removal.localOnly === "sent") && !step.again ? setStep({ ...step, again: true }) : remove(step.removal)
                  }
                >
                  {step.again ? t("delete its keys anyway") : t("Remove this browser")}
                </button>
                <button type="button" className={quiet} onClick={() => setStep({ kind: "choose" })}>
                  {t("cancel")}
                </button>
              </div>
            </div>
          )}
          {step.kind === "removed" && (
            <div className="flex flex-col gap-3" data-testid="move-notice-removed">
              <Note tone="ok">
                <p className="text-xs leading-5 text-fg">{t("This browser's keys are deleted here.")}</p>
                {step.localOnly && <p data-testid="move-notice-removed-local" className="text-xs leading-5 text-muted">{t("The device stays on your account, at {{host}}.", { host })}</p>}
              </Note>
              <StillListedNote why={step.result.stillListed} />
              {step.result.failures.length > 0 && (
                <Note tone="warn">
                  <p className="text-xs leading-5 text-fg">{t("Some of it could not be cleared:")}</p>
                  {step.result.failures.map((f) => (
                    <span key={f} className="font-mono text-2xs break-words text-muted">
                      {f}
                    </span>
                  ))}
                </Note>
              )}
            </div>
          )}
          {(!here.held || standalone || step.kind === "removed") && !popup && (
            <div className="flex flex-col gap-1.5">
              {standalone ? (
                <>
                  {!here.held && (
                    <p data-testid="move-notice-standalone" className="text-xs leading-5 text-muted">
                      {t("Open {{host}} in Safari and add it to the Home Screen: Droppen is there now.", { host })}
                    </p>
                  )}
                  <a href={`${app}/`} target="_blank" rel="noopener" data-testid="move-notice-open" className={quiet}>
                    {t("open {{host}}", { host })}
                  </a>
                </>
              ) : (
                <button type="button" data-testid="move-notice-open" className={primary} onClick={onward}>
                  {t("Continue to {{host}}", { host })}
                </button>
              )}
            </div>
          )}
        </div>
      </Panel>
    </Centered>
  );
}
