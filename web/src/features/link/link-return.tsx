import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Loader2 } from "lucide-react";
import { Centered, Note, Panel } from "@/features/cloud/ui";

/*
 * `/link/return` on a device's own page (never droppen.app): where droppen.app
 * sends the browser back with its answer to a sign-in that had no popup opener
 * — the Droppen desktop app's window opened the system browser, or a browser
 * blocked the popup (docs/local-aggregator.md §4.9). The answer — the
 * enrollment code, sealed to the attempt's one-off key — came in the address's
 * fragment, which `fragment-boot.ts` took out of the address before anything
 * else ran; this page hands it to its server, once, and says how it went.
 *
 * It stands alone, before the app's router and login: the browser it opens in
 * (the system browser, for the desktop app) has no login here, and needs none
 * — the answer is its own credential (`POST /api/cloud/link/return`). The
 * Droppen app's window and Settings → Access → Droppen show the rest; droppen.app, in
 * the tab that opened this one, finishes adding the device.
 */

/** The route that takes the answer (`server-rs/src/api/cloud.rs`, `RETURN_ROUTE`). */
export const RETURN_API = "/api/cloud/link/return";

export type ReturnState =
  /** Handing it over. */
  | { kind: "sending" }
  /** The server has it: the hub took the enrollment, droppen.app completes it. */
  | { kind: "done" }
  /** Opened without an answer (reloaded, or typed in). */
  | { kind: "nothing" }
  | { kind: "error"; code: string; message: string };

/** Hand droppen.app's answer to this page's server. */
export async function giveBack(answer: { attempt: string; sealed: string }, fetcher: typeof fetch = fetch): Promise<ReturnState> {
  try {
    const res = await fetcher(RETURN_API, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(answer),
      cache: "no-store",
      credentials: "omit",
    });
    if (res.ok) return { kind: "done" };
    const body = (await res.json().catch(() => ({}))) as { error?: unknown; message?: unknown };
    return {
      kind: "error",
      code: typeof body.error === "string" ? body.error : `http_${res.status}`,
      message: typeof body.message === "string" ? body.message : "",
    };
  } catch (error) {
    return { kind: "error", code: "unreachable", message: error instanceof Error ? error.message : String(error) };
  }
}

/** What the page says for a failure, in the reader's language. */
function failureText(t: (key: string, options?: Record<string, unknown>) => string, state: { code: string; message: string }): string {
  switch (state.code) {
    case "expired":
      return t("This sign-in ran out of time. Start again from Droppen on this device.");
    case "no_attempt":
      return t("This sign-in is no longer open on this device: it was used, cancelled or started again. If it did not finish, start again from Droppen on this device.");
    case "bad_reply":
      return t("This answer does not belong to the sign-in open on this device. Nothing was added.");
    case "already_enrolled":
      return t("This device is in a Droppen account already.");
    case "local_auth_required":
      return t("Create a local account for this device first, in Settings → Access. Once it is signed in to Droppen, your other devices are reachable through it, so it must not be open to anyone.");
    case "hub_unreachable":
      return t("This device could not reach droppen.app. Check its connection, then start again from Droppen on this device.");
    case "unreachable":
      return t("Droppen on this device did not answer. Is it still running?");
    default:
      return t("This device could not take the sign-in: {{message}}", { message: state.message || state.code });
  }
}

/** The page for a state (render-tested). */
export function LinkReturnView({ state }: { state: ReturnState }) {
  const { t } = useTranslation();
  return (
    <Centered>
      <Panel title={t("Signing in to Droppen")} label="Droppen">
        <div data-testid="link-return" data-state={state.kind} className="flex flex-col gap-3">
          {state.kind === "sending" ? (
            <p className="flex items-center gap-2 text-xs text-faint">
              <Loader2 className="size-3 animate-spin" />
              {t("handing the sign-in to Droppen on this device…")}
            </p>
          ) : state.kind === "done" ? (
            <Note tone="ok">
              <p data-signal="ok" className="flex items-center gap-2 text-xs text-ok">
                <Check className="size-3" />
                {t("This device has the sign-in.")}
              </p>
              <p className="text-xs leading-5 text-fg">{t("droppen.app, in the other tab, finishes adding it. You can close this tab and go back to Droppen.")}</p>
            </Note>
          ) : state.kind === "nothing" ? (
            <p className="text-xs leading-5 text-fg">{t("Nothing to hand over here. To sign this device in, open Droppen on it: Settings → Access → Droppen → Sign in to Droppen.")}</p>
          ) : (
            <Note tone="danger">
              <p data-signal="failed" role="alert" className="text-xs leading-5 text-danger">
                {failureText(t, state)}
              </p>
            </Note>
          )}
        </div>
      </Panel>
    </Centered>
  );
}

/** Each answer is handed over once, however often a render asks. */
const sent = new WeakMap<object, Promise<ReturnState>>();

function giveBackOnce(answer: { attempt: string; sealed: string }): Promise<ReturnState> {
  let pending = sent.get(answer);
  if (!pending) {
    pending = giveBack(answer);
    sent.set(answer, pending);
  }
  return pending;
}

/**
 * The page: hand `answer` (taken from the address before the app started,
 * `takeReturn()`) over once, then say how it went — and close, when the
 * browser lets it.
 */
export function LinkReturnPage({ answer }: { answer: { attempt: string; sealed: string } | null }) {
  const [state, setState] = useState<ReturnState>(answer ? { kind: "sending" } : { kind: "nothing" });
  useEffect(() => {
    if (!answer) return;
    let alive = true;
    void giveBackOnce(answer).then((next) => {
      if (!alive) return;
      setState(next);
      // The tab droppen.app opened for this closes itself; one the person
      // opened stays, saying it may be closed.
      if (next.kind === "done") setTimeout(() => window.close(), 2500);
    });
    return () => {
      alive = false;
    };
  }, [answer]);
  return <LinkReturnView state={state} />;
}
