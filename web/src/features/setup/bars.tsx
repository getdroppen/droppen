/*
 * The guide's one line above the session list, in the Droppen app only:
 * "Setup: N steps left · Continue · ×" while a started guide is unfinished,
 * or — for an install that never started it — "No AI tool is ready on this
 * device yet · Get one ready" while no AI tool is ready. An existing install
 * never gets the welcome page itself.
 */
import { useNavigate } from "@tanstack/react-router";
import { Info, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { isDesktopApp } from "@/lib/auth";
import { useHomeMachine, useMachine } from "@/lib/machines/context";
import { useSaveProgress, useSetupProgress, useSetupTools } from "./api";
import { dismiss, type SetupBar, whichBar } from "./model";

/** The bar for this list: only the app's own device's, and nothing elsewhere. */
export function SetupBars() {
  const machine = useMachine();
  const home = useHomeMachine();
  const on = isDesktopApp() && machine === home;
  const progress = useSetupProgress(on);
  // Asked only for an install that never started the guide: detection runs the CLIs.
  const tools = useSetupTools(on && progress.data != null && !progress.data.startedAt);
  const save = useSaveProgress();
  const navigate = useNavigate();
  const bar = whichBar({ desktopApp: on, progress: progress.data, tools: tools.data });
  return <SetupBarView bar={bar} onOpen={() => void navigate({ to: "/welcome" })} onDismiss={progress.data ? () => void save(dismiss(progress.data!)) : null} />;
}

/** The bar itself, given what to show (the render tests draw it directly). */
export function SetupBarView({ bar, onOpen, onDismiss }: { bar: SetupBar; onOpen: () => void; onDismiss: (() => void) | null }) {
  const { t } = useTranslation();
  if (!bar) return null;
  return (
    <div
      data-testid={bar.kind === "resume" ? "setup-resume-bar" : "setup-no-tool-bar"}
      role="note"
      className="flex shrink-0 items-center gap-2 border-b border-border py-1 pr-1.5 pl-2.5 text-xs"
    >
      <Info className="size-3.5 shrink-0 text-muted" aria-hidden="true" />
      <span className="min-w-0 flex-1 leading-4">
        {bar.kind === "noTool" ? t("No AI tool is ready on this device yet.") : bar.left === 1 ? t("Setup: 1 step left") : t("Setup: {{n}} steps left", { n: bar.left })}
      </span>
      <Button size="sm" onClick={onOpen}>
        {bar.kind === "resume" ? t("Continue") : t("Get one ready")}
      </Button>
      {bar.kind === "resume" && onDismiss && (
        <button
          type="button"
          aria-label={t("Don't remind me")}
          title={t("Don't remind me")}
          onClick={onDismiss}
          className="flex size-5 shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg"
        >
          <X className="size-3" />
        </button>
      )}
    </div>
  );
}
