/*
 * The views the app loads the first time one is shown, not with the app
 * (#169, "split by route"): a session, a new session, chat, files,
 * terminals, a screen, an app, and the login and setup pages. The boot chunk
 * keeps the shell and the session list, the home; on a weak link every byte
 * of it is part of the wait on a cold start, and of the first load after a
 * release, which rehashes it. Their files are fetched ahead of use once the
 * first page is drawn (`view-prefetch.ts`), so opening one does not wait on
 * the network either: the build finds every module a `lazyRouteComponent`
 * here or in `router.tsx` loads, so a view added to either needs no list.
 *
 * Routes (`router.tsx`) and split panes (`secondary-pane.tsx`) both take them
 * from here. `lazyRouteComponent` reloads the page once when a chunk is gone
 * from the server (a release replaced the build this page was loaded from);
 * any other failure is the routed view's or the pane's error boundary's.
 * The build fails if one of these modules ends up in a boot chunk again
 * (`ON_DEMAND` in `vite.config.ts`).
 */
import { lazyRouteComponent } from "@tanstack/react-router";

const sessionDetail = () => import("@/features/sessions/session-detail");
const newSession = () => import("@/features/sessions/new-session-view");
const apiChat = () => import("@/features/chat/api-chat-view");
const files = () => import("@/features/files/files-view");

export const SessionDetail = lazyRouteComponent(sessionDetail, "SessionDetail");
export const SessionDetailInner = lazyRouteComponent(sessionDetail, "SessionDetailInner");
export const SubagentDetail = lazyRouteComponent(() => import("@/features/sessions/subagent-view"), "SubagentDetail");
export const NewSessionRoute = lazyRouteComponent(newSession, "NewSessionRoute");
export const NewSessionView = lazyRouteComponent(newSession, "NewSessionView");
export const ChatHome = lazyRouteComponent(() => import("@/features/chat/chat-home"), "ChatHome");
export const ClaudeChatView = lazyRouteComponent(() => import("@/features/chat/claude-chat-view"), "ClaudeChatView");
export const CodexChatView = lazyRouteComponent(() => import("@/features/chat/claude-chat-view"), "CodexChatView");
export const ApiChatView = lazyRouteComponent(apiChat, "ApiChatView");
export const ApiChatViewInner = lazyRouteComponent(apiChat, "ApiChatViewInner");
export const FilesView = lazyRouteComponent(files, "FilesView");
export const FilesBrowser = lazyRouteComponent(files, "FilesBrowser");
export const TerminalsView = lazyRouteComponent(() => import("@/features/terminals/terminals-view"), "TerminalsView");
export const ScreenView = lazyRouteComponent(() => import("@/features/screens/screen-view"), "ScreenView");
export const AppView = lazyRouteComponent(() => import("@/features/apps/app-view"), "AppView");
export const LoginView = lazyRouteComponent(() => import("@/features/auth/login-view"), "LoginView");
export const SetupView = lazyRouteComponent(() => import("@/features/auth/setup-view"), "SetupView");
