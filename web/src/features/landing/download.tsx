import { type MouseEvent, type ReactNode, type RefObject, useEffect, useMemo, useRef, useState } from "react";
import { Menu } from "@base-ui-components/react/menu";
import { Tabs } from "@base-ui-components/react/tabs";
import i18n from "i18next";
import type { TFunction } from "i18next";
import { ArrowRight, Check, ChevronDown, ExternalLink, Globe } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";
import { appHost } from "@/lib/hub/hosts";
import type { LatestRelease } from "@/lib/hub/releases";
import { landingBundles } from "./copy";
import { browserCpu, browserHints, detectKind, type EnvironmentHints, initialOs, LINUX_CPUS, osForKind } from "./detect";
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
import { AppleMark, LinuxMark, WindowsMark } from "./os-marks";
import { CommandLine, FooterLine, landingPrimary, PublicHeader, Section } from "./page-kit";
import { AppWindow, ProductShot } from "./product-tour";
import { fileFacts, fileLink, useLatestRelease } from "./release-files";
import { publicLink, SECURITY_PATH } from "./route";

/*
 * droppen.app/download: the Mac app, then the command-line server, the browser
 * on any other device, and the maintenance details folded away. The platform
 * menu and server tabs open on the detected (or `?os=`) platform.
 */

const COLUMN = "mx-auto w-full max-w-[64rem]";
const LINK = "text-fg underline underline-offset-4 hover:no-underline";

for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

function useDocumentMeta() {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("Download Droppen");
    document.documentElement.lang = active.language;
    const meta = document.querySelector('meta[name="description"]');
    const description = meta?.getAttribute("content") ?? null;
    meta?.setAttribute(
      "content",
      t("Install Droppen where Claude Code or Codex runs: the Mac app, or one command on Linux, macOS, or WSL. Your phone needs only the browser."),
    );
    return () => {
      document.title = before;
      if (meta && description !== null) meta.setAttribute("content", description);
    };
  }, [t, active.language]);
}

function BlockHead({ title, lead }: { title: string; lead?: ReactNode }) {
  return (
    <div className="flex flex-col gap-3 px-6 pb-7 max-md:px-4">
      <h2 className="text-2xl leading-8 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-xl">{title}</h2>
      {lead && <p className="max-w-[42em] text-sm leading-6 text-muted">{lead}</p>}
    </div>
  );
}

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
            <h3 className="min-w-0 text-base leading-6 font-semibold text-fg">{step.title}</h3>
          </div>
          <div className="max-w-[44rem] text-base leading-6 text-muted">{step.body}</div>
          {step.action && <div className="max-w-[44rem]">{step.action}</div>}
        </li>
      ))}
    </ol>
  );
}

/** A hairline disclosure, opened by the address's fragment (`#update`, …) when it names one. */
function Disclosure({ id, testId, summary, children }: { id?: string; testId?: string; summary: ReactNode; children: ReactNode }) {
  const ref = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const details = ref.current;
    if (!details || !id) return;
    const sync = () => {
      if (location.hash === `#${id}`) details.open = true;
    };
    sync();
    addEventListener("hashchange", sync);
    return () => removeEventListener("hashchange", sync);
  }, [id]);
  return (
    <details ref={ref} id={id} data-testid={testId} className="group scroll-mt-14 border-t border-border first:border-t-0">
      <summary className="flex cursor-pointer list-none items-center justify-between gap-4 px-6 py-4 text-base leading-6 font-semibold text-fg transition-colors hover:bg-hover max-md:px-4 [&::-webkit-details-marker]:hidden">
        <span className="min-w-0">{summary}</span>
        <span aria-hidden="true" className="font-mono text-sm text-muted group-open:hidden">
          +
        </span>
        <span aria-hidden="true" className="hidden font-mono text-sm text-muted group-open:inline">
          −
        </span>
      </summary>
      {children}
    </details>
  );
}

const MENU_ITEM = "flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm outline-none";

/** The `▾` half of the download control: the other platforms. */
function PlatformMenu({ anchor }: { anchor: RefObject<HTMLDivElement | null> }) {
  const { t } = useTranslation();
  const jumpToBrowser = (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    document.getElementById("browser")?.scrollIntoView();
    history.replaceState(null, "", "#browser");
  };
  return (
    <Menu.Root>
      <Menu.Trigger
        aria-label={t("Other platforms")}
        data-testid="platform-menu-trigger"
        className="group flex min-h-9 shrink-0 items-center justify-center border-l border-bg/25 bg-fg px-2.5 text-bg transition-colors hover:bg-fg/85 data-[popup-open]:bg-fg/85"
      >
        <ChevronDown aria-hidden="true" className="size-4 transition-transform duration-150 group-data-[popup-open]:rotate-180" />
      </Menu.Trigger>
      <Menu.Portal keepMounted>
        <Menu.Positioner anchor={anchor} side="bottom" align="start" sideOffset={4} className="z-50 outline-none">
          <Menu.Popup data-testid="platform-menu" className="popup-motion w-60 border border-border-strong bg-bg py-1 outline-none">
            <Menu.Item data-testid="hero-os-macos" className={cn(MENU_ITEM, "text-fg data-[highlighted]:bg-hover")}>
              <AppleMark className="size-4 shrink-0" />
              <span className="flex-1 font-medium">{t("macOS")}</span>
              <Check aria-hidden="true" className="size-3.5 shrink-0" />
            </Menu.Item>
            <Menu.Item disabled data-testid="hero-os-linux" className={cn(MENU_ITEM, "cursor-not-allowed text-faint")}>
              <LinuxMark className="size-4 shrink-0" />
              <span className="flex-1">{t("Linux")}</span>
              <span className="shrink-0 text-xs">{t("Coming soon")}</span>
            </Menu.Item>
            <Menu.Item disabled data-testid="hero-os-windows" className={cn(MENU_ITEM, "cursor-not-allowed text-faint")}>
              <WindowsMark className="size-4 shrink-0" />
              <span className="flex-1">{t("Windows")}</span>
              <span className="shrink-0 text-xs">{t("Coming soon")}</span>
            </Menu.Item>
            <Menu.Item
              render={<a href="#browser" onClick={jumpToBrowser} />}
              data-testid="hero-browser"
              className={cn(MENU_ITEM, "border-t border-border text-fg data-[highlighted]:bg-hover")}
            >
              <Globe aria-hidden="true" className="size-4 shrink-0" strokeWidth={1.5} />
              <span className="flex-1 font-medium">{t("Browser")}</span>
              <ArrowRight aria-hidden="true" className="size-3.5 shrink-0" />
            </Menu.Item>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

function MacDownload({ availability, release }: { availability: Availability; release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  const control = useRef<HTMLDivElement>(null);
  if (!availability.app) {
    return (
      <div className="flex flex-col gap-2">
        <p data-testid="app-soon" className="text-sm leading-5 font-semibold text-fg">
          {t("The App is coming soon")}
        </p>
        <p className="text-sm leading-5 text-muted">
          {t("Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.")}
        </p>
        <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="install-command" primaryCopy />
        <p className="text-xs leading-5 text-muted">{t("Paste it into Terminal on that Mac.")}</p>
      </div>
    );
  }
  const dmg = release?.files.macos;
  const facts = release && dmg ? fileFacts(release, dmg) : null;
  return (
    <div className="flex flex-col gap-2">
      <div ref={control} className="relative inline-flex w-fit border border-fg bg-fg max-sm:w-full">
        <a
          {...fileLink(dmg)}
          data-variant="primary"
          data-testid="app-download"
          className={cn(landingPrimary, "min-h-9 border-0 bg-fg max-sm:flex-1")}
        >
          <AppleMark className="size-4 shrink-0" />
          {t("Download for macOS")}
        </a>
        <PlatformMenu anchor={control} />
      </div>
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
          <MacDownload availability={availability} release={release} />
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

type ServerTab = "linux" | "macos" | "windows";

const SERVER_TABS: { id: ServerTab; label: string; Icon: typeof AppleMark }[] = [
  { id: "macos", label: "macOS", Icon: AppleMark },
  { id: "linux", label: "Linux", Icon: LinuxMark },
  { id: "windows", label: "Windows / WSL", Icon: WindowsMark },
];

function serverTabForOs(os: DownloadOs): ServerTab {
  if (os === "macos-app" || os === "macos-bg") return "macos";
  if (os === "windows") return "windows";
  return "linux";
}

function requirement(t: TFunction, tab: ServerTab): string {
  switch (tab) {
    case "linux":
      return t("Linux · x86_64 or aarch64 · glibc 2.28+ · systemd");
    case "macos":
      return t("macOS 13 or later · Apple silicon and Intel");
    case "windows":
      return t("Windows · WSL 2");
  }
}

function serverSteps(t: TFunction, tab: ServerTab): Step[] {
  // The installer takes the same one-time code on every platform; only Linux
  // and WSL need lingering.
  const linger = tab !== "macos";
  return [
    {
      title: t("Get a one-time code"),
      body: t("In my devices, add a device and show its one-time code. It works once, for 10 minutes."),
    },
    {
      title: t("Install it"),
      body: t("Replace <code> with the one-time code."),
      action: <CommandLine command={codeInstallCommand(t("<code>"))} label={t("copy the command")} testId="install-command" primaryCopy />,
    },
    {
      title: t("Check it is online"),
      body: linger
        ? t("It shows as online in my devices. If the script asks, run this so it stays up after you log out.")
        : t("It shows as online in my devices."),
      action: linger ? <CommandLine command={LINGER_COMMAND} label={t("copy the command")} testId="linger-command" /> : undefined,
    },
  ];
}

function ServerSection({ availability, os, arch }: { availability: Availability; os: DownloadOs; arch: string | null }) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<ServerTab>(() => serverTabForOs(os));
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
          {requirement(t, tab)}
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
      <Steps testId="server-steps" items={serverSteps(t, tab)} className="border-t-0" />
    </div>
  );
  return (
    <Section id="server">
      <div className={cn(COLUMN, "py-12 max-md:py-10")}>
        <BlockHead title={t("Run the server")} />
        <Tabs.Root value={tab} onValueChange={(value) => setTab(value as ServerTab)}>
          <Tabs.List aria-label={t("Server")} activateOnFocus className="mx-6 flex w-fit max-w-full items-stretch border border-border-strong bg-bg max-md:mx-4">
            {SERVER_TABS.map(({ id, label, Icon }, index) => (
              <Tabs.Tab
                key={id}
                value={id}
                data-testid={`server-tab-${id}`}
                className={cn(
                  "flex min-h-9 shrink-0 items-center gap-2 px-4 text-sm font-medium text-muted transition-colors hover:text-fg data-[active]:bg-fg data-[active]:text-bg max-md:min-h-11 max-md:px-3",
                  index > 0 && "border-l border-border",
                )}
              >
                <Icon aria-hidden="true" className="size-4 max-md:hidden" />
                {t(label)}
              </Tabs.Tab>
            ))}
          </Tabs.List>
          {SERVER_TABS.map(({ id }) => (
            <Tabs.Panel key={id} value={id} tabIndex={-1} className="outline-none">
              {panel}
            </Tabs.Panel>
          ))}
        </Tabs.Root>
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
        <BlockHead title={t("Use it on any other device")} lead={t("Nothing to install: any browser works.")} />
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
      <dt className="text-base leading-6 font-semibold text-fg">{term}</dt>
      <dd className="max-w-[44rem] text-base leading-6 text-muted">{children}</dd>
    </div>
  );
}

function Manage({ availability }: { availability: Availability }) {
  const { t } = useTranslation();
  return (
    <Section id="manage">
      <div className={cn(COLUMN, "py-12 max-md:py-10")}>
        <BlockHead title={t("Manage your installation")} lead={t("Update, uninstall, or check what you installed.")} />
        <div className="mx-6 border-t border-border max-md:mx-4">
          <Disclosure id="update" testId="manage-update" summary={t("Update")}>
            <div className="flex flex-col gap-4 px-6 py-5 max-md:px-4">
              <p className="max-w-[44rem] text-base leading-6 text-muted">
                {availability.app
                  ? t("The Mac app updates itself. For a command-line install, run the install command again: it upgrades in place and keeps your data.")
                  : t("Run the install command again: it upgrades in place and keeps your data.")}
              </p>
              <div className="max-w-[44rem]">
                <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="update-command" />
              </div>
            </div>
          </Disclosure>

          <Disclosure id="uninstall" testId="manage-uninstall" summary={t("Uninstall")}>
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
                      <CommandLine command={UNINSTALL_COMMAND} label={t("copy the command")} testId="uninstall-command" />
                      <span>{t("It stops and removes the service and the program, keeps your data, and shows where the data is and how to delete it.")}</span>
                    </span>
                  ),
                },
              ]}
            />
          </Disclosure>

          <Disclosure id="verify" testId="manage-verify" summary={t("Checks & connection details")}>
            <div className="flex flex-col gap-5 px-6 py-5 max-md:px-4">
              <div className="flex max-w-[44rem] flex-col gap-2">
                <h4 className="text-base leading-6 font-semibold text-fg">{t("Check the file before you open it")}</h4>
                <p className="text-base leading-6 text-muted">{t("Before you open the .dmg or extract the .tar.gz, check it in a terminal:")}</p>
                <CommandLine command={CHECKSUM_COMMAND} label={t("copy the command")} testId="checksum-command" />
                <p className="text-base leading-6 text-muted">
                  {t("The checksum must match that file's line in the release's SHA256SUMS. If it does not, do not install the file. The install script runs the same check first.")}
                </p>
              </div>
              <div className="flex max-w-[44rem] flex-col gap-2 border-t border-border pt-5">
                <h4 className="text-base leading-6 font-semibold text-fg">{t("Build provenance")}</h4>
                <p className="text-base leading-6 text-muted">
                  {t("From v0.2.127, every file has a build provenance attestation. With the GitHub CLI, check that this repository's release workflow built it:")}
                </p>
                <CommandLine command={ATTESTATION_COMMAND} label={t("copy the command")} testId="attestation-command" />
                <a
                  href={VERIFY_DOWNLOAD_URL}
                  target="_blank"
                  rel="noopener noreferrer"
                  data-testid="verify-download-link"
                  className="inline-flex w-fit items-center gap-1 text-xs font-semibold text-muted underline underline-offset-4 hover:text-fg"
                >
                  {t("How to verify a download")}
                  <ExternalLink className="size-3" />
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
                    <a {...publicLink(SECURITY_PATH)} data-testid="download-security" className="w-fit text-fg underline underline-offset-4 hover:no-underline">
                      {t("How the encryption works")}
                    </a>
                  </span>
                </Row>
              </dl>
            </div>
          </Disclosure>
        </div>
      </div>
    </Section>
  );
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
  useDocumentMeta();
  const detected = useMemo(() => osForKind(detectKind(hints ?? browserHints() ?? { userAgent: "" })), [hints]);
  const os = useMemo<DownloadOs>(
    () => initialOs({ search: typeof location === "undefined" ? "" : location.search, remembered: null, detected }),
    [detected],
  );
  const onLinux = detected === "linux";
  const [arch, setArch] = useState<string | null>(cpu ?? null);
  const latest = useLatestRelease(release);
  useEffect(() => {
    if (os !== "linux" || !onLinux || hints) return;
    let live = true;
    void browserCpu().then((value) => live && setArch(value));
    return () => {
      live = false;
    };
  }, [os, onLinux, hints]);

  // The old platform deep links now name a section below the hero.
  const explicit = typeof location === "undefined" ? null : osOfSearch(location.search);
  useEffect(() => {
    if (typeof location === "undefined") return;
    const id = location.hash.slice(1) || (explicit === "windows" ? "browser" : explicit === "linux" || explicit === "macos-bg" ? "server" : "");
    if (!id || typeof requestAnimationFrame === "undefined") return;
    requestAnimationFrame(() => document.getElementById(id)?.scrollIntoView());
  }, [explicit]);

  return (
    <div data-testid="download" className="h-full w-full overflow-y-auto bg-bg text-base [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main
          data-testid="environments"
          data-os={os}
          data-detected={detected ?? undefined}
          className="mx-auto max-w-[1360px] md:border-x md:border-border"
        >
          <Hero availability={availability} release={latest} />
          <ServerSection availability={availability} os={os} arch={onLinux ? arch : null} />
          <BrowserSection />
          <Manage availability={availability} />
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
