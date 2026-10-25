import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download, Loader2, Share2 } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { browserHints, type DeviceKind, detectKind, type EnvironmentHints } from "@/features/landing/detect";
import { type Availability, AVAILABILITY, DOWNLOAD_PATH, DROPPEN_SITE_ORIGIN, INSTALL_COMMAND, WSL_SYSTEMD_URL } from "@/features/landing/get-droppen";
import { fileFacts, fileLink, useLatestRelease } from "@/features/landing/release-files";
import { publicHref } from "@/features/landing/route";
import { cn } from "@/lib/cn";
import { appHost } from "@/lib/hub/hosts";
import type { LatestRelease } from "@/lib/hub/releases";
import { CopyButton, primary, quiet, secondary } from "./ui";

/*
 * 「我的设备」's first-run card, while the account has no device that runs AI
 * (just signed up): the one thing to do next — Droppen on the device the AI
 * runs on — said for the device this page is open on (`detect.ts`), with its
 * first action right here:
 *
 * - **Mac**: the App's `.dmg` itself (`useLatestRelease`, the release page
 *   until the hub names it), then three steps: install it, open it, 「登录
 *   Droppen」 in the App (`APP_SIGN_IN_AVAILABLE`; off, in a preview, the
 *   background install's command instead);
 * - **Linux**: the one-line install command, its page's 「登录 Droppen」 —
 *   and, for a server with no browser, the one-time code;
 * - **Linux** on a Chromebook: the same, once its Linux is turned on;
 * - **Windows**: no app yet, said plainly, and what works: droppen.app in
 *   this browser once a Mac or Linux device is connected — WSL too, with
 *   systemd turned on (the installer runs Droppen as a systemd user service);
 * - **a phone or tablet**: Droppen goes on the device the AI runs on, so the
 *   link to open there (the download page, which opens on that device's way),
 *   shared or copied.
 *
 * Every one says first what that device needs: Claude Code or Codex,
 * installed and signed in. Another kind of device is a click away, and the
 * full guide (「添加可运行 AI 的设备」, `add-computer.tsx`) behind it. Nothing
 * here watches for the device: the page's list does, and the card goes once
 * the account has one.
 */

/** Which way onto a device the card shows. */
export type FirstHostWay = "mac" | "linux" | "windows" | "phone";

export const FIRST_HOST_WAYS: readonly FirstHostWay[] = ["mac", "linux", "windows", "phone"];

/** The way for a kind of device: a device it cannot tell gets the Mac's (as the download page opens on the Mac App), a Chromebook Linux's. */
export function firstHostWay(kind: DeviceKind): FirstHostWay {
  switch (kind) {
    case "linux":
    case "chromeos":
      return "linux";
    case "windows":
      return "windows";
    case "iphone":
    case "ipad":
    case "android":
      return "phone";
    case "mac":
    case "unknown":
      return "mac";
  }
}

/** The link a phone sends to the device the AI runs on: the download page, absolute (on the site host where the hub keeps one). */
export function setupLink(origin: string = globalThis.location?.origin ?? DROPPEN_SITE_ORIGIN): string {
  const href = publicHref(DOWNLOAD_PATH);
  return /^https?:\/\//.test(href) ? href : `${origin.replace(/\/+$/, "")}${href}`;
}

/** Whether this browser can hand a link to the system's share sheet. */
function canShareHere(): boolean {
  return typeof navigator !== "undefined" && typeof navigator.share === "function";
}

/** Numbered steps, short. */
function Steps({ items, testId }: { items: { title: string; body: string }[]; testId: string }) {
  return (
    <ol data-testid={testId} className="grid gap-x-6 gap-y-3 sm:grid-cols-3">
      {items.map((s, i) => (
        <li key={s.title} className="flex min-w-0 gap-2.5">
          <span className="font-mono text-2xs leading-5 text-faint tabular-nums">{String(i + 1).padStart(2, "0")}</span>
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="text-xs leading-5 font-medium text-fg">{s.title}</span>
            <span className="text-xs leading-5 text-muted">{s.body}</span>
          </span>
        </li>
      ))}
    </ol>
  );
}

/** A command to copy, its copy button the card's one primary action. */
function Command({ command }: { command: string }) {
  const { t } = useTranslation();
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <code data-testid="first-host-command" className="border border-border bg-raised px-2.5 py-2 font-mono text-2xs whitespace-pre-wrap text-fg select-all [overflow-wrap:anywhere]">
        {command}
      </code>
      <div className="flex">
        <CopyButton text={command} label={t("copy command")} primary testId="first-host-command-copy" />
      </div>
    </div>
  );
}

function MacWay({ availability, release }: { availability: Availability; release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  const app = appHost();
  if (!(availability.app && availability.appSignIn)) {
    // Off (a preview): the background install, which signs in from its own page.
    return (
      <div data-testid="first-host-mac" data-app="false" className="flex flex-col gap-4">
        <p className="text-xs leading-5 text-muted">{t("Open Terminal on that Mac and run:")}</p>
        <Command command={INSTALL_COMMAND} />
        <Steps
          testId="first-host-steps"
          items={[
            { title: t("Run the command"), body: t("It installs and starts Droppen, and makes a local login for it.") },
            { title: t("Sign in on its page"), body: t("On the page it opens, sign in with that login, then click Sign in to Droppen.") },
            { title: t("Add it"), body: t("A {{app}} window opens: sign in, then click add this device.", { app }) },
          ]}
        />
      </div>
    );
  }
  const dmg = release?.files.macos;
  return (
    <div data-testid="first-host-mac" data-app="true" className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {/* The file itself when the hub named it, downloaded in place; else the release page. */}
        <a {...fileLink(dmg)} data-variant="primary" data-testid="first-host-download" className={cn(primary, "max-md:w-full")}>
          <Download className="size-3.5" />
          {t("Download Droppen for Mac")}
        </a>
        <span data-testid="first-host-facts" className="font-mono text-2xs text-faint">
          {release && dmg ? `${fileFacts(release, dmg)} · ${t("macOS 13 or later")}` : t("macOS 13 or later, Apple silicon or Intel")}
        </span>
      </div>
      <Steps
        testId="first-host-steps"
        items={[
          { title: t("Install it"), body: t("Open the .dmg and drag Droppen to Applications.") },
          { title: t("Open Droppen"), body: t("It opens a short setup guide, four steps.") },
          {
            title: t("Sign in to Droppen in the App"),
            body: t(
              "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A {{app}} window opens: sign in, then click add this device.",
              { app },
            ),
          },
        ]}
      />
    </div>
  );
}

function LinuxWay({ onCode, chromebook }: { onCode?: () => void; chromebook: boolean }) {
  const { t } = useTranslation();
  return (
    <div data-testid="first-host-linux" className="flex flex-col gap-4">
      {chromebook && (
        <p data-testid="first-host-chromebook" className="text-xs leading-5 text-fg">
          {t("On a Chromebook, turn on Linux first: in Settings, search for “Linux development environment”.")}
        </p>
      )}
      <p className="text-xs leading-5 text-muted">{t("Run this in a terminal on that device:")}</p>
      <Command command={INSTALL_COMMAND} />
      <Steps
        testId="first-host-steps"
        items={[
          { title: t("Run the command"), body: t("It installs and starts Droppen, and makes a local login for it.") },
          { title: t("Sign in on its page"), body: t("On the page it opens, sign in with that login, then click Sign in to Droppen.") },
          { title: t("Add it"), body: t("A {{app}} window opens: sign in, then click add this device.", { app: appHost() }) },
        ]}
      />
      {onCode && (
        <p className="text-xs leading-5 text-muted">
          {t("No browser on that device, like a server?")}{" "}
          <button type="button" data-testid="first-host-code" className="text-fg underline underline-offset-2 hover:no-underline" onClick={onCode}>
            {t("use a one-time code")}
          </button>
        </p>
      )}
    </div>
  );
}

function WindowsWay({ onWay }: { onWay: (way: FirstHostWay) => void }) {
  const { t } = useTranslation();
  return (
    <div data-testid="first-host-windows" className="flex flex-col gap-3">
      <p className="text-xs leading-5 font-medium text-fg">{t("There is no Windows app yet.")}</p>
      <p className="text-xs leading-5 text-muted">
        {t("Your AI needs a Mac or a Linux device with Droppen on it (WSL works too, with systemd turned on). Once one is connected, use {{app}} right here in this browser to watch and answer it.", { app: appHost() })}{" "}
        <a href={WSL_SYSTEMD_URL} target="_blank" rel="noopener noreferrer" data-testid="first-host-wsl-systemd" className="text-fg underline underline-offset-2 hover:no-underline">
          {t("how to turn on systemd in WSL")}
        </a>
      </p>
      <div className="flex flex-wrap gap-2">
        <button type="button" data-testid="first-host-to-mac" className={secondary} onClick={() => onWay("mac")}>
          {t("set up a Mac")}
        </button>
        <button type="button" data-testid="first-host-to-linux" className={secondary} onClick={() => onWay("linux")}>
          {t("set up Linux or WSL")}
        </button>
      </div>
    </div>
  );
}

function PhoneWay({ link, share }: { link: string; share: boolean }) {
  const { t } = useTranslation();
  const [shareFailed, setShareFailed] = useState(false);
  const shareIt = () => {
    void navigator.share({ title: t("Set up Droppen"), url: link }).catch((error: unknown) => {
      // Closed by the person: nothing to say. Anything else: the copy button is beside it.
      if (!(error instanceof DOMException && error.name === "AbortError")) setShareFailed(true);
    });
  };
  return (
    <div data-testid="first-host-phone" className="flex flex-col gap-3">
      <p className="text-xs leading-5 font-medium text-fg">{t("Set it up on the device your AI runs on.")}</p>
      <p className="text-xs leading-5 text-muted">{t("Droppen goes on the Mac or Linux device where your AI runs, not on this one. Open this link there:")}</p>
      <code data-testid="first-host-link" className="border border-border bg-raised px-2.5 py-2 font-mono text-2xs break-all text-fg select-all">
        {link}
      </code>
      <div className="flex flex-wrap gap-2">
        {share && !shareFailed && (
          <button type="button" data-variant="primary" data-testid="first-host-share" className={primary} onClick={shareIt}>
            <Share2 className="size-3.5" />
            {t("share link")}
          </button>
        )}
        <CopyButton text={link} label={t("copy link")} primary={!share || shareFailed} testId="first-host-copy-link" />
      </div>
      <p className="text-xs leading-5 text-muted">{t("Once it is connected, you can watch and answer your AI from here too.")}</p>
    </div>
  );
}

/**
 * The card. `hints`, `release` and `share` stand in for the browser, the hub
 * and the share sheet (tests); `availability` for the build's switches.
 */
export function FirstHostCard({
  onGuide,
  onCode,
  pairWith,
  hints,
  release,
  share,
  availability = AVAILABILITY,
}: {
  /** Open the full 「添加可运行 AI 的设备」 guide. */
  onGuide: () => void;
  /** Open it on its one-time code (a device with no browser). */
  onCode?: () => void;
  /** The pairing that adds a browser or phone (`devices.tsx`), for a sign-in window whose browser is new to the account. */
  pairWith?: (onDone: () => void) => ReactNode;
  hints?: EnvironmentHints;
  release?: LatestRelease | null;
  share?: boolean;
  availability?: Availability;
}) {
  const { t } = useTranslation();
  const [kind] = useState<DeviceKind>(() => detectKind(hints ?? browserHints() ?? { userAgent: "" }));
  const detected = firstHostWay(kind);
  const [way, setWay] = useState<FirstHostWay>(detected);
  const [pairing, setPairing] = useState(false);
  // The App's file, as the hub names it: asked only while the card offers it.
  const latest = useLatestRelease(way === "mac" && availability.app && availability.appSignIn ? release : null);
  const [shareable] = useState(() => share ?? canShareHere());
  const label = (w: FirstHostWay) => (w === "mac" ? t("Mac") : w === "linux" ? t("Linux") : w === "windows" ? t("Windows") : t("phone or tablet"));
  return (
    <section data-testid="first-host" data-way={way} data-detected={detected} className="border-b border-border">
      <div className="flex max-w-[60rem] flex-col gap-4 px-6 py-6 max-md:px-4 max-md:py-5">
        <div className="flex flex-col gap-1.5">
          <CapsLabel>{t("next step")}</CapsLabel>
          <h2 className="text-[20px] leading-7 font-semibold tracking-[-0.02em] text-fg max-md:text-[18px]">{t("Connect the device your AI runs on")}</h2>
          <p className="max-w-[44rem] text-xs leading-5 text-muted">{t("Install Droppen on the Mac or Linux device where you use Claude Code or Codex. Then you can watch and answer your AI from here, and from your phone.")}</p>
          <p data-testid="first-host-needs" className="text-xs leading-5 text-fg">
            <span className="font-medium">{t("You need:")}</span> {t("Claude Code or Codex, installed and signed in on that device.")}
          </p>
        </div>
        {way === "mac" && <MacWay availability={availability} release={latest} />}
        {way === "linux" && <LinuxWay onCode={onCode} chromebook={kind === "chromeos"} />}
        {way === "windows" && <WindowsWay onWay={setWay} />}
        {way === "phone" && <PhoneWay link={setupLink()} share={shareable} />}
        {(way === "mac" || way === "linux" || way === "phone") && (
          <div className="flex flex-col gap-2">
            <p data-testid="first-host-waiting" className="flex items-center gap-2 text-2xs leading-4 text-muted">
              <Loader2 className="size-3 shrink-0 animate-spin text-faint" />
              {t("Keep this page open: it notices the new device by itself.")}
            </p>
            {pairWith &&
              (pairing ? (
                pairWith(() => setPairing(false))
              ) : (
                <p className="text-2xs leading-4 text-muted">
                  {way === "phone"
                    ? t("Droppen there opens a sign-in window. If it shows a code first, add it from here:")
                    : t("Is that window's browser new to your account? It shows a code first: add it from here.")}{" "}
                  <button type="button" data-testid="first-host-pair" className="text-fg underline underline-offset-2 hover:no-underline" onClick={() => setPairing(true)}>
                    {t("add a browser or phone")}
                  </button>
                </p>
              ))}
          </div>
        )}
        <div data-testid="first-host-ways" className="flex flex-wrap items-center gap-x-1 gap-y-1 border-t border-border pt-3 text-2xs text-muted">
          <span className="mr-1">{t("Another kind of device?")}</span>
          {FIRST_HOST_WAYS.filter((w) => w !== way).map((w) => (
            <button key={w} type="button" data-testid={`first-host-way-${w}`} className={cn(quiet, "h-7 px-2 text-2xs")} onClick={() => setWay(w)}>
              {label(w)}
            </button>
          ))}
          <span aria-hidden="true" className="mx-1">
            ·
          </span>
          <button type="button" data-testid="first-host-guide" className={cn(quiet, "h-7 px-2 text-2xs")} onClick={onGuide}>
            {t("all the ways to add one")}
          </button>
        </div>
      </div>
    </section>
  );
}
