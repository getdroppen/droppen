import { type ReactNode, useEffect, useMemo, useState } from "react";
import { Tabs as BaseTabs } from "@base-ui-components/react/tabs";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Download, ExternalLink, Laptop, Monitor, Server } from "lucide-react";
import { cn } from "@/lib/cn";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease } from "@/lib/hub/releases";
import { landingBundles } from "./copy";
import { browserCpu, browserHints, detectKind, type EnvironmentHints, initialOs, LINUX_CPUS, osForKind } from "./detect";
import {
  type Availability,
  AVAILABILITY,
  CHECKSUM_COMMAND,
  codeInstallCommand,
  DOWNLOAD_PATH,
  type DownloadOs,
  DROPPEN_SITE_ORIGIN,
  INSTALL_COMMAND,
  osOfSearch,
  RELEASES_URL,
  UNINSTALL_COMMAND,
  WSL_SYSTEMD_URL,
} from "./get-droppen";
import { cjkTracking, CommandLine, FooterLine, landingPrimary, landingSecondary, PublicHeader, Section, SectionHead } from "./page-kit";
import { fileFacts, fileLink, LinuxTarballs, useLatestRelease } from "./release-files";
import { appHost } from "@/lib/hub/hosts";
import { APP_PATH, publicLink, SECURITY_PATH, SIGN_IN_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

/*
 * droppen.app/download, in the order a customer needs it: where Droppen goes
 * (the computer Claude Code or Codex runs on; the phone needs nothing
 * installed) and what to have first (the AI tools, the system, an account);
 * the install for that computer; then the phone; then, for whoever approves
 * the install, what it puts on the computer and how to check the files; and
 * updating and uninstalling. Download pages elsewhere (Tailscale, ngrok,
 * Warp) are a platform, a button or numbered commands, and little else.
 *
 * The install is an ARIA tablist of the computers, Mac, Linux and Windows,
 * opened on the one this browser is on (`detect.ts`), marked "detected"
 * (a phone marks none: it installs nothing; a device it cannot tell opens on
 * the Mac, with nothing marked). The Mac's two ways are one tab, a switch
 * inside it:
 *
 * - **Mac · App** (the one people are pointed to): the Droppen App for macOS,
 *   universal: the latest release's `.dmg` itself, downloaded in place, with
 *   its version and size, as the hub names it (`release-files.tsx`; the
 *   release page until it does, or if it cannot), the release page beside it
 *   for its other files and checksums; open it and its setup guide opens,
 *   then 「登录 Droppen」 inside it — in that guide's step 02, later under
 *   Settings → Access → Droppen (`APP_SIGN_IN_AVAILABLE`; off, in a preview, a
 *   note that it is coming in an update, never a step for a button the App
 *   does not have). Until the first one is out (`APP_AVAILABLE`), "coming
 *   soon", and the background install as the way in today — never a link to a
 *   file that is not there;
 * - **Mac · background**: the install command, for a Mac that runs Droppen in
 *   the background or as a server;
 * - **Linux server**: the command and headless enrolment with a one-time code
 *   (`DROPPEN_CODE`), "coming" until `LINUX_AVAILABLE`;
 * - **Windows**: not yet, and what works from a PC today;
 *
 * The way is in the address (`?os=`), so it can be shared and linked (the
 * 「添加可运行 AI 的设备」 guide links its way); `?os=mobile` opens on the
 * phone's part. A choice made here is kept for the browser tab's session,
 * and the address wins over it. A public page in the landing page's
 * drafting-sheet language (its own lazy chunk), signed in or not; one
 * primary action, the chosen tab's.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

/** Where a tab chosen here is kept, for this browser tab's session (the address wins). */
const OS_KEY = "rh.download.os";

function rememberedOs(): DownloadOs | null {
  try {
    return osOfSearch(`?os=${sessionStorage.getItem(OS_KEY) ?? ""}`);
  } catch {
    return null;
  }
}

function rememberOs(os: DownloadOs): void {
  try {
    sessionStorage.setItem(OS_KEY, os);
  } catch {
    // nothing to keep it in: the address still says it
  }
}

/** Numbered steps, as hairline cells. */
function Steps({ items, testId }: { items: { title: string; body: ReactNode }[]; testId?: string }) {
  return (
    <ol data-testid={testId} className={cn("grid border-t border-border sm:grid-cols-2", items.length >= 4 ? "lg:grid-cols-4" : "lg:grid-cols-3")}>
      {items.map((s, i) => (
        <li
          key={s.title}
          className={cn(
            "flex min-w-0 flex-col gap-2 border-border px-6 py-5 max-md:px-4",
            "max-sm:border-b max-sm:last:border-b-0",
            "sm:max-lg:[&:nth-child(2n)]:border-l sm:max-lg:[&:not(:nth-last-child(-n+2))]:border-b",
            i > 0 && "lg:border-l",
          )}
        >
          <span className="text-xs font-semibold text-faint tabular-nums">{String(i + 1).padStart(2, "0")}</span>
          <span className="text-sm leading-5 font-semibold text-fg">{s.title}</span>
          <span className="text-xs leading-5 text-muted">{s.body}</span>
        </li>
      ))}
    </ol>
  );
}

/** A panel's lead: what this way gets you, and its one action. */
function PanelHead({ title, body, children }: { title: string; body: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col gap-4 px-6 py-6 max-md:px-4">
      <div className="flex max-w-[44rem] flex-col gap-2">
        <h2 className="text-xl leading-7 font-semibold tracking-tight text-fg">{title}</h2>
        <p className="text-sm leading-6 text-muted">{body}</p>
      </div>
      {children}
    </div>
  );
}

/** "Coming soon": said plainly, with where to go meanwhile (no label where the panel's lead already says it). */
function Soon({ label, body, children, testId }: { label?: string; body: string; children?: ReactNode; testId: string }) {
  return (
    <div data-testid={testId} className="flex max-w-[44rem] flex-col gap-3 border border-border bg-raised px-4 py-3.5">
      {label && <span className="inline-flex w-fit items-center border border-fg px-1.5 text-sm leading-4 font-semibold tracking-wider text-fg uppercase">{label}</span>}
      <p className="text-xs leading-5 text-fg">{body}</p>
      {children}
    </div>
  );
}

function AppPanel({ availability, onPick, release }: { availability: Availability; onPick: (os: DownloadOs) => void; release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  const dmg = release?.files.macos;
  const head = {
    title: t("Droppen for Mac"),
    body: availability.appSignIn
      ? t("Open the app and Droppen runs on this Mac, with no terminal and no local password. To use it from your phone and other devices, sign in to Droppen from the app.")
      : t("Open the app and Droppen runs on this Mac, with no terminal and no local password."),
  };
  if (!availability.app) {
    return (
      <PanelHead {...head}>
        <Soon testId="app-soon" label={t("The App is coming soon")} body={t("Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.")}>
          <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="install-command" primaryCopy />
          <button type="button" data-testid="to-macos-bg" onClick={() => onPick("macos-bg")} className="w-fit text-xs text-fg underline underline-offset-4 hover:no-underline">
            {t("the steps for running it in the background")}
          </button>
        </Soon>
      </PanelHead>
    );
  }
  return (
    <>
      <PanelHead {...head}>
        <div className="flex flex-wrap items-center gap-3">
          {/* The file itself when the hub named it, downloaded in place; else the release page. */}
          <a {...fileLink(dmg)} data-variant="primary" data-testid="app-download" className={landingPrimary}>
            <Download className="size-3.5" />
            {t("Download Droppen App (macOS)")}
          </a>
          <span data-testid="app-file-facts" className="text-xs font-semibold text-faint">
            {release && dmg
              ? `${fileFacts(release, dmg)} · ${t("macOS 13 or later · Universal: Apple silicon and Intel")}`
              : t("macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg")}
          </span>
          <a href={release?.releasesPage ?? RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="app-release-link" className="inline-flex items-center gap-1 text-xs font-semibold text-muted underline underline-offset-4 hover:text-fg">
            {t("release notes, checksums and other files")}
            <ExternalLink className="size-3" />
          </a>
        </div>
      </PanelHead>
      <Steps
        testId="app-steps"
        items={[
          { title: t("Open the .dmg"), body: t("The file you just downloaded, from Downloads.") },
          { title: t("Drag Droppen to Applications"), body: t("Like any other Mac app.") },
          // Where the App's sign-in is: its setup guide's step 02, then Settings — as 「我的设备」's first-run card says it (`first-host.tsx`).
          { title: t("Open Droppen"), body: t("It opens a short setup guide, four steps.") },
          ...(availability.appSignIn
            ? [
                {
                  title: t("Sign in to Droppen from the app"),
                  // The App signs in through the system browser, showing this device's fingerprint (`browser-sign-in.tsx`).
                  body: t(
                    "In the setup guide's Where to use it step, choose “On my phone and other devices too”, then click Sign in to Droppen. Your browser opens {{app}}: sign in, check that the fingerprint matches the one the app shows, then click add this device. Later, the same is under Settings → Access → Droppen.",
                    { app: appHost() },
                  ),
                },
              ]
            : []),
        ]}
      />
      {/* Off (`APP_SIGN_IN_AVAILABLE`, a preview): no step for a button the App does not have, and said plainly. */}
      {!availability.appSignIn && (
        <div className="border-t border-border px-6 py-5 max-md:px-4">
          <Soon
            testId="app-sign-in-soon"
            label={t("coming")}
            body={t("Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.")}
          />
        </div>
      )}
    </>
  );
}

function BackgroundPanel() {
  const { t } = useTranslation();
  return (
    <>
      <PanelHead title={t("Droppen in the background on a Mac")} body={t("For a Mac that stays on as a server, or if you prefer the terminal. One command installs Droppen; you then add this Mac to your account in the browser.")}>
        <div className="flex max-w-[44rem] flex-col gap-2">
          <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="install-command" primaryCopy />
          <p className="text-xs font-semibold text-faint">{t("Paste it into Terminal on that Mac.")}</p>
        </div>
      </PanelHead>
      <Steps
        testId="bg-steps"
        items={[
          // The local account as install.sh's `ensure_account` makes it: the system user name, a password asked for (12 characters at least), or generated when there is no terminal.
          {
            title: t("It installs and starts at login"),
            body: t("The script installs Droppen, starts it now and at every login, and creates the local page's first account under your user name. It asks you to choose a password (at least 12 characters), or generates one when run without a terminal."),
          },
          { title: t("The local page opens"), body: t("Your browser opens Droppen's local page. Sign in with that account: everything works there without a Droppen account. To use it from your phone, continue with the next step.") },
          { title: t("Sign in to Droppen"), body: t("On the local page, click Sign in to Droppen. A {{app}} window opens: sign in, check that its fingerprint matches the one on the local page, then click add this device.", { app: appHost() }) },
          { title: t("It is in my devices"), body: t("Within seconds it shows as online in my devices, on every phone and browser in your account. Add more devices the same way, and the sessions on all of them appear in one list.") },
        ]}
      />
    </>
  );
}

function LinuxPanel({
  availability,
  onPick,
  arch,
  release,
}: {
  availability: Availability;
  onPick: (os: DownloadOs) => void;
  arch: string | null;
  release: LatestRelease | null | undefined;
}) {
  const { t } = useTranslation();
  const head = {
    title: t("Droppen on a Linux server"),
    body: t("Run your AI on a server that stays on, and follow it and answer it from your phone. The server needs no browser: it joins your account with a one-time code."),
  };
  if (!availability.linux) {
    return (
      <PanelHead {...head}>
        <Soon testId="linux-soon" label={t("coming")} body={t("Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.")}>
          <button type="button" data-variant="primary" data-testid="to-macos-bg" onClick={() => onPick("macos-bg")} className={cn(landingPrimary, "w-fit")}>
            {t("run it in the background on a Mac")}
          </button>
        </Soon>
      </PanelHead>
    );
  }
  return (
    <>
      <PanelHead {...head}>
        {arch && (
          <p data-testid="linux-cpu" className="text-xs font-semibold text-faint">
            {t("This browser reports Linux on {{arch}}.", { arch })}
            {!(LINUX_CPUS as readonly string[]).includes(arch) && <> {t("Droppen's Linux builds are for x86_64 and aarch64.")}</>}
          </p>
        )}
        <div className="flex max-w-[44rem] flex-col gap-2">
          <CommandLine command={codeInstallCommand(t("<code>"))} label={t("copy the command")} testId="install-command" primaryCopy />
          <p className="text-xs leading-5 text-muted">{t("Replace <code> with the one-time code from step 01 below.")}</p>
          <LinuxTarballs release={release} cpu={arch} testId="linux-tarballs" sayUnbuilt={false} />
        </div>
      </PanelHead>
      <Steps
        testId="linux-steps"
        items={[
          { title: t("Get a one-time code"), body: t("On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.", { app: appHost() }) },
          // install.sh: a systemd user service, never root; it prints the linger command when the service would stop at logout.
          {
            title: t("Run the command on the server"),
            body: t("The script installs Droppen, starts it as a systemd user service under your own user, never as root, and joins your account with the code. If it says so at the end, run sudo loginctl enable-linger to keep it running after you log out."),
          },
          { title: t("It is in my devices"), body: t("Within seconds it shows as online in my devices, on every phone and browser in your account.") },
        ]}
      />
    </>
  );
}

function WindowsPanel({ availability, signedIn, onPick }: { availability: Availability; signedIn: boolean; onPick: (os: DownloadOs) => void }) {
  const { t } = useTranslation();
  const into = signedIn ? APP_PATH : SIGN_IN_PATH;
  return (
    <PanelHead title={t("Droppen on Windows")} body={t("Droppen does not run on Windows yet. From a browser on Windows you can already follow your AI and answer its questions.")}>
      <Soon testId="windows-soon" body={t("What works on Windows today:")}>
        <ul className="flex flex-col gap-2">
          {[
            { key: "browser", line: t("Control your other devices from this browser: sign in at {{app}}.", { app: appHost() }) },
            { key: "mac", line: availability.app ? t("Run the AI on a Mac, with the Droppen App.") : t("Run the AI on a Mac: in the background today, with the Droppen App soon.") },
            availability.linux
              ? {
                  key: "linux",
                  // The installer runs Droppen as a systemd user service: WSL has one only once systemd is turned on.
                  line: (
                    <>
                      {t("Run the AI on a Linux server, or in WSL with systemd turned on.")}{" "}
                      <a href={WSL_SYSTEMD_URL} target="_blank" rel="noopener noreferrer" data-testid="wsl-systemd" className="underline underline-offset-4 hover:no-underline">
                        {t("how to turn on systemd in WSL")}
                      </a>
                    </>
                  ),
                }
              : { key: "linux", line: t("Once Linux is supported: a Linux server, or WSL.") },
          ].map(({ key, line }) => (
            <li key={key} className="flex gap-2.5 text-xs leading-5 text-fg">
              <span className="mt-[7px] size-1.5 shrink-0 bg-fg" />
              <span>{line}</span>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap gap-2">
          <a {...publicLink(into)} data-variant="primary" data-testid="use-in-browser" className={landingPrimary}>
            {t("use {{app}} in this browser", { app: appHost() })}
          </a>
          <button type="button" data-testid="to-macos-app" onClick={() => onPick("macos-app")} className={landingSecondary}>
            {t("the Mac options")}
          </button>
          {availability.linux && (
            <button type="button" data-testid="to-linux" onClick={() => onPick("linux")} className={landingSecondary}>
              {t("the Linux server options")}
            </button>
          )}
        </div>
      </Soon>
    </PanelHead>
  );
}

/** After the install: the phone needs nothing installed, only the browser, approved from a device already in the account. */
function PhoneSection({ signedIn }: { signedIn: boolean }) {
  const { t } = useTranslation();
  const into = signedIn ? APP_PATH : SIGN_IN_PATH;
  return (
    <Section id="phone">
      <SectionHead title={t("Then, use it from your phone")} />
      <Steps
        testId="phone-steps"
        items={[
          { title: t("Open {{app}} on your phone", { app: appHost() }), body: t("Open {{app}} in your phone's browser and sign in to the same account.", { app: appHost() }) },
          {
            title: t("Approve the phone"),
            body: t("On a device already in your account, open my devices → add a browser or phone, then scan the QR code the phone shows, or enter its number code."),
          },
          {
            title: t("Add it to the home screen"),
            body: t("iPhone and iPad: in Safari, tap Share, then Add to Home Screen. Android: in Chrome, tap ⋮, then Add to Home screen or Install app. Then you can turn on notifications."),
          },
        ]}
      />
      <div className="border-t border-border px-6 py-5 max-md:px-4">
        <a {...publicLink(into)} data-testid="open-droppen" className={cn(landingSecondary, "w-fit")}>
          {t("open {{app}}", { app: appHost() })}
        </a>
      </div>
    </Section>
  );
}

/** Each tab's icon and name: the computers Droppen installs on (the phone needs nothing installed: `PhoneSection`). */
function useTabs(): { os: DownloadOs; label: string; icon: typeof Laptop }[] {
  const { t } = useTranslation();
  return [
    { os: "macos-app", label: t("Mac"), icon: Laptop },
    { os: "linux", label: t("Linux"), icon: Server },
    { os: "windows", label: t("Windows"), icon: Monitor },
  ];
}

/** The tab a way is under: both Mac ways under the Mac's, and a phone's visitor on the Mac's (it installs on a computer). */
function tabOf(os: DownloadOs): DownloadOs {
  return os === "macos-bg" || os === "mobile" ? "macos-app" : os;
}

/** On the Mac tab, its two ways: the App, and the command line (a Mac that stays on, or a terminal user). */
function MacWays({ os, onPick }: { os: DownloadOs; onPick: (os: DownloadOs) => void }) {
  const { t } = useTranslation();
  const ways = [
    { os: "macos-app" as const, label: t("Mac app (recommended)") },
    { os: "macos-bg" as const, label: t("Command line") },
  ];
  const on = os === "macos-bg" ? "macos-bg" : "macos-app";
  return (
    <div role="group" aria-label={t("How to install on a Mac")} data-testid="mac-ways" className="flex w-fit border border-border-strong">
      {ways.map((w) => (
        <button
          key={w.os}
          type="button"
          aria-pressed={w.os === on}
          data-testid={`way-${w.os}`}
          onClick={() => onPick(w.os)}
          className={cn("h-8 px-3 text-xs whitespace-nowrap transition-colors", w.os === on ? "bg-fg text-bg" : "text-muted hover:text-fg")}
        >
          {w.label}
        </button>
      ))}
    </div>
  );
}

/** The computers, as a tablist: the detected one marked, the chosen one inked in. */
function Environments({
  os,
  detected,
  onPick,
  availability,
  signedIn,
  arch,
  release,
}: {
  os: DownloadOs;
  /** The detected device's way; `null` when the device cannot be told. A phone's marks no tab: it installs nothing. */
  detected: DownloadOs | null;
  onPick: (os: DownloadOs) => void;
  availability: Availability;
  signedIn: boolean;
  arch: string | null;
  release: LatestRelease | null | undefined;
}) {
  const { t } = useTranslation();
  const tabs = useTabs();
  const marked = detected && detected !== "mobile" ? tabOf(detected) : null;
  return (
    <BaseTabs.Root
      value={tabOf(os)}
      onValueChange={(next) => onPick(next === "macos-app" && os === "macos-bg" ? "macos-bg" : (next as DownloadOs))}
      data-testid="environments"
      data-os={os}
      data-detected={detected ?? undefined}
    >
      <BaseTabs.List aria-label={t("Choose your system")} activateOnFocus className="flex gap-8 border-b border-border px-6 max-md:gap-6 max-md:px-4">
        {tabs.map(({ os: value, label, icon: Icon }) => (
          <BaseTabs.Tab
            key={value}
            value={value}
            data-testid={`os-${value}`}
            className="group relative flex h-12 shrink-0 items-center gap-2 text-sm text-muted outline-none hover:text-fg focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent data-[active]:font-semibold data-[active]:text-fg"
          >
            <Icon aria-hidden="true" className="size-4 shrink-0" />
            {label}
            {value === marked && (
              <span data-testid="detected" className="border border-border px-1.5 text-xs font-semibold leading-4 font-normal text-muted">
                {t("Detected")}
              </span>
            )}
            {/* The chosen tab's rule. */}
            <span aria-hidden="true" className="absolute inset-x-0 -bottom-px hidden h-0.5 bg-fg group-data-[active]:block" />
          </BaseTabs.Tab>
        ))}
      </BaseTabs.List>
      {tabs.map(({ os: value }) => (
        <BaseTabs.Panel key={value} value={value} data-testid={`panel-${value}`} className="outline-none">
          {value === "macos-app" && (
            <>
              {availability.app && (
                <div className="px-6 pt-6 max-md:px-4">
                  <MacWays os={os} onPick={onPick} />
                </div>
              )}
              <div data-testid={`way-panel-${os === "macos-bg" ? "macos-bg" : "macos-app"}`}>
                {os === "macos-bg" ? <BackgroundPanel /> : <AppPanel availability={availability} onPick={onPick} release={release} />}
              </div>
            </>
          )}
          {value === "linux" && <LinuxPanel availability={availability} onPick={onPick} arch={arch} release={release} />}
          {value === "windows" && <WindowsPanel availability={availability} signedIn={signedIn} onPick={onPick} />}
        </BaseTabs.Panel>
      ))}
    </BaseTabs.Root>
  );
}

/** What to have before installing, under the page's lead: the AI tools, the system, an account (or the way back in). */
function Needs({ signedIn, signup }: { signedIn: boolean; signup: SignupMode }) {
  const { t } = useTranslation();
  const link = "whitespace-nowrap text-fg underline underline-offset-4 hover:no-underline";
  const account = signedIn ? (
    <>
      {t("You are signed in.")}{" "}
      <a {...publicLink("/devices")} data-testid="download-open-devices" className={link}>
        {t("Open my devices")}
      </a>
    </>
  ) : signup === "closed" ? (
    <>
      {t("A Droppen account. Sign-up is closed for now.")}{" "}
      <a {...publicLink(WAITLIST_HREF)} data-testid="download-waitlist" className={link}>
        {t("Join the waitlist")}
      </a>
    </>
  ) : signup === "invite" ? (
    <>
      {t("A Droppen account. Sign-up needs an invite code.")}{" "}
      <a {...publicLink(SIGN_UP_PATH)} data-testid="download-signup" className={link}>
        {t("Sign up")}
      </a>
    </>
  ) : (
    <>
      {t("A free Droppen account. You can also create one when you sign in to Droppen on the device.")}{" "}
      <a {...publicLink(SIGN_UP_PATH)} data-testid="download-signup" className={link}>
        {t("Create a free account")}
      </a>
    </>
  );
  const items: { key: string; body: ReactNode }[] = [
    { key: "tools", body: t("Claude Code or Codex, installed and signed in on that device") },
    { key: "system", body: t("macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64, glibc 2.28 or later)") },
    { key: "account", body: account },
  ];
  return (
    <div data-testid="download-needs" className="flex flex-col gap-2.5">
      <p className="text-sm leading-6 font-semibold text-fg">{t("Before you install:")}</p>
      <ul className="flex flex-col gap-2">
        {items.map((item) => (
          <li key={item.key} className="flex gap-2.5 text-sm leading-6 text-pretty text-muted">
            <span className="mt-[9px] size-1.5 shrink-0 bg-fg" />
            <span>{item.body}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** For whoever approves the install: what it puts on the computer, how it runs, and how to check the files. */
function Verify({ availability }: { availability: Availability }) {
  const { t } = useTranslation();
  const rows: { term: string; body: ReactNode }[] = [
    ...(availability.app ? [{ term: t("Mac app"), body: t("Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.") }] : []),
    { term: t("Install script"), body: t("Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.") },
    // install.sh: `--system` is Linux only, still runs as you, and `sctl` uses sudo for that service alone. The App's own port is not 8700 (`desktop/core/src/identity.rs`).
    {
      term: t("Background service"),
      body: t("For a command-line install: a launchd user agent on macOS, a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo is used only to set up and manage that service."),
    },
    {
      term: t("Network"),
      body: t("Listens only on 127.0.0.1 and reaches Droppen over outbound connections, so there is no inbound port to open. A command-line install uses port 8700, or the next free port."),
    },
    {
      term: t("Manual download"),
      body: (
        <span className="flex flex-col gap-2">
          <a href={RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="releases-link" className="inline-flex w-fit items-center gap-1 text-fg underline underline-offset-4 hover:no-underline">
            {t("the latest release on GitHub (getdroppen/droppen)")}
            <ExternalLink className="size-3" />
          </a>
          <span>{t("Before you open the .dmg or extract the .tar.gz, check it in a terminal:")}</span>
          <CommandLine command={CHECKSUM_COMMAND} label={t("copy")} />
          <span>{t("The checksum it prints must match that file's line in the release's SHA256SUMS; if it does not, do not install it. The install script runs the same check before it installs anything.")}</span>
        </span>
      ),
    },
    {
      term: t("Encryption"),
      body: (
        <span className="flex flex-col gap-2">
          <span>{t("End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.")}</span>
          <a {...publicLink(SECURITY_PATH)} data-testid="download-security" className="w-fit text-fg underline underline-offset-4 hover:no-underline">
            {t("How the encryption works")}
          </a>
        </span>
      ),
    },
  ];
  return (
    <Section id="verify">
      <div className="grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.55fr)]">
        <SectionHead title={t("What gets installed, and how to verify it")} className="lg:border-r lg:border-border" />
        <dl data-testid="download-verify" className="flex flex-col max-lg:border-t max-lg:border-border">
          {rows.map((r) => (
            <div key={r.term} className="grid grid-cols-[10rem_minmax(0,1fr)] items-baseline gap-4 border-b border-border px-6 py-3.5 last:border-b-0 max-md:grid-cols-1 max-md:gap-1 max-md:px-4">
              <dt className="text-sm leading-5 font-semibold text-fg">{r.term}</dt>
              <dd className="min-w-0 text-xs leading-5 text-muted">{r.body}</dd>
            </div>
          ))}
        </dl>
      </div>
    </Section>
  );
}

function Uninstall({ availability }: { availability: Availability }) {
  const { t } = useTranslation();
  // Every key it held is replaced (the security page's 2.8); removal stops its link but leaves its files (`cloud/control.rs`): the disconnect deletes them.
  const steps = [
    { title: t("Remove it from your account"), body: t("On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.", { app: appHost() }) },
    { title: t("Disconnect it"), body: t("On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. It deletes the Droppen keys stored there, which removing it from the account leaves in place.") },
  ];
  return (
    <Section id="uninstall">
      <SectionHead
        title={t("Updating and uninstalling")}
        lead={
          availability.app
            ? t("The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.")
            : t("To update, run the install command again: it upgrades in place and keeps your data.")
        }
      />
      <ol className="grid border-t border-border md:grid-cols-3">
        {steps.map((s, i) => (
          <li key={s.title} className="flex flex-col gap-2 border-border px-6 py-6 max-md:border-b max-md:px-4 md:border-r">
            <span className="text-xs font-semibold text-faint tabular-nums">0{i + 1}</span>
            <span className="text-sm leading-5 font-semibold text-fg">{s.title}</span>
            <span className="text-xs leading-5 text-muted">{s.body}</span>
          </li>
        ))}
        <li className="flex min-w-0 flex-col gap-2 px-6 py-6 max-md:px-4">
          <span className="text-xs font-semibold text-faint tabular-nums">03</span>
          <span className="text-sm leading-5 font-semibold text-fg">{t("Remove the program")}</span>
          {availability.app && <span className="text-xs leading-5 text-muted">{t("The App: quit Droppen and drag it from Applications to the Trash.")}</span>}
          <span className="text-xs leading-5 text-muted">
            {availability.app ? t("Command-line install (Mac or Linux): run this on that device, as the user who installed it:") : t("Run this on that device, as the user who installed it:")}
          </span>
          <CommandLine command={UNINSTALL_COMMAND} label={t("copy")} testId="uninstall-command" />
          {/* install.sh's `--uninstall`: the service and the program go, the data stays, and it prints where that is and how to delete it. */}
          <span className="text-xs leading-5 text-muted">{t("It stops and removes the service and the program and keeps your data; at the end it prints where the data is, and the command that deletes it.")}</span>
        </li>
      </ol>
    </Section>
  );
}

/** This page's address, to open on another device. */
function pageAddress(): string {
  return `${typeof location === "undefined" ? DROPPEN_SITE_ORIGIN : location.origin}${DOWNLOAD_PATH}`;
}

/** The page's title and description in the reader's language. */
function useDocumentMeta() {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("Download Droppen");
    document.documentElement.lang = active.language;
    const meta = document.querySelector('meta[name="description"]');
    const description = meta?.getAttribute("content") ?? null;
    meta?.setAttribute("content", t("Install Droppen on the Mac or Linux device where Claude Code or Codex runs: the Mac app, or one command. Your phone needs only the browser."));
    return () => {
      document.title = before;
      if (meta && description !== null) meta.setAttribute("content", description);
    };
  }, [t, active.language]);
}

/**
 * The tab, as the address says, else as chosen in this session, else as
 * detected — kept in the address (`?os=`) and the session as it changes,
 * and following Back and Forward.
 */
function useChosenOs(detected: DownloadOs | null): [DownloadOs, (os: DownloadOs) => void] {
  const [os, setOs] = useState<DownloadOs>(() => initialOs({ search: typeof location === "undefined" ? "" : location.search, remembered: rememberedOs(), detected }));
  useEffect(() => {
    const popped = () => {
      const named = osOfSearch(location.search);
      if (named) setOs(named);
    };
    addEventListener("popstate", popped);
    return () => removeEventListener("popstate", popped);
  }, []);
  const pick = (next: DownloadOs) => {
    setOs(next);
    rememberOs(next);
    if (typeof history === "undefined" || osOfSearch(location.search) === next) return;
    // The address names the tab (`downloadHref`); anything else it carries stays.
    const params = new URLSearchParams(location.search);
    params.set("os", next);
    history.replaceState(history.state, "", `${location.pathname}?${params}${location.hash}`);
  };
  return [os, pick];
}

/** droppen.app/download, signed in or not. `availability`, `hints`, `cpu` and `release` stand in for the build's switches, the browser and the hub (tests, previews). */
export default function DownloadPage({
  signedIn = false,
  availability = AVAILABILITY,
  hints,
  cpu,
  release,
  signup = "open",
}: {
  signedIn?: boolean;
  availability?: Availability;
  hints?: EnvironmentHints;
  cpu?: string | null;
  release?: LatestRelease | null;
  /** The hub's sign-up mode (`signup-mode.ts`): by invite, the way to sign up names the invite code; closed, it offers the waitlist. */
  signup?: SignupMode;
}) {
  const { t } = useTranslation();
  useDocumentMeta();
  const detected = useMemo(() => osForKind(detectKind(hints ?? browserHints() ?? { userAgent: "" })), [hints]);
  const [os, pick] = useChosenOs(detected);
  // Linux's processor, said on its tab — only when this browser is on Linux (from the high-entropy hints, else the user agent): a Mac's says nothing about a server.
  const onLinux = detected === "linux";
  const [arch, setArch] = useState<string | null>(cpu ?? null);
  const latest = useLatestRelease(release);
  // Sent here for the phone (`?os=mobile`): the part about the phone.
  useEffect(() => {
    if (os === "mobile") document.getElementById("phone")?.scrollIntoView({ block: "start" });
    // Once, as the page opens: picking a tab later scrolls nothing.
  }, []);
  useEffect(() => {
    if (os !== "linux" || !onLinux || hints) return;
    let live = true;
    void browserCpu().then((value) => live && setArch(value));
    return () => {
      live = false;
    };
  }, [os, onLinux, hints]);
  return (
    <div data-testid="download" className="h-full w-full overflow-y-auto bg-bg text-base [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1360px] md:border-x md:border-border">
          <div className="flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
            <h1 className="max-w-[18em] text-5xl leading-display font-normal tracking-tight text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-4xl xl:text-5xl">
              {t("Install Droppen on the device that runs your AI.")}
            </h1>
            <p className="max-w-[40em] text-base leading-6 text-pretty text-muted [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-sm max-md:leading-6">
              {t(
                "Droppen goes on the Mac or Linux device where Claude Code or Codex runs, alongside them. Your phone and other devices need nothing installed: open {{app}} in the browser to see every session, answer your AI's questions, steer it and open a terminal.",
                { app: appHost() },
              )}
            </p>
            {/* Read on a phone: it installs nothing; the address to open on the computer. */}
            {detected === "mobile" && (
              <div data-testid="on-a-phone" className="flex max-w-[44rem] flex-col gap-2">
                <p className="text-sm leading-6 text-fg">{t("You are on a phone. To install, open this page on the device that runs your AI:")}</p>
                <CommandLine command={pageAddress()} label={t("copy")} testId="page-address" />
              </div>
            )}
            <div className="mt-3">
              <Needs signedIn={signedIn} signup={signup} />
            </div>
          </div>
          <Section id="get">
            <Environments os={os} detected={detected} onPick={pick} availability={availability} signedIn={signedIn} arch={onLinux ? arch : null} release={latest} />
          </Section>
          <PhoneSection signedIn={signedIn} />
          <Verify availability={availability} />
          <Uninstall availability={availability} />
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
