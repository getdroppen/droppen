/*
 * 「这台设备」: the settings this browser keeps for itself — direct
 * connections, and the relay it uses when a connection is not direct.
 */

import { type ReactNode, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight } from "lucide-react";
import { type CloudAccount } from "@/lib/hub/account";
import type { HubLink } from "@/lib/hub/link";
import { useStore } from "@/lib/machines/store";
import { setHubOnly } from "@/lib/transport/p2p";
import { useHubOnly } from "./context";
import { askedPlace } from "./devices-route";
import { useMe } from "./plan";
import { ErrorLine, errorText, SectionHead } from "./ui";
import { Switch } from "@/components/ui/switch";

/**
 * 「这台设备」: the settings this browser keeps for itself — whether it may
 * connect straight to a device (faster; its IP address is seen), and which
 * relay carries it when it does not. A place for more of its own settings.
 */
export function ThisDeviceSection({ name, relays }: { name: string | null; relays?: ReactNode }) {
  const { t } = useTranslation();
  const asked = useStore(askedPlace);
  // The relay picker, folded until asked for (“change relay…” opens the page on it).
  const [relaysOpen, setRelaysOpen] = useState(() => askedPlace.get()?.anchor === "relays");
  useEffect(() => {
    if (asked?.anchor === "relays") setRelaysOpen(true);
  }, [asked]);
  return (
    <section data-testid="this-device">
      <div className="px-6 max-md:px-4">
        <SectionHead index="02" title={t("This device")}>
          {name && <span className="min-w-0 truncate font-mono text-xs text-muted">{name}</span>}
        </SectionHead>
      </div>
      <div className="flex flex-col gap-1 pt-2">
        <DirectConnectionSetting />
        {relays && (
          <details open={relaysOpen} onToggle={(e) => setRelaysOpen(e.currentTarget.open)} className="group/relays">
            <summary className="flex min-h-10 cursor-pointer list-none items-center gap-2 px-6 py-2 select-none hover:bg-hover max-md:px-4 [&::-webkit-details-marker]:hidden">
              <ChevronRight aria-hidden="true" className="size-3 shrink-0 text-muted transition-transform group-open/relays:rotate-90" />
              <span className="text-[13px] font-medium text-fg">{t("Relays")}</span>
              <span className="min-w-0 truncate text-xs text-muted">{t("what carries a connection that is not direct")}</span>
            </summary>
            {relays}
          </details>
        )}
      </div>
    </section>
  );
}

/**
 * This device's direct connections (the stored switch is "hub only", from
 * before relays: `setHubOnly`). Allowed, a connection goes straight to the
 * device when it can — faster, and not counted as transfer — and that device
 * and the STUN providers see this one's IP address; not allowed, everything
 * goes through a relay, and the IP address stays between this device and the
 * relays. Kept on this device only; takes effect at once (a direct path in
 * use is let go).
 */
export function DirectConnectionSetting({ allowed, onChange }: { allowed?: boolean; onChange?: (allowed: boolean) => void }) {
  const { t } = useTranslation();
  const hubOnly = useHubOnly();
  const on = allowed ?? !hubOnly;
  const label = t("Allow direct connections (faster)");
  return (
    <div data-testid="direct-connections" data-allowed={on ? "" : undefined} className="flex items-start justify-between gap-6 px-6 py-3 max-md:gap-3 max-md:px-4">
      <div className="flex min-w-0 max-w-[48rem] flex-col gap-0.5">
        <span className="text-xs font-medium text-fg">{label}</span>
        <span className="text-xs leading-5 text-muted">
          {t(
            "Direct connections skip our servers: faster, and not counted as transfer. Your devices that run AI, and the STUN providers Cloudflare and Google, then see this device's IP address. Either way, everything stays end-to-end encrypted.",
          )}
        </span>
      </div>
      <Switch on={on} blocked={false} label={label} labelHidden testId="direct-switch" onChange={(next) => (onChange ? onChange(next) : setHubOnly(!next))} />
    </div>
  );
}

/**
 * 「备用中继」 (relay-and-plans §2.7), for the whole account, under 「这台设备」's
 * relays: off (the default), the Hub keeps one relay in the account's set —
 * each device that runs AI holds one relay connection, and a relay that fails
 * or restarts is replaced at once; on, two, so the move is quicker. Shown
 * only when the hub says what the account has (`/v1/me` `relay_prefs`).
 */
export function RelayStandbySetting({ account, link }: { account: CloudAccount; link: HubLink }) {
  const { t } = useTranslation();
  const me = useMe(account, link);
  const stored = me?.view?.relayStandby ?? null;
  const [pending, setPending] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (stored === null && pending === null) return null;
  const on = pending ?? stored ?? false;
  const label = t("Keep a standby relay");
  const change = async (next: boolean) => {
    setPending(next);
    setError(null);
    try {
      await account.api.setRelayStandby(account.session.token, next);
      await me?.refresh();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setPending(null);
    }
  };
  return (
    <div
      data-testid="relay-standby"
      data-on={on ? "" : undefined}
      className="flex items-start justify-between gap-6 py-3 pr-6 pl-11 max-md:gap-3 max-md:pr-4 max-md:pl-9"
    >
      <div className="flex min-w-0 max-w-[48rem] flex-col gap-0.5">
        <span className="text-xs font-medium text-fg">{label}</span>
        <span className="text-xs leading-5 text-muted">
          {t(
            "For the whole account: each of your devices that runs AI also stays connected to a second relay, so a relay that fails or restarts is switched faster. One more idle connection per device; an idle connection is not counted as transfer.",
          )}
        </span>
        <ErrorLine message={error} />
      </div>
      <Switch on={on} blocked={pending !== null} label={label} labelHidden testId="relay-standby-switch" onChange={(next) => void change(next)} />
    </div>
  );
}
