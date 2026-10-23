import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Loader2 } from "lucide-react";
import { Centered, Note, Panel, primary, quiet } from "@/features/cloud/ui";
import { type MoveOutcome, type MovePageProps, type PullSender, type ReceiverView, type SenderView, servePullSender, servePushReceiver } from "@/lib/move";

/*
 * `/move`, on both hosts of the browser key move (`lib/move`): a small popup
 * that serves whichever side of the move its origin is.
 * - On the app host it is the push's receiving side: droppen.app opened it,
 *   it takes the device, signs in with it, says so and closes.
 * - On the legacy origin it is the pull's sending side: the app host opened
 *   it, it shows which device it would send, and sends it on one click here.
 * Opened by anything else — typed in, or by another site — it does nothing
 * and says so.
 */

type T = (key: string, vars?: Record<string, unknown>) => string;

const hostOf = (origin: string) => {
  try {
    return new URL(origin).host;
  } catch {
    return origin;
  }
};

/** How a move ended, as the person reads it. */
export function outcomeText(t: T, outcome: MoveOutcome, hosts: { app: string; legacy: string }): string {
  switch (outcome.kind) {
    case "moved":
      return outcome.senderWiped
        ? t("This browser is now at {{app}}. Its copy at {{legacy}} was removed.", hosts)
        : t("This browser is now at {{app}}. {{legacy}} still holds its old copy until you open it once more.", hosts);
    case "popupBlocked":
      return t("The browser blocked the window the move needs. Allow pop-ups for this site, then try again.");
    case "closed":
      return t("The other window was closed before the move finished. Nothing was removed: you can start again.");
    case "cancelled":
      return t("Cancelled. Nothing was moved.");
    case "nothingToMove":
      return t("This browser holds no device at {{legacy}} to move.", hosts);
    case "targetHasDevice":
      return t("This browser is already set up as a device at {{app}}. Nothing was moved.", hosts);
    case "verifyFailed":
      return t("{{app}} could not sign in with the moved keys, so nothing was removed at {{legacy}}: {{why}}", { ...hosts, why: outcome.reason });
    case "unfinished":
      return t("{{app}} may already be using this browser's device, and {{legacy}} still holds it too. Press Move this browser at {{legacy}} again to finish; do not remove it there meanwhile.", hosts);
    case "busy":
      return t("Another tab of this browser is busy with its keys right now. Try again in a moment.");
    case "timeout":
      return t("The other window did not answer in time. Nothing was removed: you can start again.");
    case "failed":
      return t("The move did not finish, and nothing was removed: {{why}}", { why: outcome.reason });
  }
}

/** The app host's side (push), for a state (render-tested). */
export function ReceiverPanel({ view, hosts }: { view: ReceiverView; hosts: { app: string; legacy: string } }) {
  const { t } = useTranslation();
  return (
    <Panel title={t("Moving this browser to {{app}}", hosts)} label="Droppen">
      <div data-testid="move-receiver" data-step={view.step} className="flex flex-col gap-3">
        {view.step === "noOpener" ? (
          <p className="text-xs leading-5 text-fg">{t("Nothing to do here. To move this browser's device from {{legacy}}, open {{legacy}} and choose Move this browser.", hosts)}</p>
        ) : view.step === "waiting" || view.step === "verifying" ? (
          <p className="flex items-center gap-2 text-xs text-faint">
            <Loader2 className="size-3 animate-spin" />
            {view.step === "waiting" ? t("waiting for {{legacy}}…", hosts) : t("signing in with the moved keys…")}
          </p>
        ) : view.step === "moved" ? (
          <Note tone="ok">
            <p data-signal="ok" className="flex items-center gap-2 text-xs text-ok">
              <Check className="size-3" />
              {t("This browser is now at {{app}}.", hosts)}
            </p>
            {view.wiped && <p className="text-xs leading-5 text-fg">{t("This window closes by itself.")}</p>}
          </Note>
        ) : view.step === "kept" ? (
          <Note tone="warn">
            <p className="text-xs leading-5 text-fg">{t("This browser is now at {{app}}. {{legacy}} still holds its old copy until you open it once more.", hosts)}</p>
          </Note>
        ) : view.step === "closed" ? (
          <p className="text-xs leading-5 text-fg">{t("The other window was closed before the move finished. Nothing was removed: you can start again.")}</p>
        ) : (
          <Note tone="danger">
            <p data-signal="failed" role="alert" className="text-xs leading-5 text-danger">
              {t("Nothing was moved: {{why}}", { why: view.detail })}
            </p>
          </Note>
        )}
      </div>
    </Panel>
  );
}

/** The legacy origin's side (pull), for a state (render-tested). */
export function SenderPanel({ view, hosts, onConfirm, onCancel }: { view: SenderView; hosts: { app: string; legacy: string }; onConfirm: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <Panel title={t("Move this browser to {{app}}", hosts)} label="Droppen">
      <div data-testid="move-sender" data-step={view.step} className="flex flex-col gap-3">
        {view.step === "noOpener" ? (
          <p className="text-xs leading-5 text-fg">{t("Nothing to do here. To bring this browser's device over, open {{app}} and choose Bring it over.", hosts)}</p>
        ) : view.step === "nothing" ? (
          <p className="text-xs leading-5 text-fg">{t("This browser holds no device at {{legacy}} to move.", hosts)}</p>
        ) : view.step === "confirm" ? (
          <>
            <p className="text-xs leading-5 text-fg">{t("{{app}} asks for this browser's device: {{device}} of account {{account}}. Its keys go to {{app}}, which signs in with them; once it has, they are removed here.", { ...hosts, device: view.device, account: view.account })}</p>
            <div className="flex gap-2">
              <button type="button" data-testid="move-confirm" className={primary} onClick={onConfirm}>
                {t("Move to {{app}}", hosts)}
              </button>
              <button type="button" data-testid="move-cancel" className={quiet} onClick={onCancel}>
                {t("Cancel")}
              </button>
            </div>
          </>
        ) : view.step === "done" ? (
          <Note tone={view.outcome.kind === "moved" ? "ok" : view.outcome.kind === "cancelled" ? undefined : "danger"}>
            <p data-signal={view.outcome.kind === "moved" ? "ok" : "failed"} role={view.outcome.kind === "moved" ? undefined : "alert"} className="text-xs leading-5 text-fg">
              {outcomeText(t, view.outcome, hosts)}
            </p>
          </Note>
        ) : (
          <p className="flex items-center gap-2 text-xs text-faint">
            <Loader2 className="size-3 animate-spin" />
            {view.step === "waiting" ? t("waiting for {{app}}…", hosts) : view.step === "wiping" ? t("removing the copy here…") : t("{{app}} is signing in with the keys…", hosts)}
          </p>
        )}
      </div>
    </Panel>
  );
}

/**
 * The one side this page serves, started once per page load whatever React
 * does with effects (StrictMode runs them twice): a second start would say
 * "ready" with another id, and the first answer would go to a side that no
 * longer listens.
 */
const serving: { started: boolean; pull: PullSender | null; view: ReceiverView | SenderView | null; show: ((view: ReceiverView | SenderView) => void) | null } = {
  started: false,
  pull: null,
  view: null,
  show: null,
};

function serveOnce(origins: MovePageProps["origins"], atApp: boolean): void {
  if (serving.started) return;
  serving.started = true;
  const onView = (view: ReceiverView | SenderView) => {
    serving.view = view;
    serving.show?.(view);
  };
  if (atApp) servePushReceiver({ origins, onView });
  else serving.pull = servePullSender({ origins, onView });
}

/** The page on `/move` (see the file's comment). */
export function MovePage({ origins }: MovePageProps) {
  const hosts = { app: hostOf(origins.app), legacy: hostOf(origins.legacy) };
  const atApp = location.origin === origins.app;
  const [view, setView] = useState<ReceiverView | SenderView>(serving.view ?? { step: "waiting" });
  useEffect(() => {
    serving.show = setView;
    serveOnce(origins, atApp);
    if (serving.view) setView(serving.view);
    return () => {
      if (serving.show === setView) serving.show = null;
    };
  }, [atApp, origins]);
  return (
    <Centered>
      {atApp ? (
        <ReceiverPanel view={view as ReceiverView} hosts={hosts} />
      ) : (
        <SenderPanel view={view as SenderView} hosts={hosts} onConfirm={() => serving.pull?.confirm()} onCancel={() => serving.pull?.cancel()} />
      )}
    </Centered>
  );
}
