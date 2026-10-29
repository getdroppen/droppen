import { type ReactNode, useEffect, useMemo, useState } from "react";
import { Tabs as BaseTabs } from "@base-ui-components/react/tabs";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Download, ExternalLink, Info, Laptop, Monitor, Server } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { cn } from "@/lib/cn";
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
import { APP_PATH, publicLink, SECURITY_PATH, SIGN_IN_PATH } from "./route";

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

/** The platforms with something to update, uninstall or check: Windows installs nothing. */
type Platform = "macos-app" | "linux";

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

/** Numbered steps, as hairline cells; `stack` for a step whose body holds a command (the Linux server's). */
function Steps({ items, testId, stack = false }: { items: { title: string; body: ReactNode }[]; testId?: string; stack?: boolean }) {
  return (
    <ol
      data-testid={testId}
      className={cn("grid border-t border-border", stack ? "grid-cols-1" : cn("sm:grid-cols-2", items.length >= 4 ? "lg:grid-cols-4" : "lg:grid-cols-3"))}
    >
      {items.map((s, i) => (
        <li
          key={s.title}
          className={cn(
            "flex min-w-0 flex-col gap-2 border-border px-6 py-5 max-md:px-4",
            stack
              ? "border-b last:border-b-0"
              : cn(
                  "max-sm:border-b max-sm:last:border-b-0",
                  "sm:max-lg:[&:nth-child(2n)]:border-l sm:max-lg:[&:not(:nth-last-child(-n+2))]:border-b",
                  i > 0 && "lg:border-l",
                ),
          )}
        >
          {/* The number is the page's face (IBM Plex Sans), with tabular figures so 01/02/03 align; the command's mono is not used here. */}
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="shrink-0 text-xs font-semibold text-faint tabular-nums">{String(i + 1).padStart(2, "0")}</span>
            <span className="min-w-0 text-base leading-6 font-semibold text-fg">{s.title}</span>
          </span>
          <div className={cn("text-base text-muted", stack && "max-w-[44rem]")}>{s.body}</div>
        </li>
      ))}
    </ol>
  );
}

/** A panel's lead: what this way gets you, what the device has to be, and its one action. */
function PanelHead({ title, body, note, children }: { title: string; body: string; note?: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col gap-4 px-6 py-6 max-md:px-4">
      <div className="flex max-w-[44rem] flex-col gap-2">
        <h2 className="text-xl leading-7 font-semibold tracking-tight text-fg">{title}</h2>
        <p className="text-base text-muted">{body}</p>
        {/* What the device has to be, under the panel it belongs to — never a band of its own above the title. */}
        {note && <Nonce>{note}</Nonce>}
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
      <p className="text-base text-fg">{body}</p>
      {children}
    </div>
  );
}

/** How to check the file before opening it: the checksum, and what it must match. `flat`: inside a card that already draws the frame. */
function Check({ flat = false }: { flat?: boolean }) {
  const { t } = useTranslation();
  return (
    <details className={cn(!flat && "border border-border bg-raised px-4 py-3")}>
      <summary className="cursor-pointer text-sm font-semibold text-fg">{t("Check the file before you open it")}</summary>
      <div className="mt-2 flex flex-col gap-2 text-sm text-muted">
        <span>{t("Before you open the .dmg or extract the .tar.gz, check it in a terminal:")}</span>
        <CommandLine command={CHECKSUM_COMMAND} label={t("copy")} />
        <span>{t("The checksum it prints must match that file's line in the release's SHA256SUMS; if it does not, do not install it. The install script runs the same check before it installs anything.")}</span>
      </div>
    </details>
  );
}

function AppPanel({ availability, release }: { availability: Availability; release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  const dmg = release?.files.macos;
  const head = {
    title: t("Droppen App (recommended)"),
    body: availability.appSignIn
      ? t("Open the app and Droppen runs on this Mac, with no terminal and no local password. To use it from your phone and other devices, sign in to Droppen from the app.")
      : t("Open the app and Droppen runs on this Mac, with no terminal and no local password."),
    note: t("macOS 13 or later: Apple silicon or Intel."),
  };
  if (!availability.app) {
    return (
      <PanelHead {...head}>
        <Soon testId="app-soon" label={t("The App is coming soon")} body={t("Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.")} />
      </PanelHead>
    );
  }
  return (
    <>
      <PanelHead {...head}>
        <div className="flex max-w-[44rem] flex-col gap-4">
          <div className="flex flex-wrap items-center gap-3">
            {/* The file itself when the hub named it, downloaded in place; else the release page. */}
            <a {...fileLink(dmg)} data-variant="primary" data-testid="app-download" className={landingPrimary}>
              <Download className="size-3.5" />
              {t("Download Droppen App (macOS)")}
            </a>
            {release && dmg && <span data-testid="app-file-facts" className="text-xs font-semibold text-faint">{fileFacts(release, dmg)}</span>}
            <a href={release?.releasesPage ?? RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="app-release-link" className="inline-flex items-center gap-1 text-xs font-semibold text-muted underline underline-offset-4 hover:text-fg">
              {t("release notes, checksums and other files")}
              <ExternalLink className="size-3" />
            </a>
          </div>
          {/* The check that belongs beside the file, not in a table further down the page. */}
          <Check />
        </div>
      </PanelHead>
      <Steps
        stack
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
                    "In the setup guide, choose “On my phone and other devices too”, then sign in: your browser opens {{app}}, where you check the fingerprint and approve this Mac.",
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

function BackgroundPanel({ primary = false }: { primary?: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      <PanelHead title={t("Command line")} body={t("For a Mac that stays on as a server, or if you prefer the terminal. One command installs Droppen; you then add this Mac to your account in the browser.")} note={t("macOS 13 or later: Apple silicon or Intel.")}>
        <div className="flex max-w-[44rem] flex-col gap-2">
          <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="install-command" primaryCopy={primary} />
          <p className="text-xs font-semibold text-faint">{t("Paste it into Terminal on that Mac.")}</p>
        </div>
      </PanelHead>
      <Steps
        stack
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
    title: t("Droppen · Linux"),
    body: t("Run your AI on a server that stays on, and follow it and answer it from your phone. The server needs no browser: it joins your account with a one-time code."),
    note: t("Linux on x86_64 or aarch64, with glibc 2.28 or later."),
  };
  if (!availability.linux) {
    return (
      <PanelHead {...head}>
        <Soon testId="linux-soon" label={t("coming")} body={t("Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.")}>
          <button type="button" data-variant="primary" data-testid="to-macos-app" onClick={() => onPick("macos-app")} className={cn(landingPrimary, "w-fit")}>
            {t("run it in the background on a Mac")}
          </button>
        </Soon>
      </PanelHead>
    );
  }
  return (
    <div className="grid lg:grid-cols-[minmax(0,1.05fr)_minmax(0,1fr)]">
      {/* The lead and the steps; the card beside them is the one object they act on. */}
      <div className="lg:border-r lg:border-border">
        <PanelHead title={head.title} body={head.body} />
        <Steps
          testId="linux-steps"
          stack
          items={[
            {
              title: t("Get a one-time code"),
              body: (
                <span className="flex flex-col gap-2">
                  {t("On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.", { app: appHost() })}{" "}
                  <a {...publicLink("/devices")} data-testid="linux-devices" className="w-fit text-fg underline underline-offset-4 hover:no-underline">
                    {t("Open my devices")}
                  </a>
                </span>
              ),
            },
            {
              title: t("Run the command on the server"),
              // install.sh: a systemd user service, never root; it prints the linger command when the service would stop at logout.
              body: t("The script installs Droppen, starts it as a systemd user service under your own user, never as root, and joins your account with the code. If it says so at the end, run sudo loginctl enable-linger to keep it running after you log out."),
            },
            { title: t("It is in my devices"), body: t("Within seconds it shows as online in my devices, on every phone and browser in your account.") },
          ]}
        />
      </div>
      {/* One card, from the title down: what the server has to be, then the command it runs and the file to check. It stays beside the steps as they scroll — below the 56px header, the 48px tab row and 16px between (7.5rem). */}
      <div className="px-6 py-6 max-md:px-4">
        <div data-testid="linux-card" className="flex flex-col gap-4 border border-border bg-raised p-4 lg:sticky lg:top-[7.5rem]">
          <div className="flex flex-col gap-2">
            <CapsLabel face="sans" className={cn(cjkTracking, "text-xs font-semibold")}>
              {t("System requirements")}
            </CapsLabel>
            <Nonce>{head.note}</Nonce>
            {arch && (
              <p data-testid="linux-cpu" className="text-xs leading-5 font-semibold text-faint">
                {t("This browser reports Linux on {{arch}}.", { arch })}
                {!(LINUX_CPUS as readonly string[]).includes(arch) && <> {t("Droppen's Linux builds are for x86_64 and aarch64.")}</>}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-2 border-t border-border pt-4">
            <p className="text-base text-muted">{t("Replace <code> with the one-time code.")}</p>
            <CommandLine command={codeInstallCommand(t("<code>"))} label={t("copy the command")} testId="install-command" primaryCopy />
            <LinuxTarballs release={release} cpu={arch} testId="linux-tarballs" sayUnbuilt={false} />
          </div>
          <div className="border-t border-border pt-4">
            <Check flat />
          </div>
        </div>
      </div>
    </div>
  );
}

function WindowsPanel({ availability, signedIn, onPick }: { availability: Availability; signedIn: boolean; onPick: (os: DownloadOs) => void }) {
  const { t } = useTranslation();
  const into = signedIn ? APP_PATH : SIGN_IN_PATH;
  return (
    <PanelHead title={t("Droppen · Windows")} body={t("On Windows, Droppen runs in the browser: follow your AI, answer its questions, steer it. Droppen itself goes on the Mac or Linux device that runs it.")}>
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
            <li key={key} className="flex gap-2.5 text-base text-fg">
              <span className="mt-[9px] size-1.5 shrink-0 bg-fg" />
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
function PhoneSection({ onPhone }: { onPhone: boolean }) {
  const { t } = useTranslation();
  return (
    <Section id="phone">
      <SectionHead
        title={t("Then, use it from your phone")}
        // Read on a phone, this page may be the first visit: say what to do first, where the reader lands.
        lead={onPhone ? t("If Droppen is not on a device yet, open {{url}} on the Mac or Linux device that runs your AI first.", { url: pageAddress() }) : undefined}
      />
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
      onValueChange={(next) => onPick(next as DownloadOs)}
      data-testid="environments"
      data-os={os}
      data-detected={detected ?? undefined}
    >
      <BaseTabs.List
        aria-label={t("Choose your system")}
        activateOnFocus
        /* Sticky through the whole platform block below it: the tab still says which platform's update, uninstall and details are being read. */
        className="sticky top-14 z-30 flex gap-8 border-b border-border bg-bg px-6 max-md:gap-6 max-md:px-4"
      >
        {tabs.map(({ os: value, label, icon: Icon }) => (
          <BaseTabs.Tab
            key={value}
            value={value}
            data-testid={`os-${value}`}
            className="group relative flex h-12 shrink-0 items-center gap-2 text-base text-muted outline-none hover:text-fg focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent data-[active]:font-semibold data-[active]:text-fg"
          >
            <Icon aria-hidden="true" className="size-4 shrink-0" />
            {label}
            {value === marked && (
              <span data-testid="detected" className="border border-border px-1.5 text-xs font-semibold leading-4 font-normal text-muted max-[360px]:hidden">
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
              {/* Both ways, side by side: no switch to find, and nothing hidden behind one. */}
              <div data-testid="mac-ways" className="grid border-border lg:grid-cols-2">
                <div data-testid="way-panel-macos-app" className="border-b border-border lg:border-r lg:border-b-0">
                  <AppPanel availability={availability} release={release} />
                </div>
                <div data-testid="way-panel-macos-bg">
                  <BackgroundPanel primary={!availability.app} />
                </div>
              </div>
              {/* The rest of the platform's page, inside the same tab: one block, one sticky row. */}
              <Update platform="macos-app" availability={availability} />
              <Uninstall platform="macos-app" availability={availability} />
              <Verify platform="macos-app" availability={availability} />
            </>
          )}
          {value === "linux" && (
            <>
              <LinuxPanel availability={availability} onPick={onPick} arch={arch} release={release} />
              <Update platform="linux" availability={availability} />
              <Uninstall platform="linux" availability={availability} />
              <Verify platform="linux" availability={availability} />
            </>
          )}
          {value === "windows" && <WindowsPanel availability={availability} signedIn={signedIn} onPick={onPick} />}
        </BaseTabs.Panel>
      ))}
    </BaseTabs.Root>
  );
}

/**
 * What a panel assumes, under its title: the system. It used to be a band of
 * its own above the title — the first thing read, before the panel said what
 * it was — and its top border doubled the tab row's.
 */
function Nonce({ children }: { children: ReactNode }) {
  return (
    <p className="flex items-start gap-2 text-sm leading-6 text-muted">
      <Info className="mt-1 size-3.5 shrink-0" />
      {children}
    </p>
  );
}

/** For whoever approves the install: what it puts on the computer, how it runs, and how to check the files. */
function Verify({ platform, availability }: { platform: Platform; availability: Availability }) {
  const { t } = useTranslation();
  const rows: { term: string; body: ReactNode; wide?: boolean }[] = [
    ...(platform === "macos-app" && availability.app
      ? [{ term: t("Mac app"), body: t("Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version."), wide: true }]
      : []),
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
  // The Mac app's row is the full width; the four that follow are a 2×2, so the rule between columns counts only those.
  let column = 0;
  const cells: { term: string; body: ReactNode; wide?: boolean; left?: boolean }[] = rows.map((row) => {
    if (row.wide) return row;
    const left = column % 2 === 0;
    column += 1;
    return { ...row, left };
  });
  return (
    <Section id="verify">
      <SectionHead title={t("What it installs, and how it connects")} />
      <dl data-testid="download-verify" className="grid border-t border-border sm:grid-cols-2">
        {cells.map((r) => (
          <div
            key={r.term}
            className={cn(
              "flex min-w-0 flex-col gap-2 border-b border-border px-6 py-5 last:border-b-0 max-md:px-4 sm:[&:nth-last-child(2)]:border-b-0",
              r.wide ? "sm:col-span-2" : r.left && "sm:border-r",
            )}
          >
            <dt className="text-base leading-6 font-semibold text-fg">{r.term}</dt>
            <dd className="min-w-0 max-w-[42em] text-base text-muted">{r.body}</dd>
          </div>
        ))}
      </dl>
    </Section>
  );
}

/** Updating: the App does it itself; a command-line install runs the install command again. */
function Update({ platform, availability }: { platform: Platform; availability: Availability }) {
  const { t } = useTranslation();
  const lead =
    platform === "macos-app" && availability.app
      ? t("The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.")
      : t("To update, run the install command again: it upgrades in place and keeps your data.");
  return (
    <Section id="update">
      {/* The lead on the left, the command it asks for on the right: the section uses the page's width instead of leaving half of it blank. */}
      <div className="grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.55fr)]">
        <div className="lg:border-r lg:border-border">
          <SectionHead title={t("Update")} lead={lead} />
        </div>
        <div className="grid grid-cols-1 items-center px-6 py-6 max-md:px-4 max-lg:border-t max-lg:border-border">
          <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="update-command" />
        </div>
      </div>
    </Section>
  );
}

function Uninstall({ platform, availability }: { platform: Platform; availability: Availability }) {
  const { t } = useTranslation();
  // Every key it held is replaced (the security page's 2.8); removal stops its link but leaves its files (`cloud/control.rs`): the disconnect deletes them.
  const steps = [
    { title: t("Remove it from your account"), body: t("On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.", { app: appHost() }) },
    { title: t("Disconnect it"), body: t("On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. It deletes the Droppen keys stored there, which removing it from the account leaves in place.") },
  ];
  const app = platform === "macos-app" && availability.app;
  return (
    <Section id="uninstall">
      <SectionHead title={t("Uninstall")} />
      <ol className="grid border-t border-border xl:grid-cols-3">
        {steps.map((s, i) => (
          <li key={s.title} className="flex flex-col gap-2 border-border px-6 py-6 max-xl:border-b max-md:px-4 xl:border-r">
            <span className="flex min-w-0 items-baseline gap-2">
              <span className="shrink-0 text-xs font-semibold text-faint tabular-nums">0{i + 1}</span>
              <span className="min-w-0 text-base leading-6 font-semibold text-fg">{s.title}</span>
            </span>
            <span className="max-w-[42em] text-base text-muted">{s.body}</span>
          </li>
        ))}
        <li className="flex min-w-0 flex-col gap-2 px-6 py-6 max-md:px-4">
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="shrink-0 text-xs font-semibold text-faint tabular-nums">03</span>
            <span className="min-w-0 text-base leading-6 font-semibold text-fg">{t("Remove the program")}</span>
          </span>
          {app && <span className="max-w-[42em] text-base text-muted">{t("The App: quit Droppen and drag it from Applications to the Trash.")}</span>}
          <span className="max-w-[42em] text-base text-muted">
            {app ? t("Command-line install (Mac or Linux): run this on that device, as the user who installed it:") : t("Run this on that device, as the user who installed it:")}
          </span>
          <CommandLine command={UNINSTALL_COMMAND} label={t("copy")} testId="uninstall-command" />
          {/* install.sh's `--uninstall`: the service and the program go, the data stays, and it prints where that is and how to delete it. */}
          <span className="max-w-[42em] text-base text-muted">{t("It stops and removes the service and the program and keeps your data; at the end it prints where the data is, and the command that deletes it.")}</span>
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
}: {
  signedIn?: boolean;
  availability?: Availability;
  hints?: EnvironmentHints;
  cpu?: string | null;
  release?: LatestRelease | null;
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
                <p className="text-base text-fg">{t("You are on a phone. To install, open this page on the device that runs your AI:")}</p>
                <CommandLine command={pageAddress()} label={t("copy")} testId="page-address" />
              </div>
            )}
          </div>
          <Section id="get">
            <Environments os={os} detected={detected} onPick={pick} availability={availability} signedIn={signedIn} arch={onLinux ? arch : null} release={latest} />
          </Section>
          <PhoneSection onPhone={detected === "mobile"} />
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
