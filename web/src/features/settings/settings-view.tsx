import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearch } from "@tanstack/react-router";
import { MachineLink, useMachineNavigate } from "@/app/machine-route";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { useFeedSessions, useFeedSettings } from "@/app/global-feed";
import { SelectField } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { Content, Panel, PanelHeader } from "@/components/panel";
import { useShowHiddenFiles } from "@/features/files/use-files-prefs";
import { AccessSection } from "@/features/settings/access-section";
import { SettingsUpdatesRow } from "@/features/updates/updates-row";
import { AccountsSection } from "@/features/settings/accounts-section";
import { ScheduleSection } from "@/features/settings/schedule-section";
import { askedSection, sectionsFor, type SectionId } from "@/features/settings/sections";
import {
  PREFERENCE_ROW,
  ROW_HINT,
  ROW_LABEL,
  SECTION_TITLE,
  SETTINGS_ACTION,
  SETTINGS_ACTION_PRIMARY,
  SETTINGS_COLUMN,
  SETTINGS_FIELD,
} from "@/features/settings/layout";
import { SetupSection } from "@/features/setup/settings-section";
import { cn } from "@/lib/cn";
import { isCloud } from "@/lib/hub/config";
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

// One row grid for the whole page (features/settings/layout.ts): a fixed
// 16rem label and 14rem control from 36rem up, stacked below it.
const preferenceRow = PREFERENCE_ROW;

/** A boolean setting: the one square switch (`components/ui/switch`), in the settings page's sans label voice. */
function SettingSwitch({
  on,
  label,
  disabled,
  onChange,
}: {
  on: boolean;
  label: string;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return <Switch on={on} blocked={disabled} label={label} onChange={onChange} className="font-sans text-xs font-medium" />;
}

/**
 * This device's `/api/prefs`, and the save that PUTs the whole object back
 * (the route replaces it). Every section that shows a pref reads this one
 * query, so a save in one is seen by the others.
 */
function usePrefs() {
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
  });
  const save = useMutation({
    mutationFn: async (next: Prefs) => {
      const res = await api.PUT("/api/prefs", { body: next });
      return res.data;
    },
    onSuccess: (data) => {
      if (data) queryClient.setQueryData(mq(machine, "prefs"), data);
    },
  });
  return { prefs, save };
}

/** Exported for the render tests (`test/cross-device-render.test.tsx`). */
export function PreferencesSection() {
  const machine = useMachine();
  // Font size and language are applied from the main device's prefs only.
  const home = useHomeMachine();
  const onMain = machine === home;
  const { t } = useTranslation();
  const { prefs, save } = usePrefs();
  // Not a server pref: what the file browser hides is a per-browser habit, and
  // it takes effect in an open file list without a save round-trip.
  const [showHiddenFiles, setShowHiddenFiles] = useShowHiddenFiles();

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className={SECTION_TITLE}>
        {t("preferences")}
      </h2>
      <div className={preferenceRow}>
        <div>
          <div className={ROW_LABEL}>{t("session list grouping")}</div>
          <div className={ROW_HINT}>{t("group sessions by project directory")}</div>
        </div>
        {prefs ? (
          <Tabs
            className="w-full"
            value={prefs.sessionGrouping}
            onValueChange={(v) => save.mutate({ ...prefs, sessionGrouping: v as Prefs["sessionGrouping"] })}
          >
            <TabsList className="w-full">
              <TabsTab value="flat">{t("flat")}</TabsTab>
              <TabsTab value="directory">{t("directory")}</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-xs text-muted">…</span>
        )}
      </div>
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className={ROW_LABEL}>{t("subagent tree")}</div>
          <div className={ROW_HINT}>{t("list the subagents a session delegated to under it")}</div>
        </div>
        {prefs ? (
          // Opt-in: the server only looks for subagents once this is on.
          <SettingSwitch
            on={prefs.subagentTree === "on"}
            disabled={save.isPending}
            label={prefs.subagentTree === "on" ? t("enabled") : t("disabled")}
            onChange={(on) => save.mutate({ ...prefs, subagentTree: on ? "on" : "off" })}
          />
        ) : (
          <span className="font-mono text-xs text-muted">…</span>
        )}
      </div>
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className={ROW_LABEL}>{t("message display")}</div>
          <div className={ROW_HINT}>{t("chat bubbles or a flat list")}</div>
        </div>
        {prefs ? (
          <Tabs
            className="w-full"
            value={prefs.transcriptStyle}
            onValueChange={(v) => save.mutate({ ...prefs, transcriptStyle: v as Prefs["transcriptStyle"] })}
          >
            <TabsList className="w-full">
              <TabsTab value="flat">{t("flat")}</TabsTab>
              <TabsTab value="bubble">{t("bubbles")}</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-xs text-muted">…</span>
        )}
      </div>
      {onMain ? (
        <>
          <div className={cn(preferenceRow, "mt-3")}>
            <div>
              <div className={ROW_LABEL}>{t("font size")}</div>
              <div className={ROW_HINT}>{t("scales the whole UI, synced across devices")}</div>
            </div>
            {prefs ? (
              // Four sizes: a select, not a segmented control.
              <SelectField
                aria-label={t("font size")}
                value={prefs.fontSize}
                options={[
                  { value: "small", label: "sm" },
                  { value: "default", label: "md" },
                  { value: "large", label: "lg" },
                  { value: "xlarge", label: "xl" },
                ]}
                onValueChange={(v) => save.mutate({ ...prefs, fontSize: v as Prefs["fontSize"] })}
              />
            ) : (
              <span className="font-mono text-xs text-muted">…</span>
            )}
          </div>
          <div className={cn(preferenceRow, "mt-3")}>
            <div>
              <div className={ROW_LABEL}>{t("language")}</div>
              <div className={ROW_HINT}>{t("auto follows the browser language")}</div>
            </div>
            {prefs ? (
              // Five languages: a select. An 11px segmented strip could not
              // hold 简体 / 繁體 / 日本語 without shrinking the text.
              <SelectField
                aria-label={t("language")}
                value={prefs.language}
                options={[
                  { value: "auto", label: t("auto") },
                  { value: "en", label: "EN" },
                  { value: "zh-CN", label: "简体" },
                  { value: "zh-TW", label: "繁體" },
                  { value: "ja", label: "日本語" },
                ]}
                onValueChange={(v) => save.mutate({ ...prefs, language: v as Prefs["language"] })}
              />
            ) : (
              <span className="font-mono text-xs text-muted">…</span>
            )}
          </div>
        </>
      ) : (
        // The page's font size and language are the main device's prefs
        // (`PrefsSync`): set here they would be saved on this device and
        // never applied.
        <div data-appearance-on-main="" className={cn(preferenceRow, "mt-3")}>
          <div className={ROW_HINT}>{t("Font size and language follow the main device: change them in its settings.")}</div>
          <MachineLink machine={home} to="/settings" className="shrink-0 text-[13px] font-medium text-fg underline underline-offset-2 hover:text-muted">
            {t("main device settings")}
          </MachineLink>
        </div>
      )}
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className={ROW_LABEL}>{t("composer send shortcut")}</div>
          <div className={ROW_HINT}>
            {prefs?.composerSend === "modifier"
              ? t("⌘/Ctrl+Enter sends · Enter inserts a newline")
              : t("Enter sends · Shift+Enter inserts a newline")}
          </div>
        </div>
        {prefs ? (
          <Tabs
            className="w-full"
            value={prefs.composerSend}
            onValueChange={(v) => save.mutate({ ...prefs, composerSend: v as Prefs["composerSend"] })}
          >
            <TabsList className="w-full">
              <TabsTab value="enter">{t("enter")}</TabsTab>
              <TabsTab value="modifier">⌘+enter</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-xs text-muted">…</span>
        )}
      </div>
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className={ROW_LABEL}>{t("show hidden files")}</div>
          <div className={ROW_HINT}>
            {t("dot-files in the file browser · this device only")}
          </div>
        </div>
        <Tabs
          className="w-full"
          value={showHiddenFiles ? "show" : "hide"}
          onValueChange={(v) => setShowHiddenFiles(v === "show")}
        >
          <TabsList className="w-full">
            <TabsTab value="hide">{t("hide")}</TabsTab>
            <TabsTab value="show">{t("show")}</TabsTab>
          </TabsList>
        </Tabs>
      </div>
    </section>
  );
}

/**
 * How what an agent writes is drawn, one row per feature a reader may want
 * off: kept apart from the preferences above, which are about the app rather
 * than about a message's content. Each row is a server pref, like those.
 *
 * Exported for the render tests (`test/cross-device-render.test.tsx`).
 */
export function MessageRenderingSection() {
  const { t } = useTranslation();
  const { prefs, save } = usePrefs();
  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className={SECTION_TITLE}>
        {t("message rendering")}
      </h2>
      <div className={preferenceRow}>
        <div>
          <div className={ROW_LABEL}>{t("LaTeX math")}</div>
          <div className={ROW_HINT}>{t("$…$ and $$…$$ in messages · code stays as written")}</div>
        </div>
        {prefs ? (
          <Tabs
            className="w-full"
            // A server from before the pref sends none, and typesets.
            value={prefs.math ?? "rendered"}
            onValueChange={(v) => save.mutate({ ...prefs, math: v as Prefs["math"] })}
          >
            <TabsList className="w-full">
              <TabsTab value="rendered">{t("rendered")}</TabsTab>
              <TabsTab value="source">{t("source")}</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-xs text-muted">…</span>
        )}
      </div>
      <div className={cn(preferenceRow, "mt-3")}>
        <div>
          <div className={ROW_LABEL}>{t("Mermaid diagrams")}</div>
          <div className={ROW_HINT}>{t("```mermaid blocks in messages · other code stays as written")}</div>
        </div>
        {prefs ? (
          <Tabs
            className="w-full"
            // A server from before the pref sends none, and draws them.
            value={prefs.diagrams ?? "rendered"}
            onValueChange={(v) => save.mutate({ ...prefs, diagrams: v as Prefs["diagrams"] })}
          >
            <TabsList className="w-full">
              <TabsTab value="rendered">{t("rendered")}</TabsTab>
              <TabsTab value="source">{t("source")}</TabsTab>
            </TabsList>
          </Tabs>
        ) : (
          <span className="font-mono text-xs text-muted">…</span>
        )}
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
      <h2 data-label="" className={SECTION_TITLE}>
        {t("notifications")}
      </h2>
      <div className={preferenceRow}>
        <div className="min-w-0">
          <div className={ROW_LABEL}>{t("push notifications")}</div>
          <div className={ROW_HINT}>
            {t("Ping this browser when a session finishes a turn or asks for input.")}
          </div>
          {state === "denied" && (
            <div data-signal="warn" className="mt-0.5 text-2xs text-warn-fg">
              {t("Blocked for this site — re-allow notifications in the browser's site settings.")}
            </div>
          )}
          {state === "unsupported" && (
            <div className="mt-0.5 text-2xs text-muted">
              {t("Unavailable here: push needs a service worker, which means a production build over https (or localhost).")}
            </div>
          )}
          {error !== "" && <div data-signal="failed" className="mt-0.5 font-mono text-2xs text-danger">{error}</div>}
        </div>
        {state === "loading" ? (
          <span className="font-mono text-xs text-muted">…</span>
        ) : state === "on" || state === "off" ? (
          // A boolean, so a square switch, not a 24px button with a verb.
          <SettingSwitch
            on={state === "on"}
            disabled={busy}
            label={state === "on" ? t("enabled") : t("disabled")}
            onChange={() => void toggle()}
          />
        ) : (
          <span className="shrink-0 font-mono text-xs text-muted">{t("unavailable")}</span>
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
  // A device key and an address: values, so mono.
  const field = cn(SETTINGS_FIELD, "font-mono");

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className={SECTION_TITLE}>
        {t("phone notifications")}
      </h2>
      <div className={cn(SETTINGS_COLUMN, "flex flex-col gap-2")}>
        <p className="text-xs leading-[18px] text-muted">
          {t(
            "Lets a session interrupt you on purpose — when it is blocked on a decision, has found something you would want to stop for, or has finished. Delivered by Bark on iOS; the device key stays on the server and is never given to the agent.",
          )}
        </p>
        <label className="flex flex-col gap-1">
          <span className="text-xs text-muted">
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
          <span className="text-xs text-muted">{t("address your phone opens")}</span>
          <input
            value={baseUrl ?? config?.baseUrl ?? ""}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder={window.location.origin}
            className={field}
          />
          <span className="text-xs text-muted">
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
            data-variant="primary" className={SETTINGS_ACTION_PRIMARY}
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
            className={SETTINGS_ACTION}
          >
            {test.isPending ? t("sending…") : t("send test")}
          </button>
          {armed && (
            <span className="ml-auto">
              <SettingSwitch
                on={config?.enabled ?? false}
                disabled={save.isPending}
                label={config?.enabled ? t("enabled") : t("disabled")}
                onChange={() => {
                  setSent(false);
                  save.mutate({ enabled: !config?.enabled });
                }}
              />
            </span>
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
      <h2 data-label="" className={SECTION_TITLE}>
        {t("install")}
      </h2>
      <div className={preferenceRow}>
        <div className="min-w-0">
          <div className={ROW_LABEL}>{t("home screen app")}</div>
          <div className={ROW_HINT}>
            {t("Opens full-screen with its own icon, and can badge it when a session needs you.")}
          </div>
          {state === "ios-hint" && (
            <div className="mt-0.5 text-xs text-muted">
              {t("On iOS: tap the Share button in Safari, then “Add to Home Screen”.")}
            </div>
          )}
        </div>
        {state === "promptable" ? (
          <button
            type="button"
            onClick={() => void promptInstall()}
            data-variant="primary" className={SETTINGS_ACTION_PRIMARY}
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
  // A base URL, a key and a model id: values, so mono.
  const field = cn(SETTINGS_FIELD, "font-mono");

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className={SECTION_TITLE}>
        {t("instant chat · api provider")}
      </h2>
      <div className={cn(SETTINGS_COLUMN, "flex flex-col gap-2")}>
        <label className="flex flex-col gap-1">
          <span className="text-xs text-muted">{t("base url (openai-compatible)")}</span>
          <input
            value={baseUrl ?? provider?.baseUrl ?? ""}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="https://openrouter.ai/api/v1"
            className={field}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-xs text-muted">
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
          <span className="text-xs text-muted">{t("default model")}</span>
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
          data-variant="primary" className={SETTINGS_ACTION_PRIMARY}
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
      <MessageRenderingSection />
      <NotificationsSection />
      <PhoneNotificationsSection />
      <InstallSection />
      <ChatProviderSection />
      <SettingsUpdatesRow />
      <section className="p-3">
        <h2 data-label="" className={SECTION_TITLE}>
          {t("server info")}
        </h2>
        {isPending && <Spinner />}
        {data && (
          <table className={SETTINGS_COLUMN}>
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
  // A page a hub serves offers no Plugins tab, and an old link to one opens
  // the general tab instead (docs/plugins/runtime.md §5).
  const cloud = isCloud();
  const sections = sectionsFor(cloud);
  const asked = askedSection(search.section, cloud);
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
          {sections.map((id) => (
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
        {/* The page title and every section title start at the same 12px. */}
        <header className="flex min-h-8 shrink-0 flex-col justify-center gap-1 border-b border-border bg-bg px-3 py-1 md:h-8 md:flex-row md:items-center md:justify-between md:py-0">
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
              {/* Phone category strip: 36px, one scrolling row, labels never
                  wrap or shrink (docs/ui-spec.md §4). */}
              <TabsList className="h-9 w-max min-w-full">
                {sections.map((id) => (
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
          ) : !cloud && section === "plugins" ? (
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
