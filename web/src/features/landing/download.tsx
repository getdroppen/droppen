import { type ReactNode, useEffect, useMemo, useState } from "react";
import { Tabs as BaseTabs } from "@base-ui-components/react/tabs";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { AppWindow, Download, ExternalLink, Laptop, Monitor, Server, Smartphone } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { primary, secondary } from "@/features/cloud/ui";
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
  DOWNLOAD_OSES,
  DOWNLOAD_PATH,
  type DownloadOs,
  DROPPEN_ORIGIN,
  INSTALL_COMMAND,
  osOfSearch,
  RELEASES_URL,
  UNINSTALL_COMMAND,
} from "./get-droppen";
import { CommandLine, FooterLine, PublicHeader, Section, SectionHead } from "./page-kit";
import { fileFacts, fileLink, LinuxTarballs, useLatestRelease } from "./release-files";
import { appHost } from "@/lib/hub/hosts";
import { APP_PATH, publicLink, SIGN_IN_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

/*
 * droppen.app/download: how Droppen gets onto a device, the way other
 * vendors' download pages work. The page reads which device it is on
 * (`detect.ts`), opens on the way that fits it — marked "detected" — and
 * every other way is one click away, in an ARIA tablist (a device it cannot
 * tell opens on the Mac App, with nothing marked):
 *
 * - **Mac · App** (the one people are pointed to): the Droppen App for macOS,
 *   universal: the latest release's `.dmg` itself, downloaded in place, with
 *   its version and size, as the hub names it (`release-files.tsx`; the
 *   release page until it does, or if it cannot), the release page beside it
 *   for its other files and checksums; open it and Droppen starts, 「登录 Droppen」 inside it once the
 *   App can (`APP_SIGN_IN_AVAILABLE`; until then, a note that it is coming in
 *   an update, never a step for a button the App does not have). Until the
 *   first one is out (`APP_AVAILABLE`), "coming soon", and the background
 *   install as the way in today — never a link to a file that is not there;
 * - **Mac · background**: the install command, for a Mac that runs Droppen in
 *   the background or as a server;
 * - **Linux server**: the command and headless enrolment with a one-time code
 *   (`DROPPEN_CODE`), "coming" until `LINUX_AVAILABLE`;
 * - **Windows**: not yet, and what works from a PC today;
 * - **Phone / tablet**: Droppen there is the remote control — droppen.app on
 *   the home screen — and this page's address, to open on the computer.
 *
 * The tab is in the address (`?os=`), so it can be shared and linked (the
 * 「添加可运行 AI 的设备」 guide links its tab); a choice made here is kept for
 * the browser tab's session, and the address wins over it. Then what it
 * runs on, the manual download with its checksums, and how to take it off.
 * A public page in the landing page's drafting-sheet language (its own lazy
 * chunk), signed in or not; one primary action, the chosen tab's.
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
          <span className="font-mono text-2xs text-faint tabular-nums">{String(i + 1).padStart(2, "0")}</span>
          <span className="text-[13px] leading-5 font-semibold text-fg">{s.title}</span>
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
        <h2 className="text-[20px] leading-7 font-semibold tracking-[-0.02em] text-fg">{title}</h2>
        <p className="text-sm leading-6 text-muted">{body}</p>
      </div>
      {children}
    </div>
  );
}

/** "Coming soon": said plainly, with where to go meanwhile. */
function Soon({ label, body, children, testId }: { label: string; body: string; children?: ReactNode; testId: string }) {
  return (
    <div data-testid={testId} className="flex max-w-[44rem] flex-col gap-3 border border-border bg-raised px-4 py-3.5">
      <span className="inline-flex w-fit items-center border border-fg px-1.5 font-mono text-[10px] leading-4 font-semibold tracking-wider text-fg uppercase">{label}</span>
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
      ? t("Open the app and Droppen is running on your Mac: no terminal, no login page. Then sign it in to Droppen from inside the app.")
      : t("Open the app and Droppen is running on your Mac: no terminal, no login page."),
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
          <a {...fileLink(dmg)} data-variant="primary" data-testid="app-download" className={primary}>
            <Download className="size-3.5" />
            {t("Download Droppen App (macOS)")}
          </a>
          <span data-testid="app-file-facts" className="font-mono text-2xs text-faint">
            {release && dmg
              ? `${fileFacts(release, dmg)} · ${t("macOS 13 or later · Universal: Apple silicon and Intel")}`
              : t("macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg")}
          </span>
          <a href={release?.releasesPage ?? RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="app-release-link" className="inline-flex items-center gap-1 font-mono text-2xs text-muted underline underline-offset-4 hover:text-fg">
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
          { title: t("Open Droppen"), body: t("It starts by itself and opens its own window. Nothing to type.") },
          ...(availability.appSignIn
            ? [{ title: t("Click Sign in to Droppen in the app"), body: t("A {{app}} window opens: sign in, check the fingerprint matches, and add the device. It is in my devices within seconds.", { app: appHost() }) }]
            : []),
        ]}
      />
      {/* Not built yet (`APP_SIGN_IN_AVAILABLE`): no step for a button the App does not have, and said plainly. */}
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
      <PanelHead title={t("Droppen in the background on a Mac")} body={t("For a Mac that stays on as a server, or if you would rather use the terminal: one line, then everything else happens in the browser.")}>
        <div className="flex max-w-[44rem] flex-col gap-2">
          <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="install-command" primaryCopy />
          <p className="font-mono text-2xs text-faint">{t("Paste it into Terminal on that Mac.")}</p>
        </div>
      </PanelHead>
      <Steps
        testId="bg-steps"
        items={[
          { title: t("It installs and starts at login"), body: t("The script installs Droppen, starts it now and at every login, and creates your first local login: it prints it, or asks you to choose one.") },
          { title: t("Its page opens"), body: t("Your browser opens Droppen on that device. Sign in with the local login: everything already works there, with no account.") },
          { title: t("Sign in to Droppen"), body: t("Click Sign in to Droppen on that page. A {{app}} window opens: sign in, check the fingerprint matches, and add the device.", { app: appHost() }) },
          { title: t("It is in my devices"), body: t("Within seconds it is online in my devices, on every phone and browser of your account. Add another device the same way, and put both on one screen.") },
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
    body: t("Run your AI on a server that never sleeps, and watch it from your phone. No browser needed on the server: it joins your account with a one-time code."),
  };
  if (!availability.linux) {
    return (
      <PanelHead {...head}>
        <Soon testId="linux-soon" label={t("coming")} body={t("Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.")}>
          <button type="button" data-variant="primary" data-testid="to-macos-bg" onClick={() => onPick("macos-bg")} className={cn(primary, "w-fit")}>
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
          <p data-testid="linux-cpu" className="font-mono text-2xs text-faint">
            {t("This browser reports Linux on {{arch}}.", { arch })}
            {!(LINUX_CPUS as readonly string[]).includes(arch) && <> {t("Droppen's Linux builds are for x86_64 and aarch64.")}</>}
          </p>
        )}
        <div className="flex max-w-[44rem] flex-col gap-2">
          <CommandLine command={codeInstallCommand(t("<code>"))} label={t("copy the command")} testId="install-command" primaryCopy />
          <p className="text-xs leading-5 text-muted">{t("Replace <code> with a one-time code: on {{app}}, open my devices → add a device that runs AI → show a one-time code.", { app: appHost() })}</p>
          <LinuxTarballs release={release} cpu={arch} testId="linux-tarballs" sayUnbuilt={false} />
        </div>
      </PanelHead>
      <Steps
        testId="linux-steps"
        items={[
          { title: t("Get a one-time code"), body: t("On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.", { app: appHost() }) },
          { title: t("Run the command on the server"), body: t("It installs Droppen, starts it as a service, and joins your account with the code.") },
          { title: t("It is in my devices"), body: t("Within seconds it is online in my devices, on every phone and browser of your account.") },
        ]}
      />
    </>
  );
}

function WindowsPanel({ availability, signedIn, onPick }: { availability: Availability; signedIn: boolean; onPick: (os: DownloadOs) => void }) {
  const { t } = useTranslation();
  const into = signedIn ? APP_PATH : SIGN_IN_PATH;
  return (
    <PanelHead title={t("Droppen on Windows")} body={t("Not supported yet: it is on its way. On Windows you can already watch and answer your AI.")}>
      <Soon testId="windows-soon" label={t("not yet")} body={t("What works on Windows today:")}>
        <ul className="flex flex-col gap-2">
          {[
            t("Control your other devices from this browser: sign in at {{app}}.", { app: appHost() }),
            availability.app ? t("Run the AI on a Mac, with the Droppen App.") : t("Run the AI on a Mac: in the background today, with the Droppen App soon."),
            availability.linux ? t("Run the AI on a Linux server, or in WSL.") : t("Once Linux is supported: a Linux server, or WSL."),
          ].map((line) => (
            <li key={line} className="flex gap-2.5 text-xs leading-5 text-fg">
              <span className="mt-[7px] size-1.5 shrink-0 bg-fg" />
              <span>{line}</span>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap gap-2">
          <a {...publicLink(into)} data-variant="primary" data-testid="use-in-browser" className={primary}>
            {t("use {{app}} in this browser", { app: appHost() })}
          </a>
          <button type="button" data-testid="to-macos-app" onClick={() => onPick("macos-app")} className={secondary}>
            {t("the Mac options")}
          </button>
          {availability.linux && (
            <button type="button" data-testid="to-linux" onClick={() => onPick("linux")} className={secondary}>
              {t("the Linux server options")}
            </button>
          )}
        </div>
      </Soon>
    </PanelHead>
  );
}

function MobilePanel({ signedIn }: { signedIn: boolean }) {
  const { t } = useTranslation();
  const into = signedIn ? APP_PATH : SIGN_IN_PATH;
  const here = `${typeof location === "undefined" ? DROPPEN_ORIGIN : location.origin}${DOWNLOAD_PATH}`;
  return (
    <>
      <PanelHead
        title={t("On your phone, Droppen is the remote control")}
        body={t("The AI runs on your Mac or server; your phone watches it work, answers its questions and opens a terminal. Put {{app}} on your home screen and it opens like an app.", { app: appHost() })}
      >
        <a {...publicLink(into)} data-variant="primary" data-testid="open-droppen" className={cn(primary, "w-fit")}>
          {t("open {{app}}", { app: appHost() })}
        </a>
      </PanelHead>
      <Steps
        testId="mobile-steps"
        items={[
          { title: t("iPhone and iPad (Safari)"), body: t("Open {{app}} in Safari, tap Share, then Add to Home Screen.", { app: appHost() }) },
          { title: t("Android (Chrome)"), body: t("Open {{app}} in Chrome, tap ⋮, then Add to Home screen or Install app.", { app: appHost() }) },
          {
            title: t("Install it where your AI runs"),
            body: (
              <span className="flex flex-col gap-2">
                <span>{t("Droppen itself goes on the device your AI runs on. Open this page there:")}</span>
                <CommandLine command={here} label={t("copy")} testId="page-address" />
              </span>
            ),
          },
        ]}
      />
    </>
  );
}

/** Each tab's icon and name. */
function useTabs(): { os: DownloadOs; label: string; icon: typeof Laptop }[] {
  const { t } = useTranslation();
  return [
    { os: "macos-app", label: t("Mac · App"), icon: AppWindow },
    { os: "macos-bg", label: t("Mac · background"), icon: Laptop },
    { os: "linux", label: t("Linux server"), icon: Server },
    { os: "windows", label: t("Windows"), icon: Monitor },
    { os: "mobile", label: t("Phone / tablet"), icon: Smartphone },
  ];
}

/** The ways onto a device, as a tablist: the detected one marked, the chosen one inked in. */
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
  /** The detected device's tab, marked; `null` when the device cannot be told. */
  detected: DownloadOs | null;
  onPick: (os: DownloadOs) => void;
  availability: Availability;
  signedIn: boolean;
  arch: string | null;
  release: LatestRelease | null | undefined;
}) {
  const { t } = useTranslation();
  const tabs = useTabs();
  return (
    <BaseTabs.Root value={os} onValueChange={(next) => onPick(next as DownloadOs)} data-testid="environments" data-os={os} data-detected={detected ?? undefined}>
      <BaseTabs.List aria-label={t("Choose your system")} activateOnFocus className="grid grid-cols-2 gap-px border-b border-border bg-border sm:grid-cols-5">
        {tabs.map(({ os: value, label, icon: Icon }, i) => (
          <BaseTabs.Tab
            key={value}
            value={value}
            data-testid={`os-${value}`}
            className={cn(
              "group flex min-h-14 min-w-0 items-center gap-2.5 bg-bg px-4 py-2.5 text-left text-fg outline-none hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent",
              "data-[active]:bg-fg data-[active]:text-bg data-[active]:hover:bg-fg",
              i === tabs.length - 1 && "max-sm:col-span-2",
            )}
          >
            <Icon aria-hidden="true" className="size-4 shrink-0" />
            <span className="flex min-w-0 flex-col">
              <span className="truncate text-[13px] leading-5 font-medium">{label}</span>
              {value === detected && (
                <span data-testid="detected" className="font-mono text-[10px] leading-3.5 tracking-wider text-muted uppercase group-data-[active]:text-bg/70">
                  {t("Detected")}
                </span>
              )}
            </span>
          </BaseTabs.Tab>
        ))}
      </BaseTabs.List>
      {DOWNLOAD_OSES.map((value) => (
        <BaseTabs.Panel key={value} value={value} data-testid={`panel-${value}`} className="outline-none">
          {value === "macos-app" && <AppPanel availability={availability} onPick={onPick} release={release} />}
          {value === "macos-bg" && <BackgroundPanel />}
          {value === "linux" && <LinuxPanel availability={availability} onPick={onPick} arch={arch} release={release} />}
          {value === "windows" && <WindowsPanel availability={availability} signedIn={signedIn} onPick={onPick} />}
          {value === "mobile" && <MobilePanel signedIn={signedIn} />}
        </BaseTabs.Panel>
      ))}
    </BaseTabs.Root>
  );
}

function Requirements({ availability }: { availability: Availability }) {
  const { t } = useTranslation();
  const rows = [
    { name: t("Mac"), value: availability.app ? t("The Droppen App: macOS 13 or later, Apple silicon or Intel, one universal download") : t("Apple silicon or Intel: in the background today, the Droppen App soon"), soon: false },
    { name: t("Linux server"), value: availability.linux ? t("The install command; no browser needed on the server") : t("coming"), soon: !availability.linux },
    { name: t("Windows"), value: t("not yet, on its way"), soon: true },
    { name: t("Phone / tablet"), value: t("to control your devices, in the browser: iPhone, iPad, Android"), soon: false },
    { name: t("Claude Code or Codex"), value: t("installed and signed in on that device: Droppen runs them, it does not replace them"), soon: false },
  ];
  return (
    <Section id="requirements">
      <div className="grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.55fr)]">
        <SectionHead label={t("System requirements")} title={t("What it runs on")} className="lg:border-r lg:border-border" />
        <dl data-testid="download-requirements" className="flex flex-col max-lg:border-t max-lg:border-border">
          {rows.map((r) => (
            <div key={r.name} className="grid grid-cols-[10rem_minmax(0,1fr)] items-baseline gap-4 border-b border-border px-6 py-3.5 last:border-b-0 max-md:grid-cols-1 max-md:gap-1 max-md:px-4">
              <dt className="text-[13px] leading-5 font-semibold text-fg">{r.name}</dt>
              <dd className={cn("text-xs leading-5", r.soon ? "font-mono text-faint" : "text-muted")}>{r.value}</dd>
            </div>
          ))}
        </dl>
      </div>
    </Section>
  );
}

function Manual() {
  const { t } = useTranslation();
  return (
    <Section id="manual">
      <SectionHead label={t("Manual download")} title={t("Want to see what you install? Download it by hand.")} lead={t("Every release is on GitHub, with the SHA-256 checksum of each file next to it.")} />
      <div className="flex flex-col gap-4 px-6 pb-10 max-md:px-4 max-md:pb-8">
        <a href={RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="releases-link" className={cn(secondary, "self-start")}>
          {t("latest release on GitHub")}
          <ExternalLink className="size-3.5" />
        </a>
        <div className="flex max-w-[44rem] flex-col gap-2">
          <p className="text-xs leading-5 text-muted">{t("Check the file before you open it:")}</p>
          <CommandLine command={CHECKSUM_COMMAND} label={t("copy")} />
          <p className="text-xs leading-5 text-muted">{t("The number it prints must be the one the release lists. The install script makes the same check before it installs anything.")}</p>
        </div>
      </div>
    </Section>
  );
}

function Uninstall({ availability }: { availability: Availability }) {
  const { t } = useTranslation();
  const steps = [
    { title: t("Remove it from your account"), body: t("On {{app}}, open my devices and remove the device. Its keys are rotated, so it cannot open anything written afterwards.", { app: appHost() }) },
    { title: t("Disconnect it"), body: t("On the device, Settings → Access → Droppen → Disconnect this device deletes its Droppen keys there.") },
  ];
  return (
    <Section id="uninstall">
      <SectionHead label={t("Uninstall")} title={t("Leaving takes three steps, too.")} />
      <ol className="grid border-t border-border md:grid-cols-3">
        {steps.map((s, i) => (
          <li key={s.title} className="flex flex-col gap-2 border-border px-6 py-6 max-md:border-b max-md:px-4 md:border-r">
            <span className="font-mono text-2xs text-faint tabular-nums">0{i + 1}</span>
            <span className="text-[13px] leading-5 font-semibold text-fg">{s.title}</span>
            <span className="text-xs leading-5 text-muted">{s.body}</span>
          </li>
        ))}
        <li className="flex min-w-0 flex-col gap-2 px-6 py-6 max-md:px-4">
          <span className="font-mono text-2xs text-faint tabular-nums">03</span>
          <span className="text-[13px] leading-5 font-semibold text-fg">{t("Remove the program")}</span>
          {availability.app && <span className="text-xs leading-5 text-muted">{t("The App: quit Droppen and drag it from Applications to the Trash.")}</span>}
          <span className="text-xs leading-5 text-muted">{availability.app ? t("In the background: run the install script again with --uninstall:") : t("Run the install script again with --uninstall:")}</span>
          <CommandLine command={UNINSTALL_COMMAND} label={t("copy")} testId="uninstall-command" />
        </li>
      </ol>
    </Section>
  );
}

function End({ signedIn, signup }: { signedIn: boolean; signup: SignupMode }) {
  const { t } = useTranslation();
  return (
    <Section>
      <div className="flex flex-col gap-6 px-6 py-14 max-md:px-4 max-md:py-10 md:flex-row md:items-end md:justify-between">
        <div className="flex flex-col gap-3">
          <CapsLabel>{t("Ready when you are")}</CapsLabel>
          <p className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[26px]">
            {t("Install it, then sign it in to Droppen.")}
          </p>
          <p className="text-sm leading-6 text-muted">
            {signedIn ? (
              t("Once it is signed in, it is in my devices: add another device the same way to see both on one screen.")
            ) : signup === "closed" ? (
              t("No account yet? Sign-up is closed for now: join the waitlist.")
            ) : signup === "invite" ? (
              t("No account yet? Sign up with your invite code first, or in the {{app}} window when you sign the device in.", { app: appHost() })
            ) : (
              t("No account yet? Sign up first, or in the {{app}} window when you sign the device in.", { app: appHost() })
            )}
          </p>
        </div>
        <div className="flex flex-wrap gap-3">
          {signedIn ? (
            <a {...publicLink("/devices")} data-testid="download-open-devices" className={secondary}>
              {t("Open my devices")}
            </a>
          ) : (
            <>
              {signup === "closed" ? (
                <a {...publicLink(WAITLIST_HREF)} data-testid="download-waitlist" className={secondary}>
                  {t("Join the waitlist")}
                </a>
              ) : (
                <a {...publicLink(SIGN_UP_PATH)} className={secondary}>
                  {t("Get started")}
                </a>
              )}
              <a {...publicLink(SIGN_IN_PATH)} className={secondary}>
                {t("Sign in")}
              </a>
            </>
          )}
        </div>
      </div>
    </Section>
  );
}

/** The page's title and description in the reader's language. */
function useDocumentMeta() {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("Download Droppen — your AI, on your phone");
    document.documentElement.lang = active.language;
    const meta = document.querySelector('meta[name="description"]');
    const description = meta?.getAttribute("content") ?? null;
    meta?.setAttribute("content", t("Get Droppen for the device Claude Code and Codex run on: the Mac app, or one line for a server. Sign it in, and it is on your phone."));
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
  // Linux's processor, said on its tab (from the high-entropy hints, else the user agent; never asked for a Mac: the App is universal).
  const [arch, setArch] = useState<string | null>(cpu ?? null);
  const latest = useLatestRelease(release);
  useEffect(() => {
    if (os !== "linux" || hints) return;
    let live = true;
    void browserCpu().then((value) => live && setArch(value));
    return () => {
      live = false;
    };
  }, [os, hints]);
  return (
    <div data-testid="download" className="h-full w-full overflow-y-auto bg-bg [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1152px] md:border-x md:border-border">
          <div className="flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
            <CapsLabel>{t("Download")}</CapsLabel>
            <h1 className="max-w-[18em] text-[52px] leading-[1.02] font-semibold tracking-[-0.035em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[34px] xl:text-[48px]">
              {t("Install it once, and your AI is on your phone.")}
            </h1>
            <p className="max-w-[40em] text-[15px] leading-[26px] text-muted max-md:text-sm max-md:leading-6">
              {t("Droppen runs next to Claude Code and Codex on your device. Open it, sign it in to Droppen, and that device is on your phone, and side by side with your other devices on one screen.")}
            </p>
            <p className="font-mono text-2xs text-faint">
              {detected ? t("We picked the way for the device you are on; every other way is one click away.") : t("Pick the device your AI runs on: every way is one click away.")}
            </p>
          </div>
          <Section id="get">
            <Environments os={os} detected={detected} onPick={pick} availability={availability} signedIn={signedIn} arch={arch} release={latest} />
          </Section>
          <Requirements availability={availability} />
          <Manual />
          <Uninstall availability={availability} />
          <End signedIn={signedIn} signup={signup} />
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
