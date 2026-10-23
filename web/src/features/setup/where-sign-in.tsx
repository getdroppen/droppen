/*
 * Step 02's "On my phone and other devices too → Sign in to Droppen": this
 * Mac signs in to Droppen through the system browser
 * (`features/link/browser-sign-in.tsx`, the same sign-in
 * as Settings → Access → Droppen and the first-run card: one per page, so leaving the
 * step keeps it going), and the step shows how it stands — continue in the
 * browser, in the account, or what went wrong. The guide does not wait for
 * it: "Keep going; I'll finish in the browser" goes on, and the guide says
 * so once this Mac is in the account.
 */
import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";
import type { components } from "@/api/schema";
import { Spinner } from "@/components/ui/spinner";
import { useApi, useMachine } from "@/lib/machines/context";
import { BrowserSignInWaiting, startDroppenSignIn } from "@/features/link/browser-sign-in";
import { cloudKey, failure, useLinkFlow } from "@/features/link/link-flow";
import type { OpenerState } from "@/features/link/opener";

type CloudStatus = components["schemas"]["CloudStatus"];

/** Why the guide cannot sign this Mac in from here. */
export type Unavailable =
  /** `GET /api/cloud` has not answered yet: nothing to say. */
  | "loading"
  /** The device's Droppen state does not load. */
  | "broken"
  /** No local authentication: signing in needs it. */
  | "no_local_auth"
  /** Another local account manages this device's sign-in. */
  | "not_manager";

/** Why a sign-in stopped, as the step words it. */
export type Failure =
  /** The link ran out before droppen.app answered. */
  | "expired"
  /** The hub could not be reached: try again online, or this Mac only for now. */
  | "unreachable"
  /** The hub or the local server refused, and said why: its words, nothing else. */
  | "refused";

/** How this Mac's sign-in to Droppen stands, as step 02 shows it. */
export type WhereSignIn =
  /** Not signed in, nothing under way: the step's primary starts it. */
  | { kind: "none" }
  /** Nothing the guide can start here, and why. */
  | { kind: "unavailable"; reason: Unavailable; owner: string | null }
  /** Under way: in the browser, or the device being added. */
  | { kind: "pending"; flow: OpenerState }
  /** In the account. */
  | { kind: "linked"; email: string | null; device: string | null; fingerprint: string | null }
  /** It stopped. */
  | { kind: "failed"; failure: Failure; message: string | null };

/** What the step is given of the sign-in: how it stands, and what can be done (each from a click). */
export interface GuideSignIn {
  state: WhereSignIn;
  start: () => void;
  reopen: () => void;
  cancel: () => void;
}

/** The codes that mean the hub was not reached, rather than that something refused. */
const UNREACHABLE = new Set(["hub_unreachable", "unavailable"]);

/** The step's view of the device's standing (`GET /api/cloud`) and the page's sign-in. */
export function whereSignIn(status: CloudStatus | undefined, flow: OpenerState): WhereSignIn {
  if (status && (status.state === "linked" || status.state === "not_hosting")) {
    return { kind: "linked", email: status.account?.email ?? null, device: status.machine.hostname || null, fingerprint: status.machine.fingerprint ?? null };
  }
  if (flow.kind === "done") return { kind: "linked", email: flow.email || (status?.account?.email ?? null), device: status?.machine.hostname ?? null, fingerprint: status?.machine.fingerprint ?? null };
  if (flow.kind === "error") return { kind: "failed", failure: flow.code && UNREACHABLE.has(flow.code) ? "unreachable" : "refused", message: flow.message };
  if (flow.kind === "expired") return { kind: "failed", failure: "expired", message: null };
  if (flow.kind !== "idle" || status?.state === "enrolling") return { kind: "pending", flow };
  const owner = status?.owner ?? null;
  if (!status) return { kind: "unavailable", reason: "loading", owner };
  if (status.state === "broken") return { kind: "unavailable", reason: "broken", owner };
  if (!status.localAuth) return { kind: "unavailable", reason: "no_local_auth", owner };
  if (!status.canManage) return { kind: "unavailable", reason: "not_manager", owner };
  return { kind: "none" };
}

/** The page's sign-in, followed while it is under way. */
export function useGuideSignIn(): GuideSignIn {
  const machine = useMachine();
  const api = useApi();
  const { flow, setFlow, opener } = useLinkFlow();
  const busy = flow.kind !== "idle" && flow.kind !== "done" && flow.kind !== "error" && flow.kind !== "expired";
  const { data: status } = useQuery({
    queryKey: cloudKey(machine),
    queryFn: async () => {
      const res = await api.GET("/api/cloud");
      if (!res.data) throw await failure(res);
      return res.data;
    },
    refetchInterval: (q) => (busy || q.state.data?.state === "enrolling" ? 1000 : false),
  });
  // In the account while the flow waited: the flow is over (as in Settings → Access → Droppen).
  useEffect(() => {
    if ((status?.state === "linked" || status?.state === "not_hosting") && busy) setFlow({ kind: "idle" });
  }, [status?.state, busy, setFlow]);
  return {
    state: whereSignIn(status, flow),
    // In the click: inside the app the attempt's address goes to the system browser.
    start: () => {
      if (status?.hubWeb) startDroppenSignIn(opener, status.hubWeb);
    },
    reopen: () => opener.reopen(),
    cancel: () => opener.cancel(),
  };
}

const quiet =
  "flex h-7 shrink-0 items-center justify-center gap-1.5 rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40";
const box = "mt-4 max-w-[720px] border border-border p-4";

/**
 * Under step 02's choices, read out politely as it changes: with
 * "Sign in to Droppen" chosen, how the sign-in stands, or why it cannot be
 * had from here; with "Only on this Mac" chosen while this Mac is in an
 * account, that it stays there.
 */
export function WhereSignInPanel({ signIn, chosen }: { signIn: GuideSignIn; chosen: "droppen" | "local" }) {
  return (
    <div role="status" aria-live="polite" data-testid="where-sign-in-live">
      <PanelBody signIn={signIn} chosen={chosen} />
    </div>
  );
}

function PanelBody({ signIn, chosen }: { signIn: GuideSignIn; chosen: "droppen" | "local" }) {
  const { t } = useTranslation();
  const { state } = signIn;
  if (chosen === "local") {
    if (state.kind !== "linked") return null;
    return (
      <p data-testid="where-stays-linked" className="mt-4 max-w-[720px] text-xs leading-[19px] text-muted">
        {t("This Mac stays in your Droppen account. To take it out, use Settings → Access → Droppen.")}
      </p>
    );
  }
  if (state.kind === "none") return null;
  if (state.kind === "unavailable") {
    if (state.reason === "loading") return null;
    const line =
      state.reason === "broken"
        ? t("this device's Droppen state does not load")
        : state.reason === "no_local_auth"
          ? t("Create a local account for this device first, in Settings → Access. Once it is signed in to Droppen, your other devices are reachable through it, so it must not be open to anyone.")
          : state.owner
            ? t("Only {{owner}} can sign this device in.", { owner: state.owner })
            : t("This window cannot sign this device in. See Settings → Access → Droppen.");
    return (
      <p data-testid="where-unavailable" data-reason={state.reason} className="mt-4 max-w-[720px] text-xs leading-[19px] text-muted">
        {line}
      </p>
    );
  }
  if (state.kind === "pending") {
    return (
      <div data-testid="where-sign-in" data-state="pending" className={box}>
        <Pending flow={state.flow} onReopen={signIn.reopen} onCancel={signIn.cancel} />
      </div>
    );
  }
  if (state.kind === "linked") {
    return (
      <div data-testid="where-sign-in" data-state="linked" className={box}>
        <div className="flex flex-col gap-2">
          <p className="flex items-center gap-2 text-[13px] leading-[18px] font-semibold" data-testid="where-linked">
            <Check className="size-3.5" aria-hidden="true" />
            {t("This Mac is in your Droppen account")}
          </p>
          {state.email && state.device && (
            <p className="text-xs leading-[19px] text-muted">{t("Open droppen.app on your phone and sign in as {{email}}. “{{device}}” is there.", { email: state.email, device: state.device })}</p>
          )}
          {state.fingerprint && (
            <p className="flex items-baseline gap-2 text-2xs text-faint">
              {t("Fingerprint")}
              <span data-testid="machine-fingerprint" className="font-mono text-xs tracking-wide text-fg">
                {state.fingerprint}
              </span>
            </p>
          )}
        </div>
      </div>
    );
  }
  // It stopped: the heading says what happened; only an unreachable hub gets the "when you're online" advice.
  const heading =
    state.failure === "expired"
      ? t("The sign-in ran out of time. Sign in again — if droppen.app asked this browser to wait for approval, once it is approved.")
      : state.failure === "unreachable"
        ? t("Can't reach droppen.app")
        : state.message;
  return (
    <div data-testid="where-sign-in" data-state="failed" data-failure={state.failure} className={box}>
      <div className="flex flex-col gap-2" data-testid="where-failed">
        <p className="text-[13px] leading-[18px] font-semibold">{heading}</p>
        {state.failure === "unreachable" && (
          <>
            <p className="text-xs leading-[19px] text-muted">{t("Use Droppen on this Mac for now. When you're online, sign in from Settings → Access → Droppen; nothing done here is lost.")}</p>
            {state.message && (
              <p data-signal="failed" className="font-mono text-2xs break-words text-danger">
                {state.message}
              </p>
            )}
          </>
        )}
        <div>
          <button type="button" className={quiet} data-testid="where-retry" onClick={signIn.start}>
            {t("try again")}
          </button>
        </div>
      </div>
    </div>
  );
}

function Pending({ flow, onReopen, onCancel }: { flow: OpenerState; onReopen: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  // The step's primary is "Keep going…": the browser's own button is not a second one (V10).
  if (flow.kind === "browser") return <BrowserSignInWaiting attempt={flow.attempt} onReopen={onReopen} onCancel={onCancel} reopenPrimary={false} />;
  const line =
    flow.kind === "starting"
      ? t("opening droppen.app…")
      : flow.kind === "finishing"
        ? t("adding this device…")
        : t("waiting for droppen.app to add this device…");
  return (
    <p className="flex items-center gap-2 text-xs text-fg" data-testid="where-progress">
      <Spinner />
      {line}
    </p>
  );
}
