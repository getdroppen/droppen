/*
 * The rotation report's words and rendering (docs/host-identity-rotation.md
 * §4.4): a report's lines, with a machine whose new key the keyring did not
 * take said loudly. One renderer for every page that reports a rotation.
 */

import { type RotationReport } from "@/lib/hub/account";
import { hostRefusalText } from "@/api/errors";
import { cn } from "@/lib/cn";
import { danger, knownText } from "./ui";

function mailWhy(code: string | undefined, t: (key: string, opts?: Record<string, unknown>) => string): string {
  if (code === "mailbox_full") return t("its mailbox at the hub is full");
  if (code === "mail_rate") return t("the hub is holding back this device's mail for a minute");
  return code ?? "";
}

/** Whether a report line is one to say loudly: a machine whose new key the keyring did not take. */
function alarmLine(report: RotationReport, line: string): boolean {
  // The machine's line, in any language: Chinese and Japanese put a full-width colon after its id.
  return report.machines.some((m) => m.keyring && (line.startsWith(`${m.machine}:`) || line.startsWith(`${m.machine}：`)) && line.includes(m.keyring));
}

/** A rotation report's lines; `alarm` marks the machine whose new key the keyring did not take (`UpgradeAccount` never did). */
export function ReportLines({ report, t, alarm = true }: { report: RotationReport; t: (key: string, opts?: Record<string, unknown>) => string; alarm?: boolean }) {
  return (
    <>
      {reportLines(report, t).map((line, i) => {
        const alarmHere = alarm && alarmLine(report, line);
        return (
          <span key={i} className={cn("font-mono text-2xs", alarmHere ? "text-danger" : "text-muted")} data-signal={alarmHere ? "alarm" : undefined}>
            {line}
          </span>
        );
      })}
    </>
  );
}

function reportLines(report: RotationReport, t: (key: string, opts?: Record<string, unknown>) => string): string[] {
  const lines =
    report.ukEpoch === null
      ? report.machines.length
        ? [t("new content keys only: the removed device held no user key")]
        : [t("no keys to rotate: the removed device held none but its own")]
      : [t("new user key (epoch {{n}}) sent to {{count}} device(s)", { n: report.ukEpoch, count: report.sealedTo.length })];
  if (report.hostKeysTo?.length) lines.push(t("new content keys sealed for {{count}} device(s) that hold no user key", { count: report.hostKeysTo.length }));
  const hostFailed = Object.keys(report.hostKeysFailed ?? {});
  if (hostFailed.length) lines.push(t("could not reach: {{ids}}", { ids: hostFailed.map((id) => `${id} (${mailWhy(report.hostKeysFailed![id], t)})`).join(", ") }));
  if (report.failedTo.length) {
    lines.push(t("could not reach: {{ids}}", { ids: report.failedTo.map((id) => (report.failedWhy[id] ? `${id} (${mailWhy(report.failedWhy[id], t)})` : id)).join(", ") }));
    lines.push(t("send it again from here with “re-send keys” once that is fixed"));
  }
  if (report.recovery) lines.push(t("and sealed for the recovery words"));
  for (const m of report.machines) {
    if (m.error) lines.push(t("{{machine}}: {{error}}", { machine: m.machine, error: knownText(m.error) }));
    else if (m.result?.via === "channel") lines.push(t("{{machine}}: new content key handed over end to end", { machine: m.machine }));
    else if (m.result?.via === "mailbox") {
      const { reason, pending } = m.result;
      lines.push(
        reason === "offline"
          ? t("{{machine}}: offline — the new content key waits in its mailbox", { machine: m.machine })
          : reason === "unconfirmed"
            ? t("{{machine}}: could not open a fresh channel yet (waiting for another device to confirm the change) — the new content key waits in its mailbox", { machine: m.machine })
            : t("{{machine}}: the channel failed — the new content key waits in its mailbox", { machine: m.machine }),
      );
      if (pending) lines.push(t("{{machine}}: re-encryption runs when this device next connects to it", { machine: m.machine }));
    }
    else if (m.result?.via === "refused") {
      lines.push(t("{{machine}}: refused the new key ({{message}})", { machine: m.machine, message: hostRefusalText(m.result.code) ?? m.result.message }));
    }
    if (m.keyring) {
      lines.push(
        t("{{machine}}: NOT IN THE KEYRING ({{why}}) — the new key is in use, but other devices cannot read what it writes from now on. Make room, then rotate the keys again.", {
          machine: m.machine,
          why: m.keyring,
        }),
      );
    }
  }
  if (report.vetoed?.devices.length) lines.push(t("also removed what it brought in: {{ids}}", { ids: report.vetoed.devices.join(", ") }));
  if (report.vetoed?.machines.length) lines.push(t("and the devices it added: {{ids}}", { ids: report.vetoed.machines.join(", ") }));
  return lines;
}
