import {
  createRootRoute,
  createRoute,
  createRouter,
  lazyRouteComponent,
  redirect,
} from "@tanstack/react-router";
import { MachineRoute } from "./machine-route";
import { RootLayout } from "./root-layout";
import { AppView } from "@/features/apps/app-view";
import { LoginView } from "@/features/auth/login-view";
import { SetupView } from "@/features/auth/setup-view";
import { ApiChatView } from "@/features/chat/api-chat-view";
import { ChatHome } from "@/features/chat/chat-home";
import { ChatLayout } from "@/features/chat/chat-layout";
import { ClaudeChatView, CodexChatView } from "@/features/chat/claude-chat-view";
import { FilesView, type FilesSearch } from "@/features/files/files-view";
import { ScreenView } from "@/features/screens/screen-view";
import { SessionDetail } from "@/features/sessions/session-detail";
import { NewSessionRoute, validateNewSessionSearch } from "@/features/sessions/new-session-view";
import { SessionsIndex, SessionsLayout } from "@/features/sessions/sessions-layout";
import { TerminalsView, type TerminalsSearch } from "@/features/terminals/terminals-view";
import { firstRunRedirect } from "@/features/setup/auto-open";
import { validateWelcomeSearch } from "@/features/setup/model";
import { authRedirect, authStatus, clearToken, isDesktopApp, LOGIN_PATH, primeDownloadTicket } from "@/lib/auth";
import { pluginAlias } from "@/plugins/aliases";
import { PluginRoute } from "@/plugins/plugin-route";

/**
 * The one thing every route waits on: whether this server wants a login, or
 * its first account.
 *
 * `authStatus()` answers from a module-level cache after the first call, so this
 * is one request per page load and nothing per navigation — after a
 * `#local-open=` link has been traded for a session, which it waits for. A
 * server with authentication switched off answers `authRequired: false` and
 * the guard is a no-op forever after; nobody signed in to a device with no
 * account yet (`setupRequired`) is sent to the setup page. Where each answer
 * goes is `authRedirect`'s.
 */
const rootRoute = createRootRoute({
  component: RootLayout,
  beforeLoad: async ({ location }) => {
    // An old top-level path a bundled plugin still answers to (`/tasks`): its
    // page lives under `/p/<id>/` now, and bookmarks and installed home-screen
    // apps keep working. Only the plugins this build bundles have aliases.
    const alias = pluginAlias(location.pathname);
    if (alias) throw redirect({ to: "/p/$plugin/$", params: { plugin: alias, _splat: "" }, replace: true });
    const status = await authStatus();
    if (status.authRequired && status.user) {
      // Downloads and `<img src>` cannot send a header, so the first paint is
      // given a ticket to build URLs from rather than a first render's worth of
      // 401s. Only the first: a picture drawn before the ticket lands now
      // repaints itself when it does, and a download resolves its own at click
      // time, so this is an optimisation rather than the load-bearing thing it
      // used to be. After the first call the ticket is cached and it resolves
      // immediately.
      await primeDownloadTicket();
    } else if (status.authRequired) {
      // `user` is the server's verdict on the token this request just carried,
      // so a stored token it does not recognise is settled here rather than by
      // the first 401 — no flash of an app that cannot load anything.
      clearToken();
    }
    const go = authRedirect(status, location.pathname, location.href);
    if (go?.to === LOGIN_PATH) throw redirect({ to: LOGIN_PATH, search: go.next ? { next: go.next } : {} });
    if (go) throw redirect({ to: go.to });
    // The Droppen app's first-run guide opens by itself on the page load's
    // first route, when that is the home and the guide never started
    // (`features/setup/auto-open.ts`); asked here, so the list never flashes.
    // Only for someone signed in, or a server with login switched off: the
    // login and setup pages are drawn without asking.
    if ((status.user || !status.authRequired) && (await firstRunRedirect(location.pathname))) {
      throw redirect({ to: "/welcome", replace: true });
    }
  },
});

interface LoginSearch {
  /** Where to go once the password is accepted. */
  next?: string;
}

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "login",
  component: LoginView,
  validateSearch: (search: Record<string, unknown>): LoginSearch => ({
    next: typeof search.next === "string" ? search.next : undefined,
  }),
});

/** The first account of a server with none yet, made with its one-time setup code. */
const setupRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "setup",
  component: SetupView,
});

/**
 * The Droppen app's first-run guide (`features/setup/`): the whole window,
 * in the app only — anywhere else it is the session list.
 */
const welcomeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "welcome",
  validateSearch: validateWelcomeSearch,
  beforeLoad: () => {
    if (!isDesktopApp()) throw redirect({ to: "/sessions", replace: true });
  },
  component: lazyRouteComponent(() => import("@/features/setup/welcome-view"), "WelcomeView"),
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({ to: "/sessions" });
  },
});

const sessionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "sessions",
  component: SessionsLayout,
});

const sessionsIndexRoute = createRoute({
  getParentRoute: () => sessionsRoute,
  path: "/",
  component: SessionsIndex,
});

const sessionDetailRoute = createRoute({
  getParentRoute: () => sessionsRoute,
  path: "$source/$id",
  component: SessionDetail,
});

/** A session about to be started: the empty session view (`new-session-view.tsx`). */
const sessionNewRoute = createRoute({
  getParentRoute: () => sessionsRoute,
  path: "new",
  component: NewSessionRoute,
  validateSearch: validateNewSessionSearch,
});

const chatRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "chat",
  component: ChatLayout,
});

const chatIndexRoute = createRoute({
  getParentRoute: () => chatRoute,
  path: "/",
  component: ChatHome,
});

const chatClaudeRoute = createRoute({
  getParentRoute: () => chatRoute,
  path: "claude/$id",
  component: ClaudeChatView,
});

const chatCodexRoute = createRoute({
  getParentRoute: () => chatRoute,
  path: "codex/$id",
  component: CodexChatView,
});

const chatApiRoute = createRoute({
  getParentRoute: () => chatRoute,
  path: "api/$id",
  component: ApiChatView,
});

const filesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "files",
  component: FilesView,
  validateSearch: (search: Record<string, unknown>): FilesSearch => ({
    path: typeof search.path === "string" ? search.path : undefined,
    file: typeof search.file === "string" ? search.file : undefined,
  }),
});

const terminalsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "terminals",
  component: TerminalsView,
  validateSearch: (search: Record<string, unknown>): TerminalsSearch => ({
    id: typeof search.id === "string" ? search.id : undefined,
  }),
});

export interface SettingsSearch {
  /** Which section to open (`plugins`, …). */
  section?: string;
}

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "settings",
  validateSearch: (search: Record<string, unknown>): SettingsSearch => ({
    section: typeof search.section === "string" ? search.section : undefined,
  }),
  // Lazy like the views below: settings and stats are a sixth of the app's own
  // code, rarely opened, and on a weak link every byte of the app chunk is
  // part of the wait on a cold start — and of the first load after a release,
  // which rehashes that chunk.
  component: lazyRouteComponent(() => import("@/features/settings/settings-view"), "SettingsView"),
});

/**
 * Every plugin page: `/p/<plugin id>/<the plugin's own path>`. Which plugins
 * exist is the build's business (`virtual:droppen/bundled-plugins`), which are
 * on is the instance's; the route itself is the same in every build.
 */
const pluginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "p/$plugin/$",
  component: PluginRoute,
});

const statsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "stats",
  component: lazyRouteComponent(() => import("@/features/stats/stats-view"), "StatsView"),
});

/** The app's own update, its notes and the restart (`features/updates/`). */
const updatesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "updates",
  component: lazyRouteComponent(() => import("@/features/updates/updates-view"), "UpdatesView"),
});

const appRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "apps/$id",
  component: AppView,
});

const screenRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "screens/$id",
  component: ScreenView,
});

/*
 * Another machine's pages: the same sections under `/m/<machine>`
 * (`machine-route.tsx`, `lib/machines/routes.ts`). The paths above are the
 * home's and stay as they are; these use the same views, which read their
 * params and search without naming a route.
 */
const machineRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "m/$machine",
  component: MachineRoute,
});

const machineIndexRoute = createRoute({
  getParentRoute: () => machineRoute,
  path: "/",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/m/$machine/sessions", params, replace: true });
  },
});

const machineSessionsRoute = createRoute({
  getParentRoute: () => machineRoute,
  path: "sessions",
  component: SessionsLayout,
});

const machineSessionsIndexRoute = createRoute({
  getParentRoute: () => machineSessionsRoute,
  path: "/",
  component: SessionsIndex,
});

const machineSessionDetailRoute = createRoute({
  getParentRoute: () => machineSessionsRoute,
  path: "$source/$id",
  component: SessionDetail,
});

const machineSessionNewRoute = createRoute({
  getParentRoute: () => machineSessionsRoute,
  path: "new",
  component: NewSessionRoute,
  validateSearch: validateNewSessionSearch,
});

const machineChatRoute = createRoute({
  getParentRoute: () => machineRoute,
  path: "chat",
  component: ChatLayout,
});

const machineChatIndexRoute = createRoute({
  getParentRoute: () => machineChatRoute,
  path: "/",
  component: ChatHome,
});

const machineChatClaudeRoute = createRoute({
  getParentRoute: () => machineChatRoute,
  path: "claude/$id",
  component: ClaudeChatView,
});

const machineChatCodexRoute = createRoute({
  getParentRoute: () => machineChatRoute,
  path: "codex/$id",
  component: CodexChatView,
});

const machineChatApiRoute = createRoute({
  getParentRoute: () => machineChatRoute,
  path: "api/$id",
  component: ApiChatView,
});

const machineFilesRoute = createRoute({
  getParentRoute: () => machineRoute,
  path: "files",
  component: FilesView,
  validateSearch: filesRoute.options.validateSearch,
});

const machineTerminalsRoute = createRoute({
  getParentRoute: () => machineRoute,
  path: "terminals",
  component: TerminalsView,
  validateSearch: terminalsRoute.options.validateSearch,
});

const machineSettingsRoute = createRoute({
  getParentRoute: () => machineRoute,
  path: "settings",
  validateSearch: settingsRoute.options.validateSearch,
  component: lazyRouteComponent(() => import("@/features/settings/settings-view"), "SettingsView"),
});

const machineStatsRoute = createRoute({
  getParentRoute: () => machineRoute,
  path: "stats",
  component: lazyRouteComponent(() => import("@/features/stats/stats-view"), "StatsView"),
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  loginRoute,
  setupRoute,
  welcomeRoute,
  sessionsRoute.addChildren([sessionsIndexRoute, sessionNewRoute, sessionDetailRoute]),
  chatRoute.addChildren([chatIndexRoute, chatClaudeRoute, chatCodexRoute, chatApiRoute]),
  terminalsRoute,
  filesRoute,
  settingsRoute,
  statsRoute,
  updatesRoute,
  pluginRoute,
  appRoute,
  screenRoute,
  machineRoute.addChildren([
    machineIndexRoute,
    machineSessionsRoute.addChildren([machineSessionsIndexRoute, machineSessionNewRoute, machineSessionDetailRoute]),
    machineChatRoute.addChildren([machineChatIndexRoute, machineChatClaudeRoute, machineChatCodexRoute, machineChatApiRoute]),
    machineTerminalsRoute,
    machineFilesRoute,
    machineSettingsRoute,
    machineStatsRoute,
  ]),
]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
