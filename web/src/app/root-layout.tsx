import { Menu } from "@base-ui-components/react/menu";
import { CloudBar } from "@/features/cloud/cloud-bar";
import { SideBySideTip } from "@/features/cloud/onboarding-hints";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Outlet, useNavigate, useRouterState } from "@tanstack/react-router";
import {
  Code,
  FolderTree,
  LayoutGrid,
  MessagesSquare,
  Monitor,
  Moon,
  Plus,
  Settings,
  SquareTerminal,
  Sun,
  X,
  Zap,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { components } from "@/api/schema";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { LOGIN_PATH, SETUP_PATH } from "@/lib/auth";
import { WELCOME_PATH } from "@/features/setup/model";
import { SetupToasts } from "@/features/setup/toast";
import { cn } from "@/lib/cn";
import { BrandMark } from "@/components/brand-mark";
import { RegMark } from "@/components/reg-mark";
import { RailHint } from "@/components/ui/rail-hint";
import { applyFontSize } from "@/lib/font-size";
import { applyThemeMode, NEXT_MODE, storedThemeMode, type ThemeMode } from "@/lib/theme";
import { useTranslation } from "react-i18next";
import { ApiError } from "@/api/errors";
import { AppFrames } from "@/features/apps/app-frames";
import { appLaunchErrorText, CodeServerMissingDialog, isCodeServerMissing } from "@/features/apps/code-server-missing";
import { ScreenFrames } from "@/features/screens/screen-frames";
import { ScreensRail } from "@/features/screens/screens-rail";
import { applyLanguage } from "@/i18n";
import { useDeviceSessions } from "@/features/machines/device-sessions";
import { type DeviceSessions, unreadAcross } from "@/features/machines/needs-you";
import { UnreadSignals } from "@/features/machines/unread-signals";
import { GatewayBoot } from "@/features/gateway/gateway-boot";
import { isCloud } from "@/lib/hub/config";
import { GlobalFeedProvider, useGlobalFeedSelector, useMachineFeedSelector, type GlobalFeedState } from "./global-feed";
import { MachineLink, useRouteMachine, useRouterCaughtUp, useSectionPath } from "./machine-route";
import { PanesProvider, usePanes } from "./panes";
import { PaneTreeView, RoutedViewBoundary } from "./pane-tree-view";
import { ScreensProvider, useScreens } from "./screens";
import { PaneLeafContent } from "./secondary-pane";
import { VersionBanner } from "./version-banner";
import { DesktopUpdate } from "./desktop-update";
import { NAV_ITEM_CLASS } from "./rail-item";
import { PluginRegistryProvider } from "@/plugins/registry";
import { PluginNav } from "@/plugins/plugin-nav";
import { NewSessionHotkey, useNewSessionShortcut, useOpenNewSession } from "@/features/sessions/new-session-view";
import { MachineProvider, useApi, useHomeMachine, useInfoOf, useMachine, useMachineInfo } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";

/** The shell's own sections. Plugins' entries follow them, then Settings. */
const NAV = [
  { to: "/sessions", label: "Sessions", icon: MessagesSquare },
  { to: "/chat", label: "Instant Chat", icon: Zap },
  { to: "/terminals", label: "Terminals", icon: SquareTerminal },
  { to: "/files", label: "Files", icon: FolderTree },
] as const;

const MODE_ICONS: Record<ThemeMode, typeof Sun> = {
  auto: Monitor,
  light: Sun,
  dark: Moon,
};

function ThemeToggle() {
  const { t } = useTranslation();
  const [mode, setMode] = useState<ThemeMode>(storedThemeMode);

  useEffect(() => {
    applyThemeMode(mode);
    if (mode !== "auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const follow = () => applyThemeMode("auto");
    mq.addEventListener("change", follow);
    return () => mq.removeEventListener("change", follow);
  }, [mode]);

  const Icon = MODE_ICONS[mode];
  return (
    <button
      type="button"
      onClick={() => setMode(NEXT_MODE[mode])}
      title={t("theme: {{mode}} (click for {{next}})", { mode: t(mode), next: t(NEXT_MODE[mode]) })}
      className="flex items-center gap-0.5 p-1 text-muted hover:bg-hover hover:text-fg max-[480px]:size-[28px] max-[480px]:justify-center max-[480px]:p-0 max-md:flex-row md:mb-2 md:flex-col"
    >
      <Icon className="size-3.5" />
      <span className="font-mono text-[9px] text-faint max-[480px]:sr-only">{t(mode)}</span>
    </button>
  );
}

const APP_ICONS = { vscode: Code } as const;

type AppInfo = components["schemas"]["AppInfo"];

/**
 * Embedded app instances (VSCode etc.) live in the rail below the main nav:
 * each instance gets its own icon that can be closed via right-click menu or
 * the hover badge; creation happens via the shared rail plus button.
 */
function AppsRail() {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const pathname = useRouterState({ select: (s) => s.location.pathname });

  const { data: apps } = useQuery({
    queryKey: mq(machine, "apps"),
    queryFn: async () => (await api.GET("/api/apps")).data?.apps ?? [],
  });

  const close = useMutation({
    mutationFn: async (id: string) => {
      await api.DELETE("/api/apps/{id}", { params: { path: { id } } });
      return id;
    },
    onSuccess: (id) => {
      void queryClient.invalidateQueries({ queryKey: mq(machine, "apps") });
      if (pathname === `/apps/${id}`) void navigate({ to: "/sessions" });
    },
  });

  return (
    <div className="flex flex-col items-center gap-1 max-md:hidden">
      {apps && apps.length > 0 && <div className="my-1 h-px w-5 bg-border" />}
      {apps?.map((a) => (
        <ContextMenu key={a.id}>
          <ContextMenuTrigger render={<div className="group relative" />}>
            <Link
              to="/apps/$id"
              params={{ id: a.id }}
              title={a.title}
              className="flex size-8 items-center justify-center text-muted hover:bg-hover hover:text-fg"
              activeProps={{ className: "!bg-fg !text-bg" }}
            >
              <Code className="size-4" />
            </Link>
            <button
              type="button"
              title={t("close {{name}}", { name: a.title })}
              onClick={(e) => {
                e.preventDefault();
                close.mutate(a.id);
              }}
              data-signal="danger" className="absolute -top-0.5 -right-0.5 hidden size-3.5 items-center justify-center bg-danger text-white group-hover:flex"
            >
              <X className="size-2.5" />
            </button>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem danger onClick={() => close.mutate(a.id)}>
              {t("close {{name}}", { name: a.title })}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      ))}
    </div>
  );
}

/**
 * The single rail plus button: everything creatable from the rail — screens
 * and embedded app instances — hangs off one menu.
 */
function RailNew({ routeMachine }: { routeMachine: MachineKey }) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { createScreen, creating } = useScreens();
  const openNewSession = useOpenNewSession(routeMachine);
  const newSessionKeys = useNewSessionShortcut();

  // A device without code-server says so in a dialog that says what to install
  // and links to how, instead of a red 「+」 whose tooltip quotes an OS error.
  const [codeServerMissing, setCodeServerMissing] = useState(false);

  const create = useMutation({
    mutationFn: async (type: "vscode") => {
      const res = await api.POST("/api/apps", { body: { type } });
      if (res.error) throw new ApiError(res.error);
      return res.data;
    },
    onError: (error) => {
      if (isCodeServerMissing(error)) setCodeServerMissing(true);
    },
    onSuccess: (info) => {
      // Write-through so the keep-alive frame can mount before the refetch.
      queryClient.setQueryData<AppInfo[]>(mq(machine, "apps"), (old) => [...(old ?? []), info]);
      void queryClient.invalidateQueries({ queryKey: mq(machine, "apps") });
      void navigate({ to: "/apps/$id", params: { id: info.id } });
    },
  });

  const pending = creating || create.isPending;

  return (
    <div className="flex flex-col items-center max-md:hidden">
      <div className="my-1 h-px w-5 bg-border" />
      <Menu.Root>
        <Menu.Trigger
          title={create.isError ? appLaunchErrorText(create.error, t) : t("new…")}
          className={cn(
            "flex size-8 items-center justify-center hover:bg-hover hover:text-fg",
            create.isError ? "text-danger" : "text-muted",
          )}
        >
          <Plus className="size-4" />
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="right" align="end" sideOffset={4} className="z-50 outline-none">
            <Menu.Popup className="min-w-36 border border-border-strong bg-bg py-1 outline-none">
              {/* A session, on the routed view's device (as ⌘N / Ctrl+N opens it). */}
              <Menu.Item
                onClick={() => openNewSession()}
                className="flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left font-mono text-2xs text-fg outline-none data-[highlighted]:bg-hover"
              >
                <MessagesSquare className="size-3.5 text-muted" />
                <span className="flex-1">{t("session")}</span>
                <kbd className="font-mono text-[10px] text-faint">{newSessionKeys}</kbd>
              </Menu.Item>
              <Menu.Item
                disabled={pending}
                onClick={createScreen}
                className="flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left font-mono text-2xs text-fg outline-none data-[highlighted]:bg-hover"
              >
                <LayoutGrid className="size-3.5 text-muted" />
                {t("screen")}
              </Menu.Item>
              {(Object.keys(APP_ICONS) as (keyof typeof APP_ICONS)[]).map((type) => {
                const Icon = APP_ICONS[type];
                return (
                  <Menu.Item
                    key={type}
                    disabled={pending}
                    onClick={() => create.mutate(type)}
                    className="flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left font-mono text-2xs text-fg outline-none data-[highlighted]:bg-hover"
                  >
                    <Icon className="size-3.5 text-muted" />
                    {create.isPending ? t("starting…") : "vscode"}
                  </Menu.Item>
                );
              })}
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      <CodeServerMissingDialog open={codeServerMissing} onOpenChange={setCodeServerMissing} />
    </div>
  );
}

/**
 * droppen.app with two or more devices: once, the tip that a screen's panes
 * can each show a different device, with a way to make one. A line of its
 * own under the bar — never over a pane — that the rail's 「+」 also opens.
 */
function SideBySideTipBar() {
  const { createScreen } = useScreens();
  return <SideBySideTip onScreen={createScreen} />;
}

/** Follows server-side UI prefs (font size, language); boot caches cover first paint. */
function PrefsSync() {
  const api = useApi();
  const machine = useMachine();
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
    staleTime: 30_000,
  });
  useEffect(() => {
    if (prefs) {
      applyFontSize(prefs.fontSize);
      applyLanguage(prefs.language);
    }
  }, [prefs]);
  return null;
}

const selectConnected = (state: GlobalFeedState) => state.connected;
const selectSettings = (state: GlobalFeedState) => state.settings;

function HealthDot({ machine }: { machine: MachineKey }) {
  const { t } = useTranslation();
  // The global websocket doubles as the health signal: connected = online.
  // The address's machine's: the one the rail's sections are about.
  const online = useMachineFeedSelector(machine, selectConnected);
  const data = useMachineFeedSelector(machine, selectSettings);
  return (
    <div
      className="flex items-center gap-1 max-md:flex-row md:mb-1 md:flex-col"
      title={online ? `${t("online")} · ${data?.hostname}` : t("offline")}
    >
      <span data-signal={online ? undefined : "offline"} className={cn("size-1.5", online ? "bg-fg" : "bg-danger")} />
      <span data-signal={online ? undefined : "offline"} className={cn("font-mono text-[9px] max-[480px]:sr-only", online ? "text-muted" : "text-danger")}>{online ? t("on") : t("off")}</span>
    </div>
  );
}

const selectSessions = (state: GlobalFeedState) => state.sessions;

/**
 * The browser tab says how much is waiting to be read — `(3) Droppen` — and
 * so does the app badge: a backgrounded tab answers "anything new?" without
 * being opened, and goes back to its plain name once everything has been read.
 * Summed over every device that runs AI (`features/machines`): this one from
 * its feed, the others (droppen.app) from their copies; locally, this one.
 */
function UnreadTitle() {
  const machine = useMachine();
  const rows = useGlobalFeedSelector(selectSessions);
  // A device not online (droppen.app, on its copy) counts on its own page, not in the tab (`unreadAcross`); locally it always is.
  const away = useMachineInfo().state !== "online";
  const live = useMemo<DeviceSessions>(() => ({ machine, rows, from: "live", asOf: null, ...(away ? { away } : {}) }), [machine, rows, away]);
  const lists = useDeviceSessions(live);
  return <UnreadSignals unread={unreadAcross(lists)} />;
}

/** The routed view plus the keep-alive app iframes and screens layered over it. */
function MainOutlet() {
  return (
    <div className="relative flex min-h-0 min-w-0 flex-1 overflow-hidden">
      <RoutedViewBoundary>
        <Outlet />
      </RoutedViewBoundary>
      <AppFrames />
      <ScreenFrames />
    </div>
  );
}

/**
 * Cmd+D splits the focused pane side-by-side; Cmd+Shift+D stacks it. On a
 * screen route the shortcut edits the screen's persistent layout, elsewhere
 * it edits the ephemeral split around the routed view.
 */
function PaneHotkeys() {
  const panes = usePanes();
  const { activeScreenId, treeApiFor } = useScreens();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "d") {
        e.preventDefault();
        const screenApi = activeScreenId ? treeApiFor(activeScreenId) : null;
        (screenApi ?? panes).splitFocused(e.shiftKey ? "column" : "row");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [panes, activeScreenId, treeApiFor]);
  return null;
}

function useIsDesktop(): boolean {
  const [desktop, setDesktop] = useState(() => window.matchMedia("(min-width: 768px)").matches);
  useEffect(() => {
    const mq = window.matchMedia("(min-width: 768px)");
    const update = () => setDesktop(mq.matches);
    mq.addEventListener("change", update);
    return () => mq.removeEventListener("change", update);
  }, []);
  return desktop;
}

function PaneArea() {
  const panes = usePanes();
  const desktop = useIsDesktop();
  // The routed view's device: what a pane beside it picks from until told otherwise.
  const routeMachine = useRouteMachine();
  // Splitting is a desktop feature: phones always render the routed view alone.
  if (!desktop) {
    return (
      <div className="flex min-h-0 min-w-0 flex-1">
        <MainOutlet />
      </div>
    );
  }
  return (
    <PaneTreeView
      api={panes}
      mainMachine={routeMachine}
      renderLeaf={(leaf) =>
        leaf.content.kind === "main" ? (
          <MainOutlet />
        ) : (
          <PaneLeafContent
            content={leaf.content}
            onChange={(content) => panes.setPaneContent(leaf.id, content)}
            onClose={() => panes.closePane(leaf.id)}
            defaultMachine={routeMachine}
          />
        )
      }
    />
  );
}

export function RootLayout() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const home = useHomeMachine();
  const caughtUp = useRouterCaughtUp();
  if (!caughtUp) return <div className="h-full bg-bg" />;
  // The login page is the whole window: no nav rail to click, and — the point —
  // no global feed socket, no prefs query, nothing that would spend a token this
  // browser does not have yet. Leaving the tree unmounted is also how those
  // things start clean the moment a password is accepted. The setup page the
  // same, all the more: a server with no account answers nothing else.
  if (pathname === LOGIN_PATH || pathname === SETUP_PATH) return <Outlet />;
  // The Droppen app's first-run guide is the whole window too, but signed in:
  // the home's API, its prefs (language) and its feed, and none of the shell.
  if (pathname === WELCOME_PATH) {
    return (
      <MachineProvider key={home} machine={home}>
        <GlobalFeedProvider>
          <PrefsSync />
          <Outlet />
        </GlobalFeedProvider>
      </MachineProvider>
    );
  }
  // Every view below is about the home machine (`lib/machines`) until a page
  // of another machine (`/m/<machine>/…`) or a pane says otherwise. On
  // droppen.app the home is the main device, which moves only when another is
  // made the main device: keyed by it, the whole tree — its feed, panes,
  // screens, the routed view — then starts over on the new one, with no
  // reload, while every other machine's queries, copy and channel stay as
  // they were. Switching the machine in view is only an address. Locally the
  // home never moves.
  return (
    <MachineProvider key={home} machine={home}>
      <GlobalFeedProvider>
        <PluginRegistryProvider>
          <PanesProvider>
            <ScreensProvider>
              <RootLayoutInner />
            </ScreensProvider>
          </PanesProvider>
        </PluginRegistryProvider>
      </GlobalFeedProvider>
    </MachineProvider>
  );
}

function RootLayoutInner() {
  const { t } = useTranslation();
  // The machine the address is about (`/m/<machine>/…`, else the home): the
  // rail's sections and its health dot follow it; screens, apps, plugins and
  // the new menu are the home's wherever the address is.
  const routeMachine = useRouteMachine();
  const home = useHomeMachine();
  // Another device the local app shows only from its copy (S1-7): its
  // sessions open read-only, and the rail's other sections stay this device's.
  const readOnly = useInfoOf(routeMachine).readOnly === true;
  const sectionMachine = readOnly ? home : routeMachine;
  const pathname = useSectionPath();
  // On phones, a transcript view (session or instant chat) has its own back
  // button and needs the height, so the bottom nav collapses there.
  // A session about to be started (`/sessions/new`) is one of them.
  const deepView = /^\/(?:sessions\/[^/]+\/[^/]+|sessions\/new\/?$|chat\/(?:claude|codex|api)\/)/.test(pathname);
  return (
    <div className="flex h-full flex-col">
      {!isCloud() && <GatewayBoot />}
      <PrefsSync />
      {/* The first-run guide's passing line, after it hands over (its first session's note). */}
      <SetupToasts />
      <PaneHotkeys />
      <NewSessionHotkey here={routeMachine} />
      <UnreadTitle />
      <VersionBanner />
      <CloudBar />
      <SideBySideTipBar />
      <div className="flex min-h-0 min-w-0 flex-1 flex-col md:flex-row">
        <nav
          className={cn(
            // min-h (not h) on phones: `pb-safe` adds the home-indicator inset
            // on top of the 3rem bar rather than eating into it.
            "pb-safe flex shrink-0 items-center border-border bg-panel max-[480px]:px-[8px] max-md:order-last max-md:min-h-12 max-md:w-full max-md:flex-row max-md:border-t min-[480px]:max-md:px-3 md:relative md:w-11 md:flex-col md:border-r",
            deepView && "max-md:hidden",
          )}
        >
          <RailHint
            label={t("usage dashboard")}
            render={
              <MachineLink
                machine={sectionMachine}
                to="/stats"
                aria-label={t("usage dashboard")}
                className="flex items-center justify-center hover:opacity-80 max-[480px]:mr-[4px] max-[480px]:size-[28px] min-[480px]:max-md:mr-3 min-[480px]:max-md:size-8 max-md:shrink-0 md:mb-2 md:h-8 md:w-full md:border-b md:border-border"
              />
            }
          >
            <BrandMark size={20} title="Droppen" />
          </RailHint>
          {/* The header seam: rail edge × the 32px header line every panel shares. */}
          <RegMark className="top-[27px] right-[-5px]" />
          {/* On a short window the screens and apps scroll under the fixed
              brand and status ends instead of pushing them off; no scrollbar
              on a 44px rail. Entries keep their size rather than squeeze. */}
          <div className="flex min-w-0 gap-1 max-[480px]:gap-0 max-md:flex-1 max-md:flex-row max-md:justify-evenly md:min-h-0 md:flex-1 md:flex-col md:items-center scrollbar-hidden md:overflow-y-auto md:overscroll-contain md:[&>*]:shrink-0">
            {NAV.map(({ to, label, icon: Icon }) => (
              <RailHint
                key={to}
                label={t(label)}
                render={
                  <MachineLink
                    machine={to === "/sessions" ? routeMachine : sectionMachine}
                    to={to}
                    aria-label={t(label)}
                    className={NAV_ITEM_CLASS}
                    activeProps={{ className: "!bg-fg !text-bg" }}
                  />
                }
              >
                <Icon className="size-4" />
              </RailHint>
            ))}
            <PluginNav className={NAV_ITEM_CLASS} />
            <RailHint
              label={t("Settings")}
              render={
                <MachineLink
                  machine={sectionMachine}
                  to="/settings"
                  aria-label={t("Settings")}
                  className={NAV_ITEM_CLASS}
                  activeProps={{ className: "!bg-fg !text-bg" }}
                />
              }
            >
              <Settings className="size-4" />
            </RailHint>
            <ScreensRail />
            <AppsRail />
            <RailNew routeMachine={routeMachine} />
          </div>
          <div className="flex items-center max-[480px]:gap-[4px] max-md:flex-row min-[480px]:max-md:gap-2 md:flex-col">
            <DesktopUpdate />
            <HealthDot machine={sectionMachine} />
            <ThemeToggle />
          </div>
        </nav>
        <PaneArea />
      </div>
    </div>
  );
}
