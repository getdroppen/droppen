import { type MouseEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import type { TFunction } from "i18next";
import { ArrowRightIcon, CheckIcon, CaretDownIcon, ArrowSquareOutIcon, GlobeIcon, AppleLogoIcon, LinuxLogoIcon, WindowsLogoIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { useEventListener } from "usehooks-ts";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tabs, TabsList, TabsPanel, TabsTab } from "@/components/ui/tabs";
import { cn } from "@/lib/cn";
import { appHost } from "@/lib/hub/hosts";
import type { LatestRelease } from "@/lib/hub/releases";
import "./copy";
import { browserHints, DEFAULT_OS, detectKind, type EnvironmentHints, LINUX_CPUS, osForKind, useBrowserCpu } from "./detect";
import {
  type Availability,
  ATTESTATION_COMMAND,
  AVAILABILITY,
  CHECKSUM_COMMAND,
  codeInstallCommand,
  type DownloadOs,
  INSTALL_COMMAND,
  LINGER_COMMAND,
  osOfSearch,
  UNINSTALL_COMMAND,
  VERIFY_DOWNLOAD_URL,
  WSL_SYSTEMD_URL,
} from "./get-droppen";
import { CommandLine, FaqAccordion, LINK, PublicLayout, QUIET_LINK, Section, SectionHead, SUBHEAD, usePageMeta } from "./page-kit";
import { AppWindow, ProductShot } from "./product-tour";
import { DownloadButton, fileFacts, useLatestRelease } from "./release-files";
import { DOWNLOAD_PATH, publicLink, SECURITY_PATH } from "./route";

const COLUMN = "mx-auto w-full max-w-[64rem]";

interface Step {
  title: string;
  body: ReactNode;
  action?: ReactNode;
}

function Steps({ items, testId, className }: { items: Step[]; testId?: string; className?: string }) {
  return (
    <ol data-testid={testId} className={cn("flex flex-col border-t border-border", className)}>
      {items.map((step, index) => (
        <li
          key={step.title}
          data-testid={testId ? `${testId}-step` : undefined}
          className="flex min-w-0 flex-col gap-3 border-b border-border px-6 py-5 last:border-b-0 max-md:px-4"
        >
          <div className="flex min-w-0 items-baseline gap-3">
            <span className="shrink-0 text-base font-semibold text-faint tabular-nums">{String(index + 1).padStart(2, "0")}</span>
            <h3 className={cn("min-w-0", SUBHEAD)}>{step.title}</h3>
          </div>
          <div className="max-w-[44rem] text-base leading-6 text-muted">{step.body}</div>
          {step.action && <div className="max-w-[44rem]">{step.action}</div>}
        </li>
      ))}
    </ol>
  );
}

const MENU_ITEM = "gap-2.5 py-2 text-left text-sm";

const OS_MARKS = { macos: AppleLogoIcon, linux: LinuxLogoIcon, windows: WindowsLogoIcon } as const;

const MENU_PLATFORMS = [
  { id: "macos", testId: "hero-os-macos", label: "macOS", current: true },
  { id: "linux", testId: "hero-os-linux", label: "Linux", current: false },
  { id: "windows", testId: "hero-os-windows", label: "Windows", current: false },
] as const;

function PlatformMenu() {
  const { t } = useTranslation();
  const jumpToBrowser = (event: MouseEvent<HTMLAnchorElement>) => {
    const section = document.getElementById("browser");
    if (!section) return;
    event.preventDefault();
    section.scrollIntoView();
    history.replaceState(null, "", "#browser");
  };
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={t("Other platforms")}
        data-testid="platform-menu-trigger"
        className="group flex min-h-9 shrink-0 items-center justify-center border-l border-bg/25 bg-fg px-2.5 text-bg transition-colors hover:bg-fg/85 data-[popup-open]:bg-fg/85"
      >
        <CaretDownIcon aria-hidden="true" className="size-4 transition-transform duration-150 group-data-[popup-open]:rotate-180" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="bottom" align="start" sideOffset={4} data-testid="platform-menu" className="w-60 py-1">
        {MENU_PLATFORMS.map(({ id, testId, label, current }) => {
          const Icon = OS_MARKS[id];
          return (
            <DropdownMenuItem key={testId} disabled={!current} data-testid={testId} className={MENU_ITEM}>
              <Icon weight="fill" aria-hidden="true" className="size-4 shrink-0" />
              <span className={cn("flex-1", current && "font-medium")}>{t(label)}</span>
              {current ? (
                <CheckIcon aria-hidden="true" className="size-3.5 shrink-0" />
              ) : (
                <span className="shrink-0 text-xs">{t("Coming soon")}</span>
              )}
            </DropdownMenuItem>
          );
        })}
        <DropdownMenuItem
          render={<a href={`${DOWNLOAD_PATH}#browser`} onClick={jumpToBrowser} />}
          data-testid="hero-browser"
          className={cn(MENU_ITEM, "border-t border-border")}
        >
          <GlobeIcon aria-hidden="true" className="size-4 shrink-0" />
          <span className="flex-1 font-medium">{t("Browser")}</span>
          <ArrowRightIcon aria-hidden="true" className="size-3.5 shrink-0" />
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function AppDownload({ availability, release }: { availability: Availability; release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  if (!availability.app) {
    return (
      <div className="flex flex-col gap-2">
        <p data-testid="app-soon" className="text-sm leading-5 font-semibold text-fg">
          {t("The App is coming soon")}
        </p>
        <p className="text-sm leading-5 text-muted">
          {t("Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.")}
        </p>
        <CommandLine command={INSTALL_COMMAND} testId="install-command" primaryCopy />
        <p className="text-xs leading-5 text-muted">{t("Paste it into Terminal on that Mac.")}</p>
      </div>
    );
  }
  const dmg = release?.files.macos;
  const facts = release && dmg ? fileFacts(release, dmg) : null;
  return (
    <div className="flex flex-col gap-2">
      <DownloadButton
        file={dmg}
        mark={<AppleLogoIcon weight="fill" aria-hidden="true" className="size-4 shrink-0" />}
        testId="app-download"
        label={t("Download for macOS")}
        menu={<PlatformMenu />}
      />
      <p className="text-xs leading-5 text-muted">
        {facts && (
          <span data-testid="app-file-facts" className="font-semibold text-faint">
            {facts} ·{" "}
          </span>
        )}
        {t("macOS 13 or later · Apple silicon and Intel")}
      </p>
      {!availability.appSignIn && (
        <p data-testid="app-sign-in-soon" className="text-xs leading-5 text-muted">
          {t("Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.")}
        </p>
      )}
    </div>
  );
}

function Hero({ availability, release }: { availability: Availability; release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  return (
    <header className={cn(COLUMN, "px-6 pt-16 pb-12 max-md:px-4 max-md:pt-12 max-md:pb-10")}>
      <div className="grid items-center gap-10 lg:grid-cols-[minmax(0,0.95fr)_minmax(0,1.05fr)]">
        <div className="flex min-w-0 flex-col gap-6">
          <h1 className="max-w-[18em] text-5xl leading-display font-normal tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-4xl">
            {t("Download Droppen")}
          </h1>
          <AppDownload availability={availability} release={release} />
        </div>
        <div className="w-full max-w-[38rem] justify-self-end max-lg:justify-self-center">
          <AppWindow>
            <ProductShot
              shot="app-session"
              alt={t("Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer")}
              eager
              width={2560}
              height={1600}
            />
          </AppWindow>
        </div>
      </div>
    </header>
  );
}

function serverTabs(t: TFunction) {
  return [
    { id: "macos", label: t("macOS"), requirement: t("macOS 13 or later · Apple silicon and Intel"), linger: false, Icon: OS_MARKS.macos },
    { id: "linux", label: t("Linux"), requirement: t("Linux · x86_64 or aarch64 · glibc 2.28+ · systemd"), linger: true, Icon: OS_MARKS.linux },
    { id: "windows", label: t("Windows / WSL"), requirement: t("Windows · WSL 2"), linger: true, Icon: OS_MARKS.windows },
  ] as const;
}

type ServerTab = ReturnType<typeof serverTabs>[number]["id"];

function serverTabForOs(os: DownloadOs): ServerTab {
  if (os === "macos-app" || os === "macos-bg") return "macos";
  if (os === "windows") return "windows";
  return "linux";
}

function serverSteps(t: TFunction, linger: boolean): Step[] {
  return [
    {
      title: t("Get a one-time code"),
      body: t("In my devices, add a device and show its one-time code. It works once, for 10 minutes."),
    },
    {
      title: t("Install it"),
      body: t("Replace <code> with the one-time code."),
      action: <CommandLine command={codeInstallCommand(t("<code>"))} testId="install-command" primaryCopy />,
    },
    {
      title: t("Check it is online"),
      body: linger
        ? t("It shows as online in my devices. If the script asks, run this so it stays up after you log out.")
        : t("It shows as online in my devices."),
      action: linger ? <CommandLine command={LINGER_COMMAND} testId="linger-command" /> : undefined,
    },
  ];
}

function ServerSection({ availability, os, arch }: { availability: Availability; os: DownloadOs; arch: string | null }) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<ServerTab>(() => serverTabForOs(os));
  const tabs = serverTabs(t);
  const tabSpec = tabs.find((entry) => entry.id === tab)!;
  const built = (LINUX_CPUS as readonly string[]).includes(arch ?? "");
  const unavailable = !availability.linux && (tab === "linux" || tab === "windows");
  const panel = unavailable ? (
    <div className="mx-6 max-md:mx-4">
      <p className="px-6 py-5 text-base leading-6 text-muted max-md:px-4">
        {tab === "linux"
          ? t("Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.")
          : t("Once Linux is supported: a Linux server, or WSL.")}
      </p>
    </div>
  ) : (
    <div className="mx-6 max-md:mx-4">
      <div className="flex flex-col gap-2 border-b border-border px-6 py-4 max-md:px-4">
        <p className="text-xs leading-5 text-muted">
          {tabSpec.requirement}
          {tab === "windows" && (
            <>
              {" · "}
              <a href={WSL_SYSTEMD_URL} target="_blank" rel="noopener noreferrer" data-testid="wsl-systemd" className={LINK}>
                {t("How to turn on systemd")}
              </a>
            </>
          )}
        </p>
        {tab === "linux" && arch && (
          <p data-testid="linux-cpu" className="text-xs leading-5 font-semibold text-faint">
            {t("This browser reports Linux on {{arch}}.", { arch })}
            {!built && <> {t("Droppen's Linux builds are for x86_64 and aarch64.")}</>}
          </p>
        )}
      </div>
      <Steps testId="server-steps" items={serverSteps(t, tabSpec.linger)} className="border-t-0" />
    </div>
  );
  return (
    <Section id="server">
      <div className={cn(COLUMN, "py-12 max-md:py-10")}>
        <SectionHead title={t("Run the server")} className="pb-7" />
        <Tabs value={tab} onValueChange={(value) => setTab(value as ServerTab)}>
          <TabsList aria-label={t("Server")} activateOnFocus className="mx-6 h-auto w-fit max-w-full max-md:mx-4">
            {tabs.map(({ id, label, Icon }) => (
              <TabsTab
                key={id}
                value={id}
                data-testid={`server-tab-${id}`}
                className="min-h-9 shrink-0 flex-none gap-2 px-4 text-sm transition-colors max-md:min-h-11 max-md:px-3"
              >
                <Icon weight="fill" aria-hidden="true" className="size-4 max-md:hidden" />
                {label}
              </TabsTab>
            ))}
          </TabsList>
          {tabs.map(({ id }) => (
            <TabsPanel key={id} value={id} tabIndex={-1} className="outline-none">
              {panel}
            </TabsPanel>
          ))}
        </Tabs>
      </div>
    </Section>
  );
}

function BrowserSection() {
  const { t } = useTranslation();
  const steps: Step[] = [
    { title: t("Sign in"), body: t("Sign in with the account your other devices use.") },
    {
      title: t("Approve this browser"),
      body: t("On a device already in your account, open my devices → add a browser or phone, then scan the QR code this browser shows, or enter its number code."),
    },
  ];
  return (
    <Section id="browser">
      <span id="phone" aria-hidden="true" className="block scroll-mt-14" />
      <div className={cn(COLUMN, "py-12 max-md:py-10")}>
        <SectionHead title={t("Use it on any other device")} lead={t("Nothing to install: any browser works.")} className="pb-7" />
        <div className="mx-6 max-md:mx-4">
          <Steps testId="phone-steps" items={steps} />
        </div>
      </div>
    </Section>
  );
}

function Row({ term, children }: { term: string; children: ReactNode }) {
  return (
    <div className="grid gap-1 border-b border-border px-6 py-5 last:border-b-0 sm:grid-cols-[10rem_minmax(0,1fr)] sm:gap-6 max-md:px-4">
      <dt className={SUBHEAD}>{term}</dt>
      <dd className="max-w-[44rem] text-base leading-6 text-muted">{children}</dd>
    </div>
  );
}

function Manage({ availability }: { availability: Availability }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<string | null>(null);
  const sync = useCallback(() => {
    const id = location.hash.slice(1);
    if (id === "update" || id === "uninstall" || id === "verify") setOpen(id);
  }, []);
  useEffect(() => {
    sync();
  }, [sync]);
  useEventListener("hashchange", sync);
  const items = [
    {
      value: "update",
      id: "update",
      testId: "manage-update",
      q: t("Update"),
      a: (
        <div className="flex flex-col gap-4 px-6 py-5 max-md:px-4">
          <p className="max-w-[44rem] text-base leading-6 text-muted">
            {availability.app
              ? t("The Mac app updates itself. For a command-line install, run the install command again: it upgrades in place and keeps your data.")
              : t("Run the install command again: it upgrades in place and keeps your data.")}
          </p>
          <div className="max-w-[44rem]">
            <CommandLine command={INSTALL_COMMAND} testId="update-command" />
          </div>
        </div>
      ),
    },
    {
      value: "uninstall",
      id: "uninstall",
      testId: "manage-uninstall",
      q: t("Uninstall"),
      a: (
        <Steps
          testId="uninstall-steps"
          className="border-t-0"
          items={[
            {
              title: t("Remove it from your account"),
              body: t("On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.", { app: appHost() }),
            },
            {
              title: t("Disconnect it"),
              body: t(
                "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. This deletes the Droppen keys stored on it. Removing the device from your account leaves them in place.",
              ),
            },
            {
              title: t("Remove the program"),
              body: (
                <span className="flex flex-col gap-3">
                  {availability.app && <span>{t("Mac app: quit Droppen and drag it from Applications to the Trash.")}</span>}
                  <span>
                    {availability.app
                      ? t("Command-line install (Mac or Linux): run this on that device, as the user who installed it:")
                      : t("Run this on that device, as the user who installed it:")}
                  </span>
                  <CommandLine command={UNINSTALL_COMMAND} testId="uninstall-command" />
                  <span>{t("It stops and removes the service and the program, keeps your data, and shows where the data is and how to delete it.")}</span>
                </span>
              ),
            },
          ]}
        />
      ),
    },
    {
      value: "verify",
      id: "verify",
      testId: "manage-verify",
      q: t("Checks & connection details"),
      a: (
        <div className="flex flex-col gap-5 px-6 py-5 max-md:px-4">
          <div className="flex max-w-[44rem] flex-col gap-2">
            <h4 className={SUBHEAD}>{t("Check the file before you open it")}</h4>
            <p className="text-base leading-6 text-muted">{t("Before you open the .dmg or extract the .tar.gz, check it in a terminal:")}</p>
            <CommandLine command={CHECKSUM_COMMAND} testId="checksum-command" />
            <p className="text-base leading-6 text-muted">
              {t("The checksum must match that file's line in the release's SHA256SUMS. If it does not, do not install the file. The install script runs the same check first.")}
            </p>
          </div>
          <div className="flex max-w-[44rem] flex-col gap-2 border-t border-border pt-5">
            <h4 className={SUBHEAD}>{t("Build provenance")}</h4>
            <p className="text-base leading-6 text-muted">
              {t("From v0.2.127, every file has a build provenance attestation. With the GitHub CLI, check that this repository's release workflow built it:")}
            </p>
            <CommandLine command={ATTESTATION_COMMAND} testId="attestation-command" />
            <a
              href={VERIFY_DOWNLOAD_URL}
              target="_blank"
              rel="noopener noreferrer"
              data-testid="verify-download-link"
              className={cn("inline-flex w-fit items-center gap-1 text-xs font-semibold", QUIET_LINK)}
            >
              {t("How to verify a download")}
              <ArrowSquareOutIcon className="size-3" />
            </a>
          </div>
          <dl data-testid="download-verify" className="border-t border-border">
            {availability.app && (
              <Row term={t("Mac app")}>{t("Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.")}</Row>
            )}
            <Row term={t("Install script")}>{t("Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.")}</Row>
            <Row term={t("Background service")}>
              {t("A command-line install runs as a launchd user agent on macOS, or a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo only sets up and manages that service.")}
            </Row>
            <Row term={t("Network")}>
              {t("Listens only on 127.0.0.1 and connects out to Droppen, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.")}
            </Row>
            <Row term={t("Encryption")}>
              <span className="flex flex-col gap-2">
                <span>{t("End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.")}</span>
                <a {...publicLink(SECURITY_PATH)} data-testid="download-security" className={cn("w-fit", LINK)}>
                  {t("How the encryption works")}
                </a>
              </span>
            </Row>
          </dl>
        </div>
      ),
    },
  ];
  return (
    <Section id="manage">
      <div className={cn(COLUMN, "py-12 max-md:py-10")}>
        <SectionHead title={t("Manage your installation")} lead={t("Update, uninstall, or check what you installed.")} className="pb-7" />
        <FaqAccordion
          items={items}
          value={open ? [open] : []}
          onValueChange={(next) => setOpen(next[0] ?? null)}
          className="mx-6 max-md:mx-4"
          itemClassName="scroll-mt-14"
          triggerClassName="px-6 py-4 max-md:px-4"
        />
      </div>
    </Section>
  );
}

function sectionOf(os: DownloadOs | null): string {
  if (os === "windows") return "browser";
  return os === "linux" || os === "macos-bg" ? "server" : "";
}

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
  usePageMeta(
    t("Download Droppen"),
    t("Install Droppen where Claude Code or Codex runs: the Mac app, or one command on Linux, macOS, or WSL. Your phone needs only the browser."),
  );
  const detected = useMemo(() => osForKind(detectKind(hints ?? browserHints() ?? { userAgent: "" })), [hints]);
  const search = typeof location === "undefined" ? "" : location.search;
  const explicit = osOfSearch(search);
  const os = explicit ?? detected ?? DEFAULT_OS;
  const onLinux = detected === "linux";
  const latest = useLatestRelease(release);
  const arch = useBrowserCpu({ ask: os === "linux" && onLinux && !hints, cpu, nav: hints ? null : undefined });

  return (
    <PublicLayout
      testId="download"
      signedIn={signedIn}
      mainTestId="environments"
      mainData={{ "data-os": os, "data-detected": detected ?? undefined }}
      hashFallback={() => sectionOf(explicit)}
    >
      <Hero availability={availability} release={latest} />
      <ServerSection availability={availability} os={os} arch={onLinux ? arch : null} />
      <BrowserSection />
      <Manage availability={availability} />
    </PublicLayout>
  );
}
