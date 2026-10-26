/*
 * The small things droppen.app says so that a second device, and then two
 * devices on one screen, are found without anyone pointing at them
 * (`onboarding.ts`):
 *
 * - {@link AddDeviceHint}: while the account has one device that runs AI,
 *   「再加一台设备，就能在一个屏里同时看」 and the way to add one — a row on the
 *   dashboard, a line in the device switcher and in the pane picker;
 * - {@link SideBySideTip}: once it has two or more, once,
 *   「每个格子都可以放不同设备的内容」 as a line under the bar, with the way to
 *   make a screen (the rail's 「+」 does the same).
 *
 * Each is put away for good with its ×, everywhere at once. None shows on a
 * device's own local page (one device, nothing to add from there), and none
 * dials anything: they read the shell's machine list.
 */
import { useTranslation } from "react-i18next";
import { Columns2, Plus, X } from "lucide-react";
import { cn } from "@/lib/cn";
import { hubShell, useShellMachines } from "@/features/machines/shells";
import { dismissHint, type Hint, openAddDevice, useHint } from "./onboarding";

/** The hint `hint` for the page's account: whether it shows, and how to put it away. */
function useAccountHint(hint: Hint, when: (machines: number) => boolean): { shown: boolean; dismiss: () => void } {
  // droppen.app only: a device's own local page has no shell (and one device).
  const shell = hubShell();
  const machines = useShellMachines();
  const account = shell ? shell.account.account : null;
  const open = useHint(hint, account);
  // Adding a device is an account manager's (a device that cannot does not see the way in).
  const shown = !!account && open && when(machines.length) && (hint !== "add-device" || shell?.account.isAdmin === true);
  return { shown, dismiss: () => account && dismissHint(hint, account) };
}

/** Whether the account has exactly one device that runs AI and the hint to add another is still up. */
export function useAddDeviceHint(): { shown: boolean; dismiss: () => void } {
  return useAccountHint("add-device", (n) => n === 1);
}

/** Whether the account has two or more devices that run AI and the side-by-side tip is still up. */
export function useSideBySideTip(): { shown: boolean; dismiss: () => void } {
  return useAccountHint("side-by-side", (n) => n >= 2);
}

function Dismiss({ onClick, className }: { onClick: () => void; className?: string }) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      data-testid="hint-dismiss"
      aria-label={t("dismiss")}
      title={t("dismiss")}
      onClick={onClick}
      className={cn("flex size-5 shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg", className)}
    >
      <X className="size-3" />
    </button>
  );
}

/**
 * 「再加一台设备，就能在一个屏里同时看」, with the way to add one: a `row` of
 * the dashboard (`onAdd` opens the guide in place), or a `line` of a menu (to
 * 「我的设备」's guide).
 */
export function AddDeviceHint({ variant, onAdd, className }: { variant: "row" | "line"; onAdd?: () => void; className?: string }) {
  const { t } = useTranslation();
  const { shown, dismiss } = useAddDeviceHint();
  if (!shown) return null;
  const add = onAdd ?? openAddDevice;
  if (variant === "row") {
    return (
      <div data-testid="add-device-hint" className={cn("flex items-center gap-3 px-6 py-3 max-md:px-4", className)}>
        <Columns2 aria-hidden="true" className="size-4 shrink-0 text-fg" />
        <p className="min-w-0 flex-1 text-[13px] leading-5 text-fg">{t("Add another device to see both on one screen.")}</p>
        <button
          type="button"
          data-testid="add-device-hint-add"
          onClick={add}
          className="flex h-7 shrink-0 items-center gap-1.5 border border-border-strong px-2.5 text-xs text-fg hover:bg-hover"
        >
          <Plus className="size-3.5" />
          <span className="max-sm:hidden">{t("add a device that runs AI")}</span>
          <span className="sm:hidden">{t("add")}</span>
        </button>
        <Dismiss onClick={dismiss} />
      </div>
    );
  }
  return (
    <div data-testid="add-device-hint" className={cn("flex items-start gap-2", className)}>
      <Columns2 aria-hidden="true" className="mt-0.5 size-3 shrink-0 text-faint" />
      <span className="min-w-0 flex-1 font-mono text-2xs leading-4 text-muted">
        {t("Add another device to see both on one screen.")}{" "}
        <button type="button" data-testid="add-device-hint-add" onClick={add} className="text-fg underline underline-offset-2 hover:no-underline">
          {t("add a device that runs AI")}
        </button>
      </span>
      <Dismiss onClick={dismiss} className="-mt-0.5" />
    </div>
  );
}

/**
 * 「每个格子都可以放不同设备的内容」, once, as a line of its own under the bar:
 * it takes its own room, so it never covers a pane or its controls. `onScreen`
 * makes a screen (and puts the tip away). Screens are made from the desktop
 * rail, so phones do not get it.
 */
export function SideBySideTip({ onScreen }: { onScreen: () => void }) {
  const { t } = useTranslation();
  const { shown, dismiss } = useSideBySideTip();
  if (!shown) return null;
  return (
    <div data-testid="side-by-side-tip" role="note" className="flex h-8 shrink-0 items-center gap-2.5 border-b border-border bg-raised px-3 max-md:hidden">
      <Columns2 aria-hidden="true" className="size-3.5 shrink-0 text-fg" />
      <span className="min-w-0 flex-1 truncate text-xs text-fg">
        {t("Each pane can show a different device's content.")}
        <span className="text-faint"> · </span>
        <span className="text-muted">{t("Make a screen with the rail's +, then pick a device in each pane.")}</span>
      </span>
      <button
        type="button"
        data-testid="side-by-side-tip-screen"
        onClick={() => {
          dismiss();
          onScreen();
        }}
        className="flex h-6 shrink-0 items-center gap-1 border border-border-strong bg-bg px-2 font-mono text-2xs text-fg hover:bg-hover"
      >
        <Plus className="size-3" />
        {t("new screen")}
      </button>
      <Dismiss onClick={dismiss} />
    </div>
  );
}
