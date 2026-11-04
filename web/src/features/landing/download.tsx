import { type MouseEvent, type ReactNode, type RefObject, type SVGProps, useEffect, useMemo, useRef, useState } from "react";
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
import { CommandLine, FaqAccordion, landingPrimary, PublicLayout, Section, usePageMeta } from "./page-kit";
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

type MarkProps = SVGProps<SVGSVGElement>;

function Mark({ children, ...props }: MarkProps) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {...props}>
      {children}
    </svg>
  );
}

/** Apple, Linux and Windows marks in `currentColor`; Apple and Linux paths from Simple Icons (CC0). */
function AppleMark(props: MarkProps) {
  return (
    <Mark {...props}>
      <path d="M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701" />
    </Mark>
  );
}

function LinuxMark(props: MarkProps) {
  return (
    <Mark {...props}>
      <path d="M12.504 0c-.155 0-.315.008-.48.021-4.226.333-3.105 4.807-3.17 6.298-.076 1.092-.3 1.953-1.05 3.02-.885 1.051-2.127 2.75-2.716 4.521-.278.832-.41 1.684-.287 2.489a.424.424 0 00-.11.135c-.26.268-.45.6-.663.839-.199.199-.485.267-.797.4-.313.136-.658.269-.864.68-.09.189-.136.394-.132.602 0 .199.027.4.055.536.058.399.116.728.04.97-.249.68-.28 1.145-.106 1.484.174.334.535.47.94.601.81.2 1.91.135 2.774.6.926.466 1.866.67 2.616.47.526-.116.97-.464 1.208-.946.587-.003 1.23-.269 2.26-.334.699-.058 1.574.267 2.577.2.025.134.063.198.114.333l.003.003c.391.778 1.113 1.132 1.884 1.071.771-.06 1.592-.536 2.257-1.306.631-.765 1.683-1.084 2.378-1.503.348-.199.629-.469.649-.853.023-.4-.2-.811-.714-1.376v-.097l-.003-.003c-.17-.2-.25-.535-.338-.926-.085-.401-.182-.786-.492-1.046h-.003c-.059-.054-.123-.067-.188-.135a.357.357 0 00-.19-.064c.431-1.278.264-2.55-.173-3.694-.533-1.41-1.465-2.638-2.175-3.483-.796-1.005-1.576-1.957-1.56-3.368.026-2.152.236-6.133-3.544-6.139zm.529 3.405h.013c.213 0 .396.062.584.198.19.135.33.332.438.533.105.259.158.459.166.724 0-.02.006-.04.006-.06v.105a.086.086 0 01-.004-.021l-.004-.024a1.807 1.807 0 01-.15.706.953.953 0 01-.213.335.71.71 0 00-.088-.042c-.104-.045-.198-.064-.284-.133a1.312 1.312 0 00-.22-.066c.05-.06.146-.133.183-.198.053-.128.082-.264.088-.402v-.02a1.21 1.21 0 00-.061-.4c-.045-.134-.101-.2-.183-.333-.084-.066-.167-.132-.267-.132h-.016c-.093 0-.176.03-.262.132a.8.8 0 00-.205.334 1.18 1.18 0 00-.09.4v.019c.002.089.008.179.02.267-.193-.067-.438-.135-.607-.202a1.635 1.635 0 01-.018-.2v-.02a1.772 1.772 0 01.15-.768c.082-.22.232-.406.43-.533a.985.985 0 01.594-.2zm-2.962.059h.036c.142 0 .27.048.399.135.146.129.264.288.344.465.09.199.14.4.153.667v.004c.007.134.006.2-.002.266v.08c-.03.007-.056.018-.083.024-.152.055-.274.135-.393.2.012-.09.013-.18.003-.267v-.015c-.012-.133-.04-.2-.082-.333a.613.613 0 00-.166-.267.248.248 0 00-.183-.064h-.021c-.071.006-.13.04-.186.132a.552.552 0 00-.12.27.944.944 0 00-.023.33v.015c.012.135.037.2.08.334.046.134.098.2.166.268.01.009.02.018.034.024-.07.057-.117.07-.176.136a.304.304 0 01-.131.068 2.62 2.62 0 01-.275-.402 1.772 1.772 0 01-.155-.667 1.759 1.759 0 01.08-.668 1.43 1.43 0 01.283-.535c.128-.133.26-.2.418-.2zm1.37 1.706c.332 0 .733.065 1.216.399.293.2.523.269 1.052.468h.003c.255.136.405.266.478.399v-.131a.571.571 0 01.016.47c-.123.31-.516.643-1.063.842v.002c-.268.135-.501.333-.775.465-.276.135-.588.292-1.012.267a1.139 1.139 0 01-.448-.067 3.566 3.566 0 01-.322-.198c-.195-.135-.363-.332-.612-.465v-.005h-.005c-.4-.246-.616-.512-.686-.71-.07-.268-.005-.47.193-.6.224-.135.38-.271.483-.336.104-.074.143-.102.176-.131h.002v-.003c.169-.202.436-.47.839-.601.139-.036.294-.065.466-.065zm2.8 2.142c.358 1.417 1.196 3.475 1.735 4.473.286.534.855 1.659 1.102 3.024.156-.005.33.018.513.064.646-1.671-.546-3.467-1.089-3.966-.22-.2-.232-.335-.123-.335.59.534 1.365 1.572 1.646 2.757.13.535.16 1.104.021 1.67.067.028.135.06.205.067 1.032.534 1.413.938 1.23 1.537v-.043c-.06-.003-.12 0-.18 0h-.016c.151-.467-.182-.825-1.065-1.224-.915-.4-1.646-.336-1.77.465-.008.043-.013.066-.018.135-.068.023-.139.053-.209.064-.43.268-.662.669-.793 1.187-.13.533-.17 1.156-.205 1.869v.003c-.02.334-.17.838-.319 1.35-1.5 1.072-3.58 1.538-5.348.334a2.645 2.645 0 00-.402-.533 1.45 1.45 0 00-.275-.333c.182 0 .338-.03.465-.067a.615.615 0 00.314-.334c.108-.267 0-.697-.345-1.163-.345-.467-.931-.995-1.788-1.521-.63-.4-.986-.87-1.15-1.396-.165-.534-.143-1.085-.015-1.645.245-1.07.873-2.11 1.274-2.763.107-.065.037.135-.408.974-.396.751-1.14 2.497-.122 3.854a8.123 8.123 0 01.647-2.876c.564-1.278 1.743-3.504 1.836-5.268.048.036.217.135.289.202.218.133.38.333.59.465.21.201.477.335.876.335.039.003.075.006.11.006.412 0 .73-.134.997-.268.29-.134.52-.334.74-.4h.005c.467-.135.835-.402 1.044-.7zm2.185 8.958c.037.6.343 1.245.882 1.377.588.134 1.434-.333 1.791-.765l.211-.01c.315-.007.577.01.847.268l.003.003c.208.199.305.53.391.876.085.4.154.78.409 1.066.486.527.645.906.636 1.14l.003-.007v.018l-.003-.012c-.015.262-.185.396-.498.595-.63.401-1.746.712-2.457 1.57-.618.737-1.37 1.14-2.036 1.191-.664.053-1.237-.2-1.574-.898l-.005-.003c-.21-.4-.12-1.025.056-1.69.176-.668.428-1.344.463-1.897.037-.714.076-1.335.195-1.814.12-.465.308-.797.641-.984l.045-.022zm-10.814.049h.01c.053 0 .105.005.157.014.376.055.706.333 1.023.752l.91 1.664.003.003c.243.533.754 1.064 1.189 1.637.434.598.77 1.131.729 1.57v.006c-.057.744-.48 1.148-1.125 1.294-.645.135-1.52.002-2.395-.464-.968-.536-2.118-.469-2.857-.602-.369-.066-.61-.2-.723-.4-.11-.2-.113-.602.123-1.23v-.004l.002-.003c.117-.334.03-.752-.027-1.118-.055-.401-.083-.71.043-.94.16-.334.396-.4.69-.533.294-.135.64-.202.915-.47h.002v-.002c.256-.268.445-.601.668-.838.19-.201.38-.336.663-.336zm7.159-9.074c-.435.201-.945.535-1.488.535-.542 0-.97-.267-1.28-.466-.154-.134-.28-.268-.373-.335-.164-.134-.144-.333-.074-.333.109.016.129.134.199.2.096.066.215.2.36.333.292.2.68.467 1.167.467.485 0 1.053-.267 1.398-.466.195-.135.445-.334.648-.467.156-.136.149-.267.279-.267.128.016.034.134-.147.332a8.097 8.097 0 01-.69.468zm-1.082-1.583V5.64c-.006-.02.013-.042.029-.05.074-.043.18-.027.26.004.063 0 .16.067.15.135-.006.049-.085.066-.135.066-.055 0-.092-.043-.141-.068-.052-.018-.146-.008-.163-.065zm-.551 0c-.02.058-.113.049-.166.066-.047.025-.086.068-.14.068-.05 0-.13-.02-.136-.068-.01-.066.088-.133.15-.133.08-.031.184-.047.259-.005.019.009.036.03.03.05v.02h.003z" />
    </Mark>
  );
}

function WindowsMark(props: MarkProps) {
  return (
    <Mark {...props}>
      <path d="M1 1h10v10H1zM13 1h10v10H13zM1 13h10v10H1zM13 13h10v10H13z" />
    </Mark>
  );
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
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    const sync = () => {
      const id = location.hash.slice(1);
      if (id === "update" || id === "uninstall" || id === "verify") setOpen(id);
    };
    sync();
    addEventListener("hashchange", sync);
    return () => removeEventListener("hashchange", sync);
  }, []);
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
            <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="update-command" />
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
                  <CommandLine command={UNINSTALL_COMMAND} label={t("copy the command")} testId="uninstall-command" />
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
      ),
    },
  ];
  return (
    <Section id="manage">
      <div className={cn(COLUMN, "py-12 max-md:py-10")}>
        <BlockHead title={t("Manage your installation")} lead={t("Update, uninstall, or check what you installed.")} />
        <FaqAccordion
          items={items}
          value={open ? [open] : []}
          onValueChange={(next) => setOpen(next[0] ?? null)}
          className="mx-6 border-t border-border max-md:mx-4"
          itemClassName="scroll-mt-14 border-t border-b-0 border-border first:border-t-0"
          triggerClassName="px-6 py-4 max-md:px-4"
        />
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
  const { t } = useTranslation();
  usePageMeta(
    t("Download Droppen"),
    t("Install Droppen where Claude Code or Codex runs: the Mac app, or one command on Linux, macOS, or WSL. Your phone needs only the browser."),
  );
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
    <PublicLayout
      testId="download"
      signedIn={signedIn}
      mainTestId="environments"
      mainData={{ "data-os": os, "data-detected": detected ?? undefined }}
    >      <Hero availability={availability} release={latest} />
      <ServerSection availability={availability} os={os} arch={onLinux ? arch : null} />
      <BrowserSection />
      <Manage availability={availability} />
    </PublicLayout>
  );
}
