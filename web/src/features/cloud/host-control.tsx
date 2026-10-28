/*
 * Who may control a device that runs AI (chain v3's `setAccept`): the choice,
 * what letting another device in means, and the confirmation before a change
 * that lets one in or shuts one out is written.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { type Accept } from "@/lib/e2e";
import { cn } from "@/lib/cn";
import { RenewalsAhead } from "./renewal";
import { danger, primary, quiet } from "./ui";
import { CapsLabel } from "@/components/ui/caps-label";
import { CHECKBOX_INPUT, CHECKBOX_LABEL, CheckboxMark } from "@/components/ui/checkbox";

/** What saving 「谁可以控制这台电脑」 does (review D3): a list that lets a device in or shuts one out is confirmed first; one that changes nobody is saved at once. */
export function controlSaveStep(change: { admitted: string[]; shutOut: string[] }): "confirm" | "save" {
  return change.admitted.length > 0 || change.shutOut.length > 0 ? "confirm" : "save";
}

/** Whether `from` names device `id` (admins aside, which every host accepts). */
function acceptsId(from: Accept, id: string): boolean {
  return from.type === "all" || (from.type === "devices" && from.ids.includes(id));
}

/**
 * Before a new 「谁可以控制这台电脑」 is written (review D3): who it lets in —
 * full access to that computer, its identity key included — and who it shuts
 * out, which gives the computer a new machine key and calls for a new
 * identity.
 */
export function ControlConfirm({
  host,
  admitted,
  shutOut,
  renewable = false,
  renews = [],
  busy,
  onConfirm,
  onCancel,
}: {
  host: string;
  admitted: string[];
  shutOut: string[];
  /** It renews its identity key here (docs/host-identity-rotation.md §4): a narrowing renews it, rather than asking to re-enroll it. */
  renewable?: boolean;
  /** The devices that run AI a narrowing starts renewing (it, and those that let it control them). */
  renews?: string[];
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div role="alertdialog" data-testid="control-confirm" data-signal="warn" className="flex flex-col gap-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
      {admitted.length > 0 && (
        <span className="text-xs leading-5 text-fg">{t("Let {{ids}} control {{host}}? Each gets full access to it, its identity key included.", { ids: admitted.join(", "), host })}</span>
      )}
      {shutOut.length > 0 && !renewable && (
        <span className="text-xs leading-5 text-muted">
          {t("{{ids}} can no longer control it: {{host}} gets a new content key now — but a device that copied its identity key can still sign in as {{host}} and receive that key. Re-enroll {{host}} afterwards: remove it, then add it again.", { ids: shutOut.join(", "), host })}
        </span>
      )}
      {shutOut.length > 0 && renewable && (
        <>
          <span className="text-xs leading-5 text-muted">
            {t("{{ids}} can no longer control it: {{host}} gets a new content key now, and a new identity key, so a copy of the old one is no use.", { ids: shutOut.join(", "), host })}
          </span>
          <RenewalsAhead ids={renews} />
        </>
      )}
      <div className="flex gap-2">
        <button type="button" className={danger} disabled={busy} onClick={onConfirm} data-testid="control-confirm-button">
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {admitted.length > 0 ? t("allow full access") : t("save and rotate its key")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/**
 * 「谁可以控制这台电脑」 (`setAccept`): only your full devices (chain v3's
 * `admins`, the default), every device, or the full devices and the ones
 * picked (a limited device, or another device that runs AI) — with what
 * allowing another computer that runs AI means.
 */
export function HostControl({
  host,
  policy,
  declaresPolicy,
  candidates,
  upgraded,
  mustRenew,
  renewable,
  renewsIfNarrowed,
  paused = [],
  busy,
  onSave,
  onCancel,
}: {
  host: string;
  policy: Accept;
  /**
   * Its agent declared `policy/1` (refuses devices that are not admins its
   * admin routes: local-aggregator §5.4, S1-4); `undefined` when the hub did
   * not say. `false`: nobody but admins may be let in — an older agent would
   * let them reach every route, self-upgrade included (the hub refuses to
   * route them there anyway).
   */
  declaresPolicy?: boolean;
  /** The devices that may be picked: current, not full devices (they always may), not the host itself — `limited`: a browser or phone that is a limited device. */
  candidates: { id: string; host: boolean; limited?: boolean }[];
  /** The chain holds a v3 entry (review D4): only then can this change. */
  upgraded: boolean;
  /** Hosts that must renew their identity key before another host may let them in (`CloudAccount.mustRenewFirst`). */
  mustRenew?: string[];
  /** This host renews its identity key here: a narrowing renews it (else: re-enroll it). */
  renewable?: boolean;
  /** The devices that run AI a narrowing would start renewing. */
  renewsIfNarrowed?: string[];
  /** The ids a renewal pause holds from controlling it now: shown apart from the list, which is what stays (§3.4 rule 10). */
  paused?: string[];
  busy: boolean;
  onSave: (from: Accept) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const [kind, setKind] = useState<Accept["type"]>(policy.type);
  const [picked, setPicked] = useState<Set<string>>(new Set(policy.type === "devices" ? policy.ids : []));
  const [confirming, setConfirming] = useState(false);
  const hostPicked = kind === "all" ? candidates.some((c) => c.host) : candidates.some((c) => c.host && picked.has(c.id));
  const from: Accept = kind === "devices" ? { type: "devices", ids: [...picked] } : { type: kind };
  const admitted = candidates.filter((c) => acceptsId(from, c.id) && !acceptsId(policy, c.id)).map((c) => c.id);
  const shutOut = candidates.filter((c) => acceptsId(policy, c.id) && !acceptsId(from, c.id)).map((c) => c.id);
  // The `policy/1` gate: widening who controls an agent that has no refusal
  // list — or one not known yet (`/v1/me` still loading: security review of S1-4, N1) — is refused here.
  const outdated = declaresPolicy !== true && admitted.length > 0;
  // A host that may have had its identity key copied is let in nowhere until it renewed it (docs/host-identity-rotation.md §7 decision 8).
  const exposed = admitted.filter((id) => mustRenew?.includes(id));
  return (
    <div data-testid={`host-control-${host}`} className="border-b border-border px-6 py-4 max-md:px-4">
      <div className="flex max-w-xl flex-col gap-2">
        <CapsLabel>{t("who can control this device")}</CapsLabel>
        {(
          [
            ["admins", t("only your full devices")],
            ["all", t("every device of the account")],
            ["devices", t("your full devices, and the devices picked here")],
          ] as const
        ).map(([value, label]) => (
          <label key={value} className="flex items-center gap-2 text-xs text-fg">
            <input type="radio" name={`accept-${host}`} checked={kind === value} disabled={!upgraded} onChange={() => setKind(value)} />
            {label}
          </label>
        ))}
        {kind === "devices" && (
          <ul className="ml-6 flex flex-col gap-1">
            {candidates.length === 0 && <li className="text-2xs text-faint">{t("no other device to pick")}</li>}
            {candidates.map((c) => (
              <li key={c.id}>
                <label className={cn(CHECKBOX_LABEL, "items-center font-mono text-2xs text-fg", !upgraded && "cursor-not-allowed opacity-60")}>
                  <input
                    type="checkbox"
                    className={CHECKBOX_INPUT}
                    disabled={!upgraded}
                    checked={picked.has(c.id)}
                    onChange={(e) => {
                      const next = new Set(picked);
                      if (e.target.checked) next.add(c.id);
                      else next.delete(c.id);
                      setPicked(next);
                    }}
                  />
                  <CheckboxMark />
                  <span className="min-w-0">
                    {c.id}
                    {c.host && <span className="text-faint">· {t("can run AI")}</span>}
                    {c.limited && <span className="text-faint">· {t("limited device")}</span>}
                  </span>
                </label>
              </li>
            ))}
          </ul>
        )}
        <p data-testid="control-full-access" className="text-2xs leading-4 text-fg">
          {t("Whoever may control this device has full access to it — its files, terminals, sessions and its own identity key. The routes it keeps for full devices only are a convenience, not a boundary.")}
        </p>
        <p data-testid="control-warning" data-signal={hostPicked ? "warn" : undefined} className={cn("text-2xs leading-4", hostPicked ? "text-warn-fg" : "text-muted")}>
          {t("Letting another device that runs AI control this one means that device — and every AI CLI on it — can drive this one.")}
        </p>
        {!upgraded && <p className="text-2xs leading-4 text-muted">{t("upgrade the account first")}</p>}
        {declaresPolicy === false ? (
          <p data-testid="control-outdated" data-signal={outdated ? "warn" : undefined} className={cn("text-2xs leading-4", outdated ? "text-warn-fg" : "text-muted")}>
            {t("Update Droppen on this device first: an older version lets every device that controls it use the routes it keeps for full devices, self-upgrade included. Until then only your full devices may control it.")}
          </p>
        ) : (
          outdated && (
            <p data-testid="control-outdated" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
              {t("Checking which Droppen this device runs: until then only your full devices may control it.")}
            </p>
          )
        )}
        {paused.length > 0 && (
          <p data-testid="control-paused" className="text-2xs leading-4 text-muted">
            {t("Paused while identity keys are renewed: {{ids}}. They come back when the pause ends; the list here is what stays.", { ids: paused.join(", ") })}
          </p>
        )}
        {exposed.length > 0 && (
          <p data-testid="control-exposed" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
            {t("Renew the identity key of {{ids}} first: a device that could control it no longer can, and may have copied it. Until then it may control no other device.", { ids: exposed.join(", ") })}
          </p>
        )}
        {confirming ? (
          <ControlConfirm
            host={host}
            admitted={admitted}
            shutOut={shutOut}
            renewable={renewable}
            renews={renewsIfNarrowed}
            busy={busy}
            onCancel={() => setConfirming(false)}
            onConfirm={() => onSave(from)}
          />
        ) : (
          <div className="flex gap-2">
            <button
              type="button"
              data-variant="primary"
              className={primary}
              disabled={busy || !upgraded || outdated || exposed.length > 0}
              onClick={() => (controlSaveStep({ admitted, shutOut }) === "confirm" ? setConfirming(true) : onSave(from))}
            >
              {busy && <Loader2 className="size-3.5 animate-spin" />}
              {t("save")}
            </button>
            <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
              {t("cancel")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
