import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SourceIcon } from "@/components/source-icon";
import { Bot, ChevronLeft, Folder, LayoutGrid, MessagesSquare, Plus, Sparkles, SquareTerminal, Terminal, X, Zap } from "lucide-react";
import { Suspense, useCallback, useMemo, useState, type ReactNode } from "react";
import { localize } from "@droppen/plugin-sdk";
import { useTranslation } from "react-i18next";
import { useChatList } from "@/features/chat/chat-layout";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { StatusBadge } from "@/features/sessions/sessions-layout";
import { unreadCountOf } from "@/features/sessions/session-attention";
import { XtermPane } from "@/features/terminals/lazy-xterm-pane";
import { useFeedSessions } from "./global-feed";
import { ApiChatViewInner, FilesBrowser, NewSessionView, SessionDetailInner } from "./lazy-views";
import { paneMachine, usesMachine, withMachine, type PaneContent } from "./pane-tree";
import { MachineView } from "./machine-route";
import { usePaneTreeMixed } from "./pane-tree-view";
import { cloudParts } from "./cloud-parts";
import { basename, formatRelative } from "@/lib/format";
import { hostLocale } from "@/plugins/host";
import { readPaneContent, storePaneContent, type PluginPaneContent } from "@/plugins/pane-content";
import { PluginIcon } from "@/plugins/boundary";
import { PluginPane, usePluginPaneOptions, usePluginPaneTitle } from "@/plugins/plugin-pane";
import { DeviceChip, DevicePicker, readOnlyHere } from "@/features/machines/device-picker";
import { useApi, useHoldMachine, useHomeMachine, useMachine, useOpenMachines } from "@/lib/machines/context";
import { isOpen, shellKind } from "@/lib/machines/registry";
import { sessionKey, type MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";

/** What a picker panel hands back: the content, and the device it is on when that is not the panel's own (a session started on another). */
type Pick = (content: PaneContent, machine?: MachineKey) => void;

function SessionPickerPanel({
  onPick,
  onBack,
  devices,
}: {
  onPick: Pick;
  onBack: () => void;
  devices: ReactNode;
}) {
  const { t: tr } = useTranslation();
  const [q, setQ] = useState("");
  // The panel's device's list (`PanePicker` puts it under that device):
  // from its copy while it is not live, so listing dials nothing.
  const allSessions = useFeedSessions();
  const isPending = allSessions === null;

  const sessions = useMemo(() => {
    // Owned only, like the sidebar. The feed carries both scopes because the
    // detail route needs external rows to exist; a *picker*
    // that offered them would hand back a pane that silently refuses to send,
    // which is the confusion removing the external section was for.
    const all = (allSessions ?? []).filter((s) => s.scope !== "external");
    if (!q.trim()) return all.slice(0, 30);
    const needle = q.toLowerCase();
    return all
      .filter(
        (s) => s.title.toLowerCase().includes(needle) || (s.cwd ?? "").toLowerCase().includes(needle),
      )
      .slice(0, 30);
  }, [allSessions, q]);

  return (
    <div className="my-auto flex min-h-0 max-h-[75%] w-88 max-w-full flex-col border border-border-strong bg-bg">
      <header className="flex h-8 shrink-0 items-center gap-1.5 border-b border-border px-2">
        <button
          type="button"
          onClick={onBack}
          className="flex size-5 items-center justify-center rounded text-muted hover:bg-hover"
        >
          <ChevronLeft className="size-3.5" />
        </button>
        <span className="flex-1 font-mono text-2xs font-semibold tracking-wider text-muted uppercase">
          {tr("pick a session")}
        </span>
        {devices}
        {/* The empty session view, in this pane: it becomes the session it starts. */}
        <button
          type="button"
          onClick={() => onPick({ kind: "new-session" })}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-2xs text-accent hover:bg-hover"
        >
          <Plus className="size-3" /> {tr("new")}
        </button>
      </header>
      <div className="shrink-0 border-b border-border p-1.5">
        <Input placeholder={tr("filter…")} value={q} onChange={(e) => setQ(e.target.value)} />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {isPending && <Spinner />}
        {sessions.map((s) => (
          <button
            key={`${s.source}-${s.id}`}
            type="button"
            onClick={() => onPick({ kind: "session", source: s.source, id: s.id })}
            className="block w-full border-b border-border px-2.5 py-1.5 text-left hover:bg-hover"
          >
            <div className="flex items-center gap-1.5">
              <span className="min-w-0 flex-1 truncate text-xs">{s.title}</span>
              <StatusBadge session={s} unread={unreadCountOf(s)} />
            </div>
            <div className="mt-0.5 flex items-center gap-1.5">
              <SourceIcon source={s.source} className="text-muted" />
              <span className="truncate font-mono text-2xs text-faint">{basename(s.cwd)}</span>
              <span className="ml-auto shrink-0 font-mono text-2xs text-faint">
                {formatRelative(s.updatedAt)}
              </span>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

function TerminalPickerPanel({
  onPick,
  onBack,
  devices,
}: {
  onPick: Pick;
  onBack: () => void;
  devices: ReactNode;
}) {
  const api = useApi();
  const machine = useMachine();
  // Terminals need the device itself: listing them is using it.
  useHoldMachine(machine);
  const { t: tr } = useTranslation();
  const queryClient = useQueryClient();
  const { data, isPending } = useQuery({
    queryKey: mq(machine, "terminals"),
    queryFn: async () => (await api.GET("/api/terminals")).data,
  });

  const create = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/terminals", { body: {} });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (t) => {
      void queryClient.invalidateQueries({ queryKey: mq(machine, "terminals") });
      onPick({ kind: "terminal", id: t.id });
    },
  });

  return (
    <div className="my-auto flex min-h-0 max-h-[75%] w-88 max-w-full flex-col border border-border-strong bg-bg">
      <header className="flex h-8 shrink-0 items-center gap-1.5 border-b border-border px-2">
        <button
          type="button"
          onClick={onBack}
          className="flex size-5 items-center justify-center rounded text-muted hover:bg-hover"
        >
          <ChevronLeft className="size-3.5" />
        </button>
        <span className="flex-1 font-mono text-2xs font-semibold tracking-wider text-muted uppercase">
          {tr("pick a terminal")}
        </span>
        {devices}
        <button
          type="button"
          onClick={() => create.mutate()}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-2xs text-accent hover:bg-hover"
        >
          <Plus className="size-3" /> {tr("new")}
        </button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {isPending && <Spinner />}
        {data?.terminals.map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => onPick({ kind: "terminal", id: t.id })}
            className="flex w-full items-center gap-1.5 border-b border-border px-2.5 py-1.5 text-left hover:bg-hover"
          >
            <SquareTerminal className="size-3.5 shrink-0 text-ok/70" />
            <span className="min-w-0 flex-1 truncate text-xs">{t.title}</span>
            <span className="shrink-0 font-mono text-2xs text-faint">{formatRelative(t.createdAt)}</span>
          </button>
        ))}
        {!isPending && (data?.terminals.length ?? 0) === 0 && (
          <div className="p-3 text-center font-mono text-2xs text-faint">no terminals yet — create one</div>
        )}
      </div>
    </div>
  );
}

function ChatPickerPanel({
  onPick,
  onBack,
  devices,
}: {
  onPick: Pick;
  onBack: () => void;
  devices: ReactNode;
}) {
  const { t: tr } = useTranslation();
  // Chats are the device's own (not in its copy): listing them is using it.
  useHoldMachine(useMachine());
  const { data: chats, isPending } = useChatList();

  return (
    <div className="my-auto flex min-h-0 max-h-[75%] w-88 max-w-full flex-col border border-border-strong bg-bg">
      <header className="flex h-8 shrink-0 items-center gap-1.5 border-b border-border px-2">
        <button
          type="button"
          onClick={onBack}
          className="flex size-5 items-center justify-center rounded text-muted hover:bg-hover"
        >
          <ChevronLeft className="size-3.5" />
        </button>
        <span className="flex-1 font-mono text-2xs font-semibold tracking-wider text-muted uppercase">
          {tr("pick a chat")}
        </span>
        {devices}
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {isPending && <Spinner />}
        {chats?.map((chat) => (
          <button
            key={`${chat.kind}-${chat.id}`}
            type="button"
            onClick={() => onPick({ kind: "chat", chatKind: chat.kind, id: chat.id })}
            className="block w-full border-b border-border px-2.5 py-1.5 text-left hover:bg-hover"
          >
            <div className="flex items-center gap-1.5">
              {chat.kind === "claude" ? (
                <Sparkles className="size-3 shrink-0 text-accent" />
              ) : chat.kind === "codex" ? (
                <Terminal className="size-3 shrink-0 text-muted" />
              ) : (
                <Bot className="size-3 shrink-0 text-muted" />
              )}
              <span className="min-w-0 flex-1 truncate text-xs">{chat.title}</span>
              <span className="ml-auto shrink-0 font-mono text-2xs text-faint">
                {formatRelative(chat.updatedAt)}
              </span>
            </div>
          </button>
        ))}
        {chats && chats.length === 0 && (
          <div className="p-3 font-mono text-2xs text-faint">{tr("no chats yet — start one →")}</div>
        )}
      </div>
    </div>
  );
}

/**
 * What a pane can show, and — on a page with more than one device — which
 * device it comes from (`machine`; `onMachine` moves the picker to another).
 * The lists are that device's (`MachineView`); a plugin's pane is the
 * home's, wherever the picker is.
 */
function PanePicker({
  machine,
  onMachine,
  onPick,
}: {
  machine: MachineKey;
  onMachine: (machine: MachineKey) => void;
  onPick: Pick;
}) {
  const { t: tr } = useTranslation();
  const [level, setLevel] = useState<null | "session" | "chat" | "terminal">(null);
  const pluginPanes = usePluginPaneOptions();
  const locale = hostLocale();
  const several = useOpenMachines().length > 1;
  const AddDeviceHint = cloudParts()?.AddDeviceHint;
  // Only devices something can be kept on: a pane never names one shown read-only.
  const devices = <DevicePicker value={machine} onSelect={onMachine} writable />;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col items-center overflow-y-auto p-4">
      {level === null && (
        <div className="my-auto flex w-56 max-w-full shrink-0 flex-col gap-2">
          <div className="mb-1 text-center font-mono text-2xs text-faint">{tr("show in this pane")}</div>
          {several && (
            <div className="mb-1 flex min-w-0 items-center justify-center gap-1.5">
              <span className="shrink-0 font-mono text-2xs text-faint">{tr("device")}</span>
              {devices}
            </div>
          )}
          {pluginPanes.map(({ plugin, pane }) => {
            return (
              <button
                key={`${plugin.id}/${pane.id}`}
                type="button"
                data-plugin={plugin.id}
                onClick={() => onPick(storePaneContent({ kind: "plugin", plugin: plugin.id, pane: pane.id }) as PaneContent)}
                className="flex items-center gap-2.5 border border-border bg-bg px-3 py-2.5 text-left hover:border-fg hover:bg-hover"
              >
                <PluginIcon host={plugin.host} icon={pane.icon} className="size-4 shrink-0 text-muted" />
                <span className="min-w-0">
                  <span className="block text-xs font-medium">{localize(pane.label, locale)}</span>
                  {pane.description && (
                    <span className="block truncate font-mono text-2xs text-faint">{localize(pane.description, locale)}</span>
                  )}
                </span>
              </button>
            );
          })}
          <button
            type="button"
            onClick={() => onPick({ kind: "files" })}
            className="flex items-center gap-2.5 border border-border bg-bg px-3 py-2.5 text-left hover:border-fg hover:bg-hover"
          >
            <Folder className="size-4 shrink-0 text-muted" />
            <span><span className="block text-xs font-medium">{tr("Files")}</span><span className="block font-mono text-2xs text-faint">{tr("browse, preview and transfer files")}</span></span>
          </button>
          <button
            type="button"
            onClick={() => setLevel("session")}
            className="flex items-center gap-2.5 border border-border bg-bg px-3 py-2.5 text-left hover:border-fg hover:bg-hover"
          >
            <MessagesSquare className="size-4 shrink-0 text-muted" />
            <span>
              <span className="block text-xs font-medium">{tr("Session")}</span>
              <span className="block font-mono text-2xs text-faint">{tr("claude / codex")}</span>
            </span>
          </button>
          <button
            type="button"
            onClick={() => setLevel("chat")}
            className="flex items-center gap-2.5 border border-border bg-bg px-3 py-2.5 text-left hover:border-fg hover:bg-hover"
          >
            <Zap className="size-4 shrink-0 text-warn-fg/80" />
            <span>
              <span className="block text-xs font-medium">{tr("Instant Chat")}</span>
              <span className="block truncate font-mono text-2xs text-faint">{tr("daily chat")}</span>
            </span>
          </button>
          <button
            type="button"
            onClick={() => setLevel("terminal")}
            className="flex items-center gap-2.5 border border-border bg-bg px-3 py-2.5 text-left hover:border-fg hover:bg-hover"
          >
            <SquareTerminal className="size-4 shrink-0 text-ok/80" />
            <span>
              <span className="block text-xs font-medium">{tr("Terminal")}</span>
              <span className="block font-mono text-2xs text-faint">{tr("shell")}</span>
            </span>
          </button>
          {/* droppen.app with one device that runs AI: a second one could be in a pane too (until put away). */}
          {!several && AddDeviceHint && <AddDeviceHint variant="line" className="mt-1" />}
        </div>
      )}
      {level !== null && (
        <MachineView machine={machine}>
          {level === "session" && <SessionPickerPanel onPick={onPick} onBack={() => setLevel(null)} devices={devices} />}
          {level === "chat" && <ChatPickerPanel onPick={onPick} onBack={() => setLevel(null)} devices={devices} />}
          {level === "terminal" && <TerminalPickerPanel onPick={onPick} onBack={() => setLevel(null)} devices={devices} />}
        </MachineView>
      )}
    </div>
  );
}

/** A leaf's dangling reference: the underlying resource is gone; offer a reset. */
function MissingContent({ note, onReset }: { note: string; onReset: () => void }) {
  const { t: tr } = useTranslation();
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2">
      <span className="font-mono text-2xs text-muted">{note}</span>
      <button
        type="button"
        onClick={onReset}
        className="flex items-center gap-1 rounded border border-border px-2 py-1 font-mono text-2xs text-accent hover:bg-hover"
      >
        {tr("pick new content")}
      </button>
    </div>
  );
}

/**
 * Renders one pane leaf: header, picker, and the content views. Shared by the
 * ephemeral route splits and persistent screens — the caller wires how content
 * changes and how the pane closes.
 */
/** A plugin pane's header title. A hook, so it lives in its own component. */
function PluginPaneTitle({ content }: { content: PluginPaneContent }) {
  return <>{usePluginPaneTitle(content)}</>;
}

/**
 * One leaf's content on its device (`docs/local-aggregator.md` §6.5): the
 * device its content names, else the home's — or, for a picker that names
 * none, `defaultMachine` (the routed view's device, beside it). Everything
 * inside is about that device (`MachineView`), which is held while the
 * content uses it; what the leaf stores names the device only when it is
 * not the home (`withMachine`).
 */
export function PaneLeafContent({
  content: stored,
  onChange,
  onClose,
  defaultMachine,
}: {
  content: PaneContent;
  onChange: (content: PaneContent) => void;
  onClose: () => void;
  /** The device a picker naming none picks from (default: the home). */
  defaultMachine?: MachineKey;
}) {
  const home = useHomeMachine();
  const mixed = usePaneTreeMixed();
  // Re-rendered as devices come and go: a pane's device may leave the page.
  useOpenMachines();
  // Layouts from before plugins say `{kind: "tasks"}`; that is a plugin pane now.
  const content = readPaneContent(stored) as PaneContent;
  // A picker picks from a device the page has: the one it names, else the
  // routed view's, else the home — never one gone from the page.
  // Never one shown read-only (another device through the local app's
  // gateway): what a pane picks is kept, and must not name it.
  const pickable = (m: MachineKey | undefined | null): m is MachineKey => !!m && isOpen(m) && !readOnlyHere(m);
  const fallback = pickable(defaultMachine) ? defaultMachine : home;
  const machine =
    content.kind === "picker" ? (pickable(content.machine) ? content.machine : fallback) : paneMachine(content, home);
  const setContent = useCallback(
    (next: PaneContent, on: MachineKey = machine) => onChange(withMachine(next, on, home)),
    [onChange, machine, home],
  );
  const showDevice = usesMachine(content) && (mixed || machine !== home);
  if (content.kind !== "picker" && !isOpen(machine)) {
    // A device this page does not have: a screen saved on droppen.app read by
    // the device's own local page, or a device that left the account. Nothing
    // here may ask it anything (it has no runtime here).
    return (
      <MissingDevicePane
        machine={machine}
        content={content}
        onSwitch={() => onChange({ kind: "picker" })}
        onClose={onClose}
      />
    );
  }
  if (content.kind === "picker") {
    return (
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-panel px-2">
          <LayoutGrid className="size-3.5 shrink-0 text-faint" />
          <span className="min-w-0 flex-1 truncate font-mono text-2xs text-muted">pick content for this pane</span>
          <PaneActions content={content} onSwitch={() => undefined} onClose={onClose} />
        </header>
        <PanePicker
          machine={machine}
          // A picker naming no device picks from `defaultMachine`, so one moved to any other — the home too — names it.
          onMachine={(next) => onChange(next === fallback ? { kind: "picker" } : { kind: "picker", machine: next })}
          onPick={(picked, on) => setContent(picked, on ?? machine)}
        />
      </div>
    );
  }
  return (
    <MachineView machine={machine} hold={usesMachine(content)}>
      <PaneLeafBody
        content={content}
        setContent={setContent}
        onClose={onClose}
        device={showDevice ? <DeviceChip machine={machine} /> : null}
      />
    </MachineView>
  );
}

/**
 * A pane whose content is on a device this page does not have. On the local
 * page that is another device of the account (the screen was arranged on
 * droppen.app); on droppen.app, a device that is no longer in the account.
 */
function MissingDevicePane({
  machine,
  content,
  onSwitch,
  onClose,
}: {
  machine: MachineKey;
  content: PaneContent;
  onSwitch: () => void;
  onClose: () => void;
}) {
  const { t: tr } = useTranslation();
  // The local app (alone, or showing other devices through its gateway): a screen arranged on droppen.app.
  const local = shellKind() !== "hub";
  return (
    <div data-pane-missing-device={machine} className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-panel px-2">
        <LayoutGrid className="size-3.5 shrink-0 text-faint" />
        <span className="min-w-0 flex-1 truncate font-mono text-2xs text-muted">{content.kind}</span>
        <PaneActions content={content} device={<DeviceChip machine={machine} />} onSwitch={onSwitch} onClose={onClose} />
      </header>
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-1.5 p-4 text-center">
        <span className="font-mono text-xs text-muted">{tr("this device is not on this page")}</span>
        <span className="max-w-sm font-mono text-2xs text-faint">
          {local
            ? tr("This pane is on another of your devices. Open this screen on droppen.app to see it.")
            : tr("It may have been removed from the account, or the link is from another account.")}
        </span>
      </div>
    </div>
  );
}

/** A pane's own controls: switch what it shows, close it. */
function PaneActions({
  content,
  device = null,
  onSwitch,
  onClose,
}: {
  content: PaneContent;
  device?: ReactNode;
  onSwitch: () => void;
  onClose: () => void;
}) {
  const { t: tr } = useTranslation();
  return (
    <>
      {device}
      {content.kind !== "picker" && (
        <button
          type="button"
          title={tr("change content")}
          aria-label={tr("change content")}
          onClick={onSwitch}
          className="shrink-0 rounded px-1.5 py-0.5 font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
        >
          switch
        </button>
      )}
      <button
        type="button"
        title={tr("close pane")}
        aria-label={tr("close pane")}
        onClick={onClose}
        className="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
      >
        <X className="size-3.5" />
      </button>
    </>
  );
}

function PaneLeafBody({
  content,
  setContent,
  onClose,
  device,
}: {
  content: PaneContent;
  /** `machine`: the device the new content is on, when not this pane's (a new session moved to another). */
  setContent: (content: PaneContent, machine?: MachineKey) => void;
  onClose: () => void;
  /** The device's name in the header, when the pane says it. */
  device: ReactNode;
}) {
  const api = useApi();
  const machine = useMachine();
  const { t: tr } = useTranslation();

  const { data: terminals } = useQuery({
    queryKey: mq(machine, "terminals"),
    queryFn: async () => (await api.GET("/api/terminals")).data,
    enabled: content.kind === "terminal",
  });
  const terminalTitle =
    content.kind === "terminal"
      ? (terminals?.terminals.find((t) => t.id === content.id)?.title ?? "terminal")
      : null;
  const terminalGone =
    content.kind === "terminal" && !!terminals && !terminals.terminals.some((t) => t.id === content.id);
  const sessionHeader =
    content.kind === "session" || content.kind === "new-session" || (content.kind === "chat" && content.chatKind !== "api");
  const paneActions = <PaneActions content={content} device={device} onSwitch={() => setContent({ kind: "picker" })} onClose={onClose} />;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      {!sessionHeader && content.kind !== "files" && <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-panel px-2">
        <LayoutGrid className="size-3.5 shrink-0 text-faint" />
        <span className="min-w-0 flex-1 truncate font-mono text-2xs text-muted">
          {content.kind === "plugin" && <PluginPaneTitle content={content} />}
          {content.kind === "chat" && `chat · ${content.id.slice(0, 8)}`}
          {content.kind === "terminal" && `terminal · ${terminalTitle}`}
        </span>
        {paneActions}
      </header>}
      {content.kind === "plugin" && (
        <PluginPane
          key={`${content.plugin}/${content.pane}`}
          content={content}
          onState={(state) => setContent(storePaneContent({ ...content, state }) as PaneContent)}
          onReset={() => setContent({ kind: "picker" })}
        />
      )}
      {/* Files, sessions and chats are loaded the first time one is shown
          (`lazy-views.ts`); a chunk that fails is the pane's error boundary's. */}
      <Suspense fallback={null}>
        {content.kind === "files" && <FilesBrowser location={content} onNavigate={(next) => setContent({ kind: "files", ...next })} actions={paneActions} />}
        {content.kind === "session" && (
          <SessionDetailInner
            key={sessionKey({ machine, source: content.source, id: content.id })}
            source={content.source}
            id={content.id}
            paneActions={paneActions}
            onArchived={() => setContent({ kind: "picker" })}
            onOpenSession={(source, id) => setContent({ kind: "session", source, id })}
          />
        )}
        {content.kind === "new-session" && (
          <NewSessionView
            key={`new-session/${machine}`}
            paneActions={paneActions}
            onCreated={(source, id) => setContent({ kind: "session", source, id })}
            onDevice={(next) => setContent({ kind: "new-session" }, next)}
          />
        )}
        {content.kind === "chat" &&
          (content.chatKind === "api" ? (
            <ApiChatViewInner key={content.id} id={content.id} />
          ) : (
            <SessionDetailInner
              key={`${content.chatKind}/${content.id}`}
              source={content.chatKind}
              id={content.id}
              chat
              paneActions={paneActions}
            />
          ))}
      </Suspense>
      {content.kind === "terminal" &&
        (terminalGone ? (
          <MissingContent note={tr("this terminal has exited")} onReset={() => setContent({ kind: "picker" })} />
        ) : (
          <div className="min-h-0 flex-1">
            <XtermPane id={content.id} visible />
          </div>
        ))}
    </div>
  );
}
