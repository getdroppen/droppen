/*
 * Which device something is on, where the page holds more than one
 * (`docs/local-aggregator.md` §6.5): the device of a new session or of a
 * pane's content, the device whose settings are shown, and the name a pane
 * carries in its header when a screen shows more than one device.
 *
 * Everything here reads what the page already knows of each device
 * (`MachineInfo`) and dials nothing: choosing a device is what asks for it
 * (the caller holds it, `useHoldMachine`).
 */
import i18n from "i18next";
import { Check, ChevronDown } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/cn";
import { formatMoment } from "@/lib/format";
import { useInfoOf, useOpenMachines } from "@/lib/machines/context";
import { LOCAL, type MachineKey } from "@/lib/machines/key";
import { isOpen, runtimeFor } from "@/lib/machines/registry";
import type { MachineInfo } from "@/lib/machines/runtime";

type T = (key: string, opts?: Record<string, unknown>) => string;

/** Whether a device can be asked for something new right now (a session started, a terminal opened). */
export function canStartOn(info: MachineInfo): boolean {
  return info.state !== "offline" && info.live !== "stale" && info.live !== "refused" && info.readOnly !== true;
}

/**
 * Whether `machine` is shown here read-only (another device, through the
 * local app's gateway): nothing kept on this page — a pane, a screen —
 * should name it.
 */
export function readOnlyHere(machine: MachineKey): boolean {
  return isOpen(machine) && runtimeFor(machine).info().get().readOnly === true;
}

/** A device's name as the page says it: "this device" for the local app's own server. */
export function deviceLabel(info: MachineInfo, t: T): string {
  return info.key === LOCAL ? t("this device") : info.name;
}

/** What a device's row says besides its name: nothing while the page is live on it. */
export function deviceNote(info: MachineInfo, t: T, opts: { forStarting?: boolean; lang?: string } = {}): string | null {
  if (info.readOnly && opts.forStarting) return t("read-only here: start new sessions on that device itself");
  if (info.state === "offline") {
    if (opts.forStarting) return t("offline — a new session needs it online");
    return info.copyAsOf ? t("offline copy · as of {{time}}", { time: formatMoment(info.copyAsOf, opts.lang ?? i18n.language) }) : t("offline");
  }
  if (info.live === "stale") return t("waiting for another of your devices to confirm");
  if (info.live === "refused") return t("this device does not accept control from this browser");
  if (info.live === "live") return null;
  if (info.readOnly) return info.state === "online" ? t("read-only here") : t(info.state);
  if (info.live === "connecting") return t("connecting…");
  return t(info.state);
}

/**
 * The order a device picker offers its default in (§6.5): the device of the
 * view it was opened from, then the one used last, then the home — the first
 * of them that is on the page and, where it matters (`usable`), can start
 * something; failing all, the first device that can, else the home.
 */
export function defaultDevice(input: {
  open: readonly MachineKey[];
  here?: MachineKey | null;
  last?: MachineKey | null;
  home: MachineKey;
  usable?: (machine: MachineKey) => boolean;
}): MachineKey {
  const { open, home, usable = () => true } = input;
  for (const candidate of [input.here, input.last, home]) {
    if (candidate && open.includes(candidate) && usable(candidate)) return candidate;
  }
  return open.find(usable) ?? home;
}

/**
 * A device's state, as the square the bar, the switcher and the picker draw.
 * `title`: what it says on hover (`undefined`: the state's own word); `null`:
 * nothing, where the state is said beside it.
 */
export function DeviceDot({ state, title }: { state: MachineInfo["state"]; title?: string | null }) {
  const { t } = useTranslation();
  return (
    <span
      data-signal={state === "offline" ? "offline" : undefined}
      title={title === null ? undefined : (title ?? t(state))}
      className={cn(
        "size-[7px] shrink-0",
        state === "online" ? "bg-fg" : state === "unknown" ? "shadow-[inset_0_0_0_1.25px_var(--color-faint)]" : "bg-danger",
      )}
    />
  );
}

/** A pane's device, in its header: its state and its name. */
export function DeviceChip({ machine }: { machine: MachineKey }) {
  const { t, i18n: tr } = useTranslation();
  const info = useInfoOf(machine);
  const note = deviceNote(info, t, { lang: tr.language });
  const name = deviceLabel(info, t);
  return (
    <span
      data-pane-device={machine}
      title={note ? `${t("on this device: {{name}}", { name })} · ${note}` : t("on this device: {{name}}", { name })}
      className="flex max-w-36 min-w-0 shrink items-center gap-1 px-1 font-mono text-2xs text-muted"
    >
      <DeviceDot state={info.state} />
      <span className="truncate">{name}</span>
    </span>
  );
}

function DeviceRow({
  machine,
  selected,
  forStarting,
  onSelect,
}: {
  machine: MachineKey;
  selected: boolean;
  forStarting: boolean;
  onSelect: () => void;
}) {
  const { t, i18n: tr } = useTranslation();
  const info = useInfoOf(machine);
  const note = deviceNote(info, t, { forStarting, lang: tr.language });
  const disabled = forStarting && !canStartOn(info);
  return (
    <button
      type="button"
      data-device={machine}
      aria-current={selected ? "true" : undefined}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        // The select menu's rows: 32px (44px under a finger), 12px in, 13px
        // sans. The device's state is the only colour in the row.
        "flex min-h-8 w-full items-center gap-2 px-3 py-1 text-left text-[13px] text-fg hover:bg-hover pointer-coarse:min-h-11 disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent",
      )}
    >
      <DeviceDot state={info.state} />
      <span className="max-w-[60%] shrink-0 truncate" title={machine}>
        {deviceLabel(info, t)}
      </span>
      <span
        title={note ?? undefined}
        data-signal={info.state === "offline" ? "offline" : undefined}
        className={cn("min-w-0 flex-1 truncate text-right", info.state === "offline" ? "text-danger" : "text-faint")}
      >
        {note}
      </span>
      <Check className={cn("size-3 shrink-0", selected ? "text-fg" : "invisible")} />
    </button>
  );
}

/** The trigger's label: the chosen device's name and state. */
function Chosen({ machine }: { machine: MachineKey }) {
  const { t } = useTranslation();
  const info = useInfoOf(machine);
  return (
    <>
      <DeviceDot state={info.state} />
      <span className="max-w-40 truncate">{deviceLabel(info, t)}</span>
    </>
  );
}

/**
 * The page's devices behind a select-shaped trigger, for forms and panes.
 * `forStarting`: the choice is where something new starts, so a device that
 * cannot take it now (offline, not accepting this browser) is offered greyed
 * with the reason. `writable`: only devices this page can change something
 * on — another device shown read-only (the local app's gateway, S1-7) is
 * not offered, so nothing that is kept (a pane, a screen) names it. Renders
 * nothing on a page with one device.
 */
export function DevicePicker({
  value,
  onSelect,
  forStarting = false,
  writable = false,
  disabled = false,
  ghost = false,
  className,
}: {
  value: MachineKey;
  onSelect: (machine: MachineKey) => void;
  forStarting?: boolean;
  writable?: boolean;
  disabled?: boolean;
  /** A borderless trigger, for a composer's chips row (`ComposerSelect` ghost). */
  ghost?: boolean;
  className?: string;
}) {
  const { t } = useTranslation();
  const listed = useOpenMachines();
  const machines = writable ? listed.filter((m) => !readOnlyHere(m)) : listed;
  const [open, setOpen] = useState(false);
  if (machines.length < 2) return null;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        data-testid="device-picker"
        data-device={value}
        disabled={disabled}
        title={t("device")}
        aria-label={t("device")}
        className={cn(
          // The same field as every other select (32px, 12px sides, 13px
          // sans; 40px on a phone), not a second 22px language. The composer's
          // ghost chip keeps its 24px mono form.
          ghost
            ? "group flex h-6 max-w-full shrink-0 items-center gap-1.5 pr-1 pl-1.5 font-mono text-2xs text-muted hover:bg-hover hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg"
            : "group flex h-8 max-w-full items-center justify-between gap-2 border border-border bg-bg px-3 text-[13px] text-fg transition-colors hover:border-border-strong data-[popup-open]:border-fg max-md:h-10",
          "disabled:cursor-default disabled:border-border disabled:bg-raised disabled:text-faint",
          className,
        )}
      >
        <Chosen machine={value} />
        <ChevronDown className="size-3.5 shrink-0 text-faint transition-transform duration-150 ease-out group-data-[popup-open]:rotate-180" />
      </PopoverTrigger>
      {/* The select's list: 4px off the trigger, `border-strong`, no shadow,
          6px above/below the rows, 32px rows (44px touch). */}
      <PopoverContent
        side="bottom"
        className="max-h-[min(50vh,20rem)] min-w-[max(var(--anchor-width),10rem)] max-w-[min(100vw-2rem,20rem)] overflow-y-auto p-0 py-1.5"
      >
        <div data-testid="device-picker-list" className="flex flex-col">
          {machines.map((machine) => (
            <DeviceRow
              key={machine}
              machine={machine}
              selected={machine === value}
              forStarting={forStarting}
              onSelect={() => {
                setOpen(false);
                if (machine !== value) onSelect(machine);
              }}
            />
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}
