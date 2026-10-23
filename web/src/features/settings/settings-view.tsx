import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearch } from "@tanstack/react-router";
import { MachineLink, useMachineNavigate } from "@/app/machine-route";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { useFeedSessions, useFeedSettings } from "@/app/global-feed";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { Content, Panel, PanelHeader } from "@/components/panel";
import { useShowHiddenFiles } from "@/features/files/use-files-prefs";
import { AccessSection } from "@/features/settings/access-section";
import { AccountsSection } from "@/features/settings/accounts-section";
import { ScheduleSection } from "@/features/settings/schedule-section";
import { ALL_SECTIONS, askedSection, type SectionId } from "@/features/settings/sections";
import { SetupSection } from "@/features/setup/settings-section";
import { cn } from "@/lib/cn";
import { formatTime } from "@/lib/format";
import {
  installPromptAvailable,
  installState,
  isIos,
  isStandalone,
  promptInstall,
  subscribeInstall,
} from "@/lib/install";
import { disablePush, enablePush, pushState, type PushState } from "@/lib/push";
import { Sheet } from "@/components/sheet";
import { PluginsSection } from "@/plugins/plugins-section";
import { DevicePicker } from "@/features/machines/device-picker";
import { useApi, useHomeMachine, useMachine, useOpenMachines } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

type Prefs = components["schemas"]["Prefs"];

// Use the available pane width: settings can be narrow even on a desktop.
const preferenceRow =
  "flex max-w-xl flex-col items-start gap-2 @min-[36rem]/settings:flex-row @min-[36rem]/settings:items-center @min-[36rem]/settings:justify-between @min-[36rem]/settings:gap-4";

/** Exported for the render tests (`test/cross-device-render.test.tsx`). */
export function PreferencesSection() {
  const api = useApi();
  const machine = useMachine();
  // Font size and language are applied from the main device's prefs only.
  const home = useHomeMachine();
  const onMain = machine === home;
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
  });
  // Not a server pref: what the file browser hides is a per-browser habit, and
  // it takes effect in an open file list without a save round-trip.
  const [showHiddenFiles, setShowHiddenFiles] = useShowHiddenFiles();

  const save = useMutation({
    mutationFn: async (next: Prefs) => {
      const res = await api.PUT("/api/prefs", { body: next });
      return res.data;
    },
    onSuccess: (data) => {
      if (data) queryClient.setQueryData(mq(machine, "prefs"), data);
    },
  });

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
        {t("preferences")}
      </h2>
      <div className={preferenceRow}>
        <div>
          <div className="text-xs">{t("session list grouping")}</div>
          <div className="text-2xs text-faint">{t("group sessions by project directory")}</div>
        </div>
        {prefs ? (
          <Tabs
            className="max-w-full shrink-0"
            value={prefs.sessionGrouping}
            onValueChange={(v) => save.mutate({ ...prefs, sessionGrouping: v as Prefs["sessionGrouping"] })}
          >
            <TabsList className="w-40">
              <TabsTab value="flat">{t("flat")}</TabsTab>
              <TabsTab value="directory">{t("directory")}</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-2xs text-faint">…</span>
        )}
      </div>
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className="text-xs">{t("message display")}</div>
          <div className="text-2xs text-faint">{t("chat bubbles or a flat list")}</div>
        </div>
        {prefs ? (
          <Tabs
            className="max-w-full shrink-0"
            value={prefs.transcriptStyle}
            onValueChange={(v) => save.mutate({ ...prefs, transcriptStyle: v as Prefs["transcriptStyle"] })}
          >
            <TabsList className="w-40">
              <TabsTab value="flat">{t("flat")}</TabsTab>
              <TabsTab value="bubble">{t("bubbles")}</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-2xs text-faint">…</span>
        )}
      </div>
      {onMain ? (
        <>
          <div className={cn(preferenceRow, "mt-3")}>
            <div>
              <div className="text-xs">{t("font size")}</div>
              <div className="text-2xs text-faint">{t("scales the whole UI, synced across devices")}</div>
            </div>
            {prefs ? (
              <Tabs
                className="max-w-full shrink-0"
                value={prefs.fontSize}
                onValueChange={(v) => save.mutate({ ...prefs, fontSize: v as Prefs["fontSize"] })}
              >
                <TabsList className="w-56">
                  <TabsTab value="small">sm</TabsTab>
                  <TabsTab value="default">md</TabsTab>
                  <TabsTab value="large">lg</TabsTab>
                  <TabsTab value="xlarge">xl</TabsTab>
                </TabsList>
              </Tabs>
            ) : (
              <span className="font-mono text-2xs text-faint">…</span>
            )}
          </div>
          <div className={cn(preferenceRow, "mt-3")}>
            <div>
              <div className="text-xs">{t("language")}</div>
              <div className="text-2xs text-faint">{t("auto follows the browser language")}</div>
            </div>
            {prefs ? (
              <Tabs
                className="max-w-full shrink-0"
                value={prefs.language}
                onValueChange={(v) => save.mutate({ ...prefs, language: v as Prefs["language"] })}
              >
                <TabsList className="w-72 max-w-full">
                  <TabsTab value="auto">{t("auto")}</TabsTab>
                  <TabsTab value="en">EN</TabsTab>
                  <TabsTab value="zh-CN">简体</TabsTab>
                  <TabsTab value="zh-TW">繁體</TabsTab>
                  <TabsTab value="ja">日本語</TabsTab>
                </TabsList>
              </Tabs>
            ) : (
              <span className="font-mono text-2xs text-faint">…</span>
            )}
          </div>
        </>
      ) : (
        // The page's font size and language are the main device's prefs
        // (`PrefsSync`): set here they would be saved on this device and
        // never applied.
        <div data-appearance-on-main="" className={cn(preferenceRow, "mt-3")}>
          <div className="text-2xs text-faint">{t("Font size and language follow the main device: change them in its settings.")}</div>
          <MachineLink machine={home} to="/settings" className="shrink-0 font-mono text-2xs text-accent hover:underline">
            {t("main device settings")}
          </MachineLink>
        </div>
      )}
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className="text-xs">{t("composer send shortcut")}</div>
          <div className="text-2xs text-faint">
            {prefs?.composerSend === "modifier"
              ? t("⌘/Ctrl+Enter sends · Enter inserts a newline")
              : t("Enter sends · Shift+Enter inserts a newline")}
          </div>
        </div>
        {prefs ? (
          <Tabs
            className="max-w-full shrink-0"
            value={prefs.composerSend}
            onValueChange={(v) => save.mutate({ ...prefs, composerSend: v as Prefs["composerSend"] })}
          >
            <TabsList className="w-40">
              <TabsTab value="enter">{t("enter")}</TabsTab>
              <TabsTab value="modifier">⌘+enter</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-2xs text-faint">…</span>
        )}
      </div>
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className="text-xs">{t("show hidden files")}</div>
          <div className="text-2xs text-faint">
            {t("dot-files in the file browser · this device only")}
          </div>
        </div>
        <Tabs
          className="max-w-full shrink-0"
          value={showHiddenFiles ? "show" : "hide"}
          onValueChange={(v) => setShowHiddenFiles(v === "show")}
        >
          <TabsList className="w-40">
            <TabsTab value="hide">{t("hide")}</TabsTab>
            <TabsTab value="show">{t("show")}</TabsTab>
          </TabsList>
        </Tabs>
      </div>
    </section>
  );
}

/**
 * Web Push opt-in. The subscription belongs to this browser, not the account,
 * so the switch only ever speaks for the device it is being tapped on.
 */
function NotificationsSection() {
  const machine = useMachine();
  const { t } = useTranslation();
  const [state, setState] = useState<PushState | "loading">("loading");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true;
    void pushState().then((s) => {
      if (alive) setState(s);
    });
    return () => {
      alive = false;
    };
  }, []);

  const toggle = async () => {
    setBusy(true);
    setError("");
    try {
      if (state === "on") await disablePush(machine);
      else await enablePush(machine);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
      setState(await pushState());
    }
  };

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
        {t("notifications")}
      </h2>
      <div className="flex max-w-xl items-center justify-between gap-4">
        <div className="min-w-0">
          <div className="text-xs">{t("push notifications")}</div>
          <div className="text-2xs text-faint">
            {t("Ping this browser when a session finishes a turn or asks for input.")}
          </div>
          {state === "denied" && (
            <div data-signal="warn" className="mt-0.5 text-2xs text-warn-fg">
              {t("Blocked for this site — re-allow notifications in the browser's site settings.")}
            </div>
          )}
          {state === "unsupported" && (
            <div className="mt-0.5 text-2xs text-faint">
              {t("Unavailable here: push needs a service worker, which means a production build over https (or localhost).")}
            </div>
          )}
          {error !== "" && <div data-signal="failed" className="mt-0.5 font-mono text-2xs text-danger">{error}</div>}
        </div>
        {state === "loading" ? (
          <span className="font-mono text-2xs text-faint">…</span>
        ) : state === "on" || state === "off" ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => void toggle()}
            className={cn(
              "flex h-7 w-24 shrink-0 items-center justify-center rounded font-mono text-2xs disabled:opacity-40",
              state === "on"
                ? "border border-border text-muted hover:bg-hover hover:text-fg"
                : "bg-fg text-bg hover:bg-fg/85",
            )}
          >
            {busy ? t("saving…") : state === "on" ? t("disable") : t("enable")}
          </button>
        ) : (
          <span className="shrink-0 font-mono text-2xs text-faint">{t("unavailable")}</span>
        )}
      </div>
    </section>
  );
}

/**
 * Bark: the channel a *session* reaches for when it decides something is worth
 * interrupting you for. The other half of NotificationsSection above, and
 * deliberately a separate one — that switch is this browser's, and this is the
 * server's, configured once and shared by every device the user carries.
 *
 * The device key is write-only from here. The server answers with a mask
 * (`https://api.day.app/••••Wc31`), so a page that has been left open, or
 * anything that scrapes it, cannot send to the user's phone.
 */
function PhoneNotificationsSection() {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: config } = useQuery({
    queryKey: mq(machine, "notify-config"),
    queryFn: async () => (await api.GET("/api/notify/config")).data,
  });
  const [endpoint, setEndpoint] = useState("");
  const [baseUrl, setBaseUrl] = useState<string | null>(null);
  const [sent, setSent] = useState(false);

  const save = useMutation({
    mutationFn: async (patch: components["schemas"]["NotifyConfigUpdate"]) => {
      const res = await api.PUT("/api/notify/config", { body: patch });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (data) => {
      if (data) queryClient.setQueryData(mq(machine, "notify-config"), data);
      setEndpoint("");
      setBaseUrl(null);
    },
  });

  const test = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/notify/test", {});
      if (res.error) throw new Error(res.error.error);
    },
    onSuccess: () => setSent(true),
  });

  const dirty = endpoint.trim() !== "" || baseUrl !== null;
  const armed = config?.hasEndpoint ?? false;
  const field =
    "h-6.5 w-full rounded border border-border bg-bg px-2 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none";

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
        {t("phone notifications")}
      </h2>
      <div className="flex max-w-xl flex-col gap-2">
        <p className="text-2xs text-faint">
          {t(
            "Lets a session interrupt you on purpose — when it is blocked on a decision, has found something you would want to stop for, or has finished. Delivered by Bark on iOS; the device key stays on the server and is never given to the agent.",
          )}
        </p>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">
            {t("bark device key or endpoint")}{" "}
            {armed && t("· saved (enter a new one to replace)")}
          </span>
          <input
            type="password"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder={config?.endpointMask || "https://api.day.app/…"}
            className={field}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("address your phone opens")}</span>
          <input
            value={baseUrl ?? config?.baseUrl ?? ""}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder={window.location.origin}
            className={field}
          />
          <span className="text-2xs text-faint">
            {t("Tapping a notification opens the session it came from. Leave it empty for no link.")}
          </span>
        </label>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            disabled={!dirty || save.isPending}
            onClick={() => {
              setSent(false);
              save.mutate({
                ...(endpoint.trim() !== "" ? { endpoint: endpoint.trim() } : {}),
                ...(baseUrl !== null ? { baseUrl } : {}),
                // Saving a key for the first time is the moment somebody means
                // to turn this on; asking them to flip a second switch
                // afterwards is a step whose only outcome is being forgotten.
                ...(!armed && endpoint.trim() !== "" ? { enabled: true } : {}),
              });
            }}
            data-variant="primary" className="flex h-7 w-24 items-center justify-center bg-fg text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
          >
            {save.isPending ? t("saving…") : t("save")}
          </button>
          <button
            type="button"
            disabled={!armed || dirty || test.isPending}
            onClick={() => {
              setSent(false);
              test.mutate();
            }}
            title={dirty ? t("save first") : undefined}
            className="flex h-7 items-center justify-center rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
          >
            {test.isPending ? t("sending…") : t("send test")}
          </button>
          {armed && (
            <button
              type="button"
              disabled={save.isPending}
              onClick={() => {
                setSent(false);
                save.mutate({ enabled: !config?.enabled });
              }}
              className={cn(
                "ml-auto flex h-7 w-24 items-center justify-center rounded font-mono text-2xs disabled:opacity-40",
                config?.enabled
                  ? "border border-border text-muted hover:bg-hover hover:text-fg"
                  : "bg-fg text-bg hover:bg-fg/85",
              )}
            >
              {config?.enabled ? t("disable") : t("enable")}
            </button>
          )}
        </div>
        {armed && !config?.enabled && (
          <span data-signal="warn" className="font-mono text-2xs text-warn-fg">
            {t("Turned off — sessions are not offered the notification skill.")}
          </span>
        )}
        {sent && <span data-signal="ok" className="font-mono text-2xs text-ok">{t("test notification sent")}</span>}
        {save.isError && <span data-signal="failed" className="font-mono text-2xs text-danger">{save.error.message}</span>}
        {test.isError && <span data-signal="failed" className="font-mono text-2xs text-danger">{test.error.message}</span>}
      </div>
    </section>
  );
}

/**
 * Add-to-home-screen offer. Chromium fires `beforeinstallprompt` and lib/install.ts
 * has been holding it since boot; iOS has no install API at all, so the only
 * thing to offer there is the Share-sheet recipe. Anywhere else (a desktop
 * browser, or a prompt already spent) there is nothing useful to say and the
 * section is not rendered.
 */
function InstallSection() {
  const { t } = useTranslation();
  const [promptable, setPromptable] = useState(installPromptAvailable);

  useEffect(() => subscribeInstall(() => setPromptable(installPromptAvailable())), []);

  const state = installState({ standalone: isStandalone(), promptable, ios: isIos() });
  if (state === "hidden") return null;

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
        {t("install")}
      </h2>
      <div className="flex max-w-xl items-center justify-between gap-4">
        <div className="min-w-0">
          <div className="text-xs">{t("home screen app")}</div>
          <div className="text-2xs text-faint">
            {t("Opens full-screen with its own icon, and can badge it when a session needs you.")}
          </div>
          {state === "ios-hint" && (
            <div className="mt-0.5 text-2xs text-faint">
              {t("On iOS: tap the Share button in Safari, then “Add to Home Screen”.")}
            </div>
          )}
        </div>
        {state === "promptable" ? (
          <button
            type="button"
            onClick={() => void promptInstall()}
            data-variant="primary" className="flex h-7 w-24 shrink-0 items-center justify-center bg-fg text-xs font-medium text-bg hover:bg-fg/85"
          >
            {t("install app")}
          </button>
        ) : state === "installed" ? (
          <span data-signal="ok" className="shrink-0 font-mono text-2xs text-ok">{t("installed")}</span>
        ) : null}
      </div>
    </section>
  );
}

/** OpenAI-compatible endpoint (OpenRouter etc.) for API-backed instant chats. */
function ChatProviderSection() {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: provider } = useQuery({
    queryKey: mq(machine, "chat-provider"),
    queryFn: async () => (await api.GET("/api/chat-provider")).data,
  });
  const [baseUrl, setBaseUrl] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: async () => {
      const res = await api.PUT("/api/chat-provider", {
        body: {
          ...(baseUrl !== null ? { baseUrl } : {}),
          ...(apiKey !== "" ? { apiKey } : {}),
          ...(model !== null ? { model } : {}),
        },
      });
      return res.data;
    },
    onSuccess: (data) => {
      if (data) queryClient.setQueryData(mq(machine, "chat-provider"), data);
      void queryClient.invalidateQueries({ queryKey: mq(machine, "chat-provider-models") });
      setApiKey("");
      setBaseUrl(null);
      setModel(null);
    },
  });

  const dirty = baseUrl !== null || apiKey !== "" || model !== null;
  const field =
    "h-6.5 w-full rounded border border-border bg-bg px-2 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none";

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
        {t("instant chat · api provider")}
      </h2>
      <div className="flex max-w-xl flex-col gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("base url (openai-compatible)")}</span>
          <input
            value={baseUrl ?? provider?.baseUrl ?? ""}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="https://openrouter.ai/api/v1"
            className={field}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">
            {t("api key")} {provider?.hasKey && t("· saved (enter a new one to replace)")}
          </span>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={provider?.hasKey ? "••••••••" : "sk-or-…"}
            className={field}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("default model")}</span>
          <input
            value={model ?? provider?.model ?? ""}
            onChange={(e) => setModel(e.target.value)}
            placeholder="openai/gpt-5.5"
            className={field}
          />
        </label>
        <button
          type="button"
          disabled={!dirty || save.isPending}
          onClick={() => save.mutate()}
          data-variant="primary" className="flex h-7 w-24 items-center justify-center bg-fg text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
        >
          {t("save")}
        </button>
      </div>
    </section>
  );
}

function GeneralSettings() {
  const { t } = useTranslation();
  // Server info and counts arrive over the global websocket feed.
  const data = useFeedSettings();
  const sessions = useFeedSessions();
  const isPending = !data;
  const claudeCount = sessions?.filter((s) => s.source === "claude").length ?? 0;
  const codexCount = sessions?.filter((s) => s.source === "codex").length ?? 0;

  const rows: [string, string][] = data
    ? [
        ["hostname", data.hostname],
        ["platform", data.platform],
        ["node", data.nodeVersion],
        ["version", data.serverVersion ?? "—"],
        ["home", data.homeDir],
        ["claude dir", data.claudeDir],
        ["codex dir", data.codexDir],
        [t("claude sessions"), String(claudeCount)],
        [t("codex sessions"), String(codexCount)],
        [t("server started"), formatTime(data.serverStartedAt)],
      ]
    : [];

  return (
    <Sheet>
      <SetupSection />
      <PreferencesSection />
      <NotificationsSection />
      <PhoneNotificationsSection />
      <InstallSection />
      <ChatProviderSection />
      <section className="p-3">
        <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
          {t("server info")}
        </h2>
        {isPending && <Spinner />}
        {data && (
          <table className="w-full max-w-xl">
            <tbody>
              {rows.map(([k, v]) => (
                <tr key={k} className="border-b border-border">
                  <td className="py-1 pr-4 align-top font-mono text-2xs whitespace-nowrap text-faint">{k}</td>
                  <td className="py-1 font-mono text-xs break-all">{v}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </Sheet>
  );
}

export function SettingsView() {
  const { t } = useTranslation();
  // `?section=plugins` opens one directly (a disabled plugin's page links here).
  const search = useSearch({ strict: false }) as { section?: string };
  const navigate = useMachineNavigate();
  const asked = askedSection(search.section);
  // Whose settings these are: the address's device (`/m/<device>/settings`,
  // else the home). With more than one device, the header can go to another's.
  const machine = useMachine();
  const several = useOpenMachines().length > 1;
  const devices = several && (
    <DevicePicker writable value={machine} onSelect={(next) => void navigate({ to: "/settings", machine: next, search: section === "general" ? {} : { section } })} />
  );
  const [chosen, setChosen] = useState<SectionId>(asked ?? "general");
  const section = asked ?? chosen;
  const setSection = (next: SectionId) => {
    setChosen(next);
    if (search.section) void navigate({ to: "/settings", search: {}, replace: true });
  };
  return (
    <>
      <Panel className="max-md:hidden">
        <PanelHeader title={t("Settings")} />
        <div className="flex flex-col">
          {ALL_SECTIONS.map((id) => (
            <button
              key={id}
              type="button"
              onClick={() => setSection(id)}
              className={cn(
                "flex h-8 items-center border-b border-border px-3 text-left text-[13px] hover:bg-hover",
                section === id ? "!bg-fg font-medium text-bg" : "text-fg",
              )}
            >
              {t(id)}
            </button>
          ))}
        </div>
      </Panel>
      <Content className="@container/settings">
        <header className="flex min-h-8 shrink-0 flex-col justify-center gap-1 border-b border-border bg-bg px-4 py-1 md:h-8 md:flex-row md:items-center md:justify-between md:py-0">
          <span className="text-[13px] font-semibold max-md:hidden">{t(section)}</span>
          {devices && <span className="flex min-w-0 justify-end max-md:hidden">{devices}</span>}
          <div
            className="min-w-0 overflow-x-auto md:hidden"
            onFocus={(event) => {
              // Handle the bubbled focus after Base UI updates the active tab.
              const tab = event.target;
              if (tab instanceof HTMLElement) tab.scrollIntoView({ block: "nearest", inline: "nearest" });
            }}
          >
            <Tabs value={section} onValueChange={(v) => setSection(v as SectionId)}>
              <TabsList className="w-max min-w-full">
                {ALL_SECTIONS.map((id) => (
                  <TabsTab key={id} value={id} className="shrink-0 whitespace-nowrap">
                    {t(id)}
                  </TabsTab>
                ))}
              </TabsList>
            </Tabs>
          </div>
          {devices && <span className="flex min-w-0 md:hidden">{devices}</span>}
        </header>
        <div className="flex min-h-0 flex-1 flex-col">
          {section === "general" ? (
            <GeneralSettings />
          ) : section === "accounts" ? (
            <AccountsSection />
          ) : section === "access" ? (
            <AccessSection />
          ) : section === "plugins" ? (
            <Sheet>
              <PluginsSection />
            </Sheet>
          ) : (
            <ScheduleSection />
          )}
        </div>
      </Content>
    </>
  );
}
