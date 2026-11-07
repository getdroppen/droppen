/*
 * The relay picker (relay-and-plans §2.6): Settings → Network → Relays, and
 * "change relay…" from the path indicator. Automatic by default — no prompt,
 * ever — with each relay a row the user may pick by hand. A manual choice is
 * this device's alone and holds until they go back to automatic, or the
 * relay leaves the list or fails; then the pool goes back to automatic and
 * the row says why, quietly.
 *
 * A change applies to the next channel; "reconnect now" moves the open one
 * make-before-break (`MachineChannel.move`), so no keystroke is lost.
 */
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { cn } from "@/lib/cn";
import type { MachineChannel } from "@/lib/hub/machine";
import type { RelayEntry, RelayNotice, RelayPool } from "@/lib/hub/relays";
import { useRelayPool } from "./context";
import { knownText } from "./ui";

/** The English names of the relays' cities (the i18n key the Hub lists, e.g. `singapore`). */
const CITY_NAMES: Record<string, string> = {
  singapore: "Singapore",
  tokyo: "Tokyo",
  frankfurt: "Frankfurt",
  "us-east": "US East",
  london: "London",
  "hong-kong": "Hong Kong",
  "san-francisco": "San Francisco",
};

/** A relay's city, in the reader's language. */
export function cityName(t: TFunction, city: string | null | undefined): string {
  if (!city) return "";
  const english = CITY_NAMES[city] ?? city.charAt(0).toUpperCase() + city.slice(1);
  return t(english);
}

const ms = (v: number | null | undefined) => (v == null ? "—" : `${Math.max(1, Math.round(v))} ms`);

/** The quiet line after a manual choice fell back to automatic. */
export function noticeText(t: TFunction, notice: RelayNotice | null): string | null {
  if (!notice) return null;
  const city = notice.city ? cityName(t, notice.city) : notice.relay;
  return t("{{city}} relay unavailable, back to automatic", { city });
}

/** `bare`: inside 「这台设备」, which already says whose setting it is and names it: no heading, no rule under it. */
export function RelaySection({ pool, machine, channel, bare = false }: { pool: RelayPool; machine: string | null; channel?: MachineChannel | null; bare?: boolean }) {
  const { t, i18n } = useTranslation();
  useRelayPool(pool);
  const separator = /^(zh|ja)/.test(i18n.language ?? "") ? "、" : ", ";
  useEffect(() => {
    // Opening the picker times the relays again.
    void pool.refresh().then(() => pool.measure());
  }, [pool]);
  const dir = pool.directory;
  if (!dir || dir.relays.length === 0) return null;
  const manual = pool.manual;
  const auto = pool.choose(machine, new Set());
  const autoChoice = manual ? null : auto;
  const members = new Set(dir.accountSet.map((m) => m.id));
  const full = members.size >= dir.limit;
  const replaceable = dir.accountSet.some((m) => Date.now() - m.lastUsedMs >= 7 * 24 * 60 * 60 * 1000);
  const inUse = dir.accountSet.map((m) => cityName(t, dir.relays.find((r) => r.id === m.id)?.city) || m.id).join(separator);
  const agent = machine ? dir.agents[machine] : undefined;
  const notice = noticeText(t, pool.notice);
  const rowState = (r: RelayEntry): { label: string | null; disabled: boolean } => {
    if (r.state === "draining") return { label: t("draining"), disabled: false };
    if (full && !members.has(r.id) && !replaceable) return { label: t("full, not selectable"), disabled: true };
    const failure = pool.failure(r.id);
    if (failure) return { label: t("last failure: {{why}}", { why: knownText(failure) }), disabled: false };
    return { label: null, disabled: false };
  };
  return (
    <section id="relays" data-testid="relays" className={cn("flex flex-col gap-2 px-6 max-md:px-4", bare ? "pt-1 pb-3.5 pl-11 max-md:pl-9" : "border-b border-border py-3.5")}>
      {!bare && (
        <span className="flex items-center gap-2 font-mono text-xs text-fg">
          {t("Relays")}
          <span className="px-1 text-[10px] tracking-[0.08em] text-fg uppercase shadow-[inset_0_0_0_1px_var(--color-border-strong)]">{t("this device")}</span>
        </span>
      )}
      <ul role="radiogroup" aria-label={t("Relays")} className="flex flex-col">
        <li>
          <label data-relay="auto" className="flex cursor-pointer items-center gap-3 py-1.5">
            <input type="radio" name="relay" checked={!manual} onChange={() => pool.setManual(null)} />
            <span className="flex-1 text-xs text-fg">{t("Automatic (recommended)")}</span>
            {autoChoice && (
              <span className="font-mono text-2xs text-muted">
                {t("now: {{city}} · {{ms}}", { city: cityName(t, autoChoice.relay.city), ms: ms(autoChoice.score) })}
              </span>
            )}
          </label>
        </li>
        {dir.relays.map((r) => {
          const state = rowState(r);
          const mine = pool.rtt(r.id);
          const theirs = agent?.[r.id];
          const toMachine = mine != null && theirs != null ? mine + theirs : null;
          return (
            <li key={r.id}>
              <label
                data-relay={r.id}
                data-disabled={state.disabled ? "" : undefined}
                title={state.disabled ? t("At most {{limit}} relays at once · in use: {{list}}", { limit: dir.limit, list: inUse }) : undefined}
                className={cn("flex items-center gap-3 py-1.5", state.disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer")}
              >
                <input type="radio" name="relay" checked={manual === r.id} disabled={state.disabled} onChange={() => pool.setManual(r.id)} />
                <span className="flex min-w-0 flex-1 items-baseline gap-2">
                  <span className="truncate text-xs text-fg">{cityName(t, r.city)}</span>
                  <span className="font-mono text-2xs text-faint">{r.id}</span>
                </span>
                <span className="font-mono text-2xs text-muted tabular-nums" title={t("this device")}>
                  {ms(mine)}
                </span>
                {machine && (
                  <span className="w-20 text-right font-mono text-2xs text-faint tabular-nums" title={t("to that device (this device + that device)")}>
                    {toMachine == null ? "" : `≈ ${ms(toMachine)}`}
                  </span>
                )}
                {state.label && (
                  <span data-signal={state.disabled ? undefined : "warn"} className="max-w-48 truncate font-mono text-2xs text-warn-fg">
                    {state.label}
                  </span>
                )}
              </label>
            </li>
          );
        })}
      </ul>
      {notice && (
        <span data-testid="relay-notice" className="text-2xs text-muted">
          {notice}
        </span>
      )}
      <span className="flex items-center gap-3 text-2xs text-faint">
        <span className="flex-1">
          {t("At most {{limit}} relays at once · in use: {{list}}", { limit: dir.limit, list: inUse || "—" })}
          {" · "}
          {t("A change applies from the next connection.")}
        </span>
        {channel && channel.status === "open" && (
          <button type="button" className="font-mono text-2xs text-muted underline-offset-2 hover:text-fg hover:underline" onClick={() => void channel.move()}>
            {t("reconnect now")}
          </button>
        )}
      </span>
    </section>
  );
}
