import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { cn } from "@/lib/cn";
import { isCloud } from "@/lib/hub/config";
import { useApi, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { machineStorageKey } from "@/lib/machines/storage";
import { cloudKey, failure, useLinkFlow } from "./link-flow";
import { startDroppenSignIn } from "./browser-sign-in";

/*
 * The local page's first run (never droppen.app): while this device has never
 * been signed in to Droppen (`GET /api/cloud` says `none`), 「登录 Droppen」 is
 * put in front of the person, with the one line that says why — their other
 * devices here, this one from their phone. The button starts the sign-in in
 * the click (the droppen.app window opens — in the Droppen desktop app, a
 * tab of the system browser: `browser-sign-in.tsx`) and goes on to
 * Settings → Access → Droppen, which shows how it stands; without a local
 * account to sign in with, it only goes there, where the login above asks for
 * one first. Local sign-in itself is not touched. "Not now" puts it away for
 * good on this browser.
 */

/** Where "not now" is remembered: this device's, on this browser. */
const dismissedKey = (machine: MachineKey) => machineStorageKey(machine, "first-run.droppen");

function dismissedHere(machine: MachineKey): boolean {
  try {
    return localStorage.getItem(dismissedKey(machine)) === "1";
  } catch {
    return false;
  }
}

/**
 * The first-run card, or nothing: on droppen.app, once signed in (or ever
 * tried), or put away. Inside the Droppen desktop app too, where it signs in
 * through the system browser.
 */
export function LinkFirstRun({ className }: { className?: string }) {
  if (isCloud()) return null;
  return <FirstRunCard className={className} />;
}

function FirstRunCard({ className }: { className?: string }) {
  const { t } = useTranslation();
  const machine = useMachine();
  const api = useApi();
  const navigate = useNavigate();
  const { flow, opener } = useLinkFlow();
  const [dismissed, setDismissed] = useState(() => dismissedHere(machine));
  const { data: status } = useQuery({
    queryKey: cloudKey(machine),
    queryFn: async () => {
      const res = await api.GET("/api/cloud");
      if (!res.data) throw await failure(res);
      return res.data;
    },
    staleTime: 30_000,
  });
  // Only a device that was never signed in, and while no sign-in is under way.
  if (dismissed || !status || status.state !== "none" || flow.kind !== "idle") return null;
  const dismiss = () => {
    try {
      localStorage.setItem(dismissedKey(machine), "1");
    } catch {
      // nothing to remember it in: put away for this page only
    }
    setDismissed(true);
  };
  const toSettings = () => void navigate({ to: "/settings", search: { section: "access" } });
  const signIn = () => {
    // In the click itself: a popup blocker only lets through windows a click opens.
    if (status.localAuth && status.canManage) startDroppenSignIn(opener, status.hubWeb);
    toSettings();
  };
  return (
    <section data-testid="droppen-first-run" className={cn("flex flex-col gap-2.5 border border-border-strong bg-bg p-3", className)}>
      <div className="flex items-center justify-between gap-2">
        <CapsLabel>Droppen</CapsLabel>
        <button type="button" aria-label={t("not now")} title={t("not now")} onClick={dismiss} className="flex size-5 items-center justify-center text-muted hover:bg-hover hover:text-fg">
          <X className="size-3" />
        </button>
      </div>
      <p className="text-xs leading-5 text-fg">{t("Sign in to Droppen to see your other devices here, and to use this one from your phone.")}</p>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          data-variant="primary"
          data-testid="droppen-first-run-sign-in"
          onClick={signIn}
          className="flex h-7 items-center justify-center bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85"
        >
          {t("Sign in to Droppen")}
        </button>
        <button type="button" onClick={dismiss} className="flex h-7 items-center px-2 font-mono text-2xs text-muted hover:bg-hover hover:text-fg">
          {t("not now")}
        </button>
      </div>
    </section>
  );
}
