/*
 * `scripts/ui-audit` renders this: real components in their real composition,
 * with no API, router or machine context. It is a fixture, not a page; keep it
 * in step with `features/settings/`, `features/cloud/` and `components/ui/`.
 *
 * `?fixture=settings` (default), `?fixture=devices`, `?fixture=agents`
 * (`&view=cards|details|empty`), `?fixture=new-chat` (`&state=claude|api|
 * signed-out|list`, the instant-chat home in the real chat layout),
 * `?fixture=diagrams` (a transcript of the diagram types messages draw),
 * `?fixture=hostile` (the reviewer's worst-case fences beside one normal
 * diagram), `?fixture=images` (the pictures messages draw; `&cloud=1` as a
 * page a hub serves), `?fixture=cloud-bar` (the cloud bar in each state of
 * its connection; `&states=catch-up`, the offline copy catching up from the
 * hub), `?fixture=session-opening` (a session opening with nothing to show
 * yet, in each stage of its wait), `?fixture=no-copy` (the foot of a session
 * with no copy on this device yet, read-only) or `?fixture=session-from-copy`
 * (the real session view on droppen.app, drawn from the device's copy of the
 * session while the machine is asked for it; `&state=live`, once it has
 * answered; `&state=offline`, the page on the copy while the machine is
 * away), plus `?theme=dark`.
 * `web/src/index.css` excludes this directory from Tailwind's scan
 * (`@source not`), so nothing here can change the production stylesheet. That
 * also means every class below must already occur in `web/src`; the layout
 * strings are imported from the page that owns them.
 */
import "@/index.css";
import { createRoot } from "react-dom/client";
import { GlobalFeedProvider } from "@/app/global-feed";
import { ChatHome } from "@/features/chat/chat-home";
import { ChatLayout } from "@/features/chat/chat-layout";
import { MachineProvider } from "@/lib/machines/context";
import { LOCAL } from "@/lib/machines/key";
import { openMachine, setShell } from "@/lib/machines/registry";
import { constantStore } from "@/lib/machines/store";
import { offlineTransport, type OfflineOptions, type OfflineSource } from "@/lib/transport/offline";
import { SwitchingTransport } from "@/lib/transport/switching";
import type { Transport } from "@/lib/transport/types";
import i18next from "i18next";
import { requestImage } from "@/components/image-lightbox";
import { DiagramRendering, Markdown } from "@/components/markdown";
import { MermaidBlock } from "@/components/mermaid-block";
import { Button } from "@/components/ui/button";
import { CHECKBOX_INPUT, CHECKBOX_LABEL, CheckboxMark } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { SelectField } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { BAR_STRIP, BarCatchUp, BarCatchUpLine, type BarLabel, ConnectionLabel, MachineSwitcher, OfflineNotice, PathIndicator, RefusalNotice, SignOutButton } from "@/features/cloud/cloud-bar";
import type { CatchUpSource } from "@/features/cloud/context";
import type { HubShell } from "@/features/cloud/hub-shell";
import { BarMachineName } from "@/features/cloud/machine-name";
import { AccountButton, DevicesButton } from "@/features/cloud/my-devices";
import { DeviceDot } from "@/features/machines/device-picker";
import type { ChannelRefusal, ConnectStep } from "@/lib/hub/machine";
import { NO_PROGRESS, type StreamProgress } from "@/lib/hub/streams";
import { setCurrentShell } from "@/lib/machines/registry";
import type { AppShell, MachineInfo } from "@/lib/machines/runtime";
import { DeviceRow, type DeviceRowView } from "@/features/cloud/device-row";
import { SectionHead } from "@/features/cloud/ui";
import type { AcpAgentInfo } from "@/features/sessions/acp-agent";
import type { FeedOpening } from "@/features/sessions/session-feed-pool";
import { SessionOpening } from "@/features/sessions/session-opening";
import { SessionDetail } from "@/features/sessions/session-detail";
import type { FeedCopy } from "@/lib/hub/streams";
import type { AcpAgentPreset } from "@/features/sessions/acp-agent-presets";
import { ComposerBox, type ComposerAttachments } from "@/features/sessions/composer-box";
import { useTranslation } from "react-i18next";
import { AcpDetailsBody, AcpTabView } from "@/features/settings/agents/acp-tab";
import { AgentsTabStrip } from "@/features/settings/agents/agents-section";
import { acpStatus, acpSignal, type AcpCheck } from "@/features/settings/agents/agents-tabs";
import { PREFERENCE_ROW, ROW_HINT, ROW_LABEL, SECTION_TITLE } from "@/features/settings/layout";
import { applyLanguage, type LanguagePref } from "@/i18n";

const query = new URLSearchParams(location.search);
document.documentElement.classList.toggle("dark", query.get("theme") === "dark");
const fixture = query.get("fixture") ?? "settings";

function Row({ id, label, hint, children }: { id: string; label: string; hint: string; children: React.ReactNode }) {
  return (
    <div data-audit-row={id} className={`${PREFERENCE_ROW} mt-3`}>
      <div>
        <div className={ROW_LABEL}>{label}</div>
        <div className={ROW_HINT}>{hint}</div>
      </div>
      <div data-audit-control className="w-full">
        {children}
      </div>
    </div>
  );
}

const models = [
  { value: "claude", label: "Claude" },
  { value: "claude-opus", label: "Claude Opus 5.5" },
  { value: "claude-sonnet", label: "Claude Sonnet 5.5" },
  { value: "claude-haiku", label: "Claude Haiku 5.5" },
  { value: "codex", label: "Codex" },
  { value: "deepseek", label: "DeepSeek v4.1 Flash" },
];
const efforts = [
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra high" },
];

function SettingsFixture() {
  return (
    <div className="@container/settings mx-auto max-w-3xl p-4">
      <header className="mb-3 flex h-8 items-center border-b border-border px-4 text-[13px] font-semibold">Settings</header>
      <section className="border-b border-border p-3">
        <h2 data-label="" className={SECTION_TITLE}>
          Preferences
        </h2>
        <Row id="grouping" label="Session list grouping" hint="Group sessions by project directory">
          <Tabs value="flat" onValueChange={() => undefined}>
            <TabsList className="w-full">
              <TabsTab value="flat">Flat</TabsTab>
              <TabsTab value="directory">Directory</TabsTab>
            </TabsList>
          </Tabs>
        </Row>
        <Row id="language" label="Language" hint="Auto follows the browser language">
          <SelectField
            value="en"
            options={[
              { value: "auto", label: "Auto" },
              { value: "en", label: "EN" },
              { value: "zh-CN", label: "简体" },
              { value: "zh-TW", label: "繁體" },
              { value: "ja", label: "日本語" },
            ]}
            onValueChange={() => undefined}
          />
        </Row>
        <Row id="model" label="Model" hint="Which model the agent uses">
          <SelectField value="claude-opus" options={models} onValueChange={() => undefined} />
        </Row>
        <Row id="effort" label="Effort" hint="How long the agent may think">
          <SelectField value="high" options={efforts} onValueChange={() => undefined} />
        </Row>
        <Row id="empty" label="Account" hint="No account is connected yet">
          <SelectField value="" options={[]} onValueChange={() => undefined} />
        </Row>
        <Row id="name" label="Display name" hint="Shown to the other devices">
          <Input defaultValue="kilerd's Mac mini" />
        </Row>
        <Row id="switch" label="Push notifications" hint="Ping this device when a turn finishes">
          <Switch on label="Push notifications" labelHidden onChange={() => undefined} />
        </Row>
        <Row id="checkbox" label="Remove the device" hint="It loses access at once">
          <label className={CHECKBOX_LABEL}>
            <input type="checkbox" className={CHECKBOX_INPUT} defaultChecked />
            <CheckboxMark />
            <span className="min-w-0">also rotate the content keys</span>
          </label>
        </Row>
        <Row id="actions" label="Connection" hint="Check the link to the hub">
          <div className="flex gap-1.5">
            <Button variant="secondary">Test</Button>
            <Button variant="primary">Save</Button>
          </div>
        </Row>
      </section>
    </div>
  );
}

const noop = () => undefined;
const device = (over: Partial<DeviceRowView>): DeviceRowView => ({
  id: "d-1",
  fingerprint: "ABCD-EFGH-IJKL-MNOP",
  name: null,
  self: false,
  kind: "browserOrPhone",
  status: "active",
  host: false,
  stoppedHost: false,
  presence: null,
  path: null,
  copyAsOf: null,
  lastSeenMs: null,
  limited: null,
  accept: null,
  ...over,
});
// Two cards carry two badges each: this device and a host, and a host that is the main device.
const devicesFixture: DeviceRowView[] = [
  device({ id: "m-1", name: "kilerd's Mac mini", self: true, kind: "computer", host: true, presence: "online", path: { relay: "sin-1", rttMs: 32 } }),
  device({ id: "m-2", name: "MacBook Pro", kind: "computer", host: true, main: true, presence: "online", path: { relay: "sin-2", rttMs: 74 } }),
  device({ id: "m-3", name: "macbook-air", kind: "computer", host: true, presence: "offline", copyAsOf: Date.now() - 3_600_000 }),
  device({ id: "d-1", name: "iPhone · Safari", presence: "online" }),
  device({ id: "d-2", status: "pending" }),
];

function DevicesFixture() {
  return (
    <div data-fixture-root="" className="@container/devices mx-auto max-w-[1200px] p-4">
      <div className="px-4">
        <SectionHead index="03" title="Devices">
          <span className="font-mono text-xs text-faint tabular-nums">{devicesFixture.length}</span>
        </SectionHead>
      </div>
      <ul
        data-testid="device-list"
        className="mt-4 grid grid-cols-1 gap-2.5 px-4 @min-[44rem]/devices:grid-cols-[repeat(auto-fill,minmax(20rem,1fr))] @min-[44rem]/devices:gap-3"
      >
        {devicesFixture.map((view) => (
          <DeviceRow key={view.id} view={view} viewerAdmin upgraded writable onOpen={noop} onRemove={noop} onHost={noop} onControl={noop} onRename={noop} onTerminal={noop} onResend={noop} onOpenSession={noop} onReEnroll={noop} onDismissReEnroll={noop} onWarnAgain={noop} onRenew={noop} onRenewCode={noop} onEndPause={noop} onRenewRetry={noop} onAddComputer={noop}>
            {view.id === "m-1" ? <div className="border-t border-border px-4 py-3 text-xs text-muted">The renewal panel lives here; it makes this card taller than its neighbours.</div> : null}
          </DeviceRow>
        ))}
      </ul>
    </div>
  );
}

const minutesAgo = (n: number) => new Date(Date.now() - n * 60_000).toISOString();
const readyCheck: AcpCheck = {
  checkedAt: minutesAgo(2),
  state: "ready",
  resolvedPath: "/home/demo/.local/bin/bub",
  protocolVersion: 1,
  agentInfo: { name: "bub-acp-server", version: "0.1.0" },
  capabilities: { loadSession: true, image: true, audio: false, embeddedContext: true, mcpHttp: false, mcpSse: false },
};
const failedCheck: AcpCheck = {
  checkedAt: minutesAgo(5),
  state: "not_found",
  error: "lab-agent: no such executable\nPATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/home/demo/.cargo/bin",
};
const agentsFixture: AcpAgentInfo[] = [
  { id: "bub", name: "Bub", command: "bub", args: ["acp"], envNames: ["OPENROUTER_API_KEY"], lastCheck: readyCheck, homepage: "https://bub.build/" },
  { id: "lab-agent", name: "Lab agent", command: "lab-agent", args: ["--acp", "--stdio"], envNames: ["LAB_API_TOKEN"], lastCheck: failedCheck },
  { id: "opencode", name: "OpenCode", command: "opencode", args: ["acp"], envNames: [] },
];
const presetsFixture: AcpAgentPreset[] = [
  { id: "bub", name: "Bub", command: "bub", args: ["acp"], onPath: false, note: "Needs Bub's ACP server: bub install bub-acp-server@main" },
  { id: "opencode", name: "OpenCode", command: "opencode", args: ["acp"], onPath: true },
];
const acpActions = { onOpen: noop, onCheck: noop, onEdit: noop, onRemove: noop };

/** Settings › Agents & accounts on its ACP tab: the strip, the grid, the details, the empty state. */
function AgentsFixture() {
  const view = query.get("view") ?? "cards";
  const agents = view === "empty" ? [] : agentsFixture;
  return (
    <div data-fixture-root="" className="@container/settings mx-auto max-w-[1000px] md:border-x md:border-border">
      <AgentsTabStrip value="acp" onChange={noop} signals={{ claude: { warn: 2, danger: 0 }, codex: { warn: 0, danger: 0 }, acp: acpSignal(agents) }} />
      <AcpTabView agents={agents} presets={presetsFixture} statusOf={(agent) => acpStatus(agent.lastCheck)} actions={acpActions} onAdd={noop} />
      {view === "details" && (
        <Dialog open>
          <DialogContent title="Lab agent" className="w-[min(32rem,calc(100vw-2rem))]">
            <AcpDetailsBody
              agent={agentsFixture[1]!}
              status={acpStatus(failedCheck)}
              sessions={2}
              models={null}
              confirming={false}
              busy={false}
              error={null}
              errorWithheld={false}
              onCheck={noop}
              onEdit={noop}
              onAskRemove={noop}
              onConfirmRemove={noop}
              onCancelRemove={noop}
            />
          </DialogContent>
        </Dialog>
      )}
    </div>
  );
}

/**
 * A transcript of the diagram types the message renderer must draw, plus one
 * fence that cannot be drawn. The blocks are the real `<Markdown>` component
 * under the same `DiagramRendering` provider a session view supplies, so the
 * fixture exercises the engine, the picture and the code fallback.
 */
const DIAGRAMS: ReadonlyArray<readonly [name: string, source: string]> = [
  [
    "flowchart (subgraph and labels)",
    `flowchart TD
  A[Client] --> B{Access token?}
  B -->|yes| C[Hub]
  B -->|no| D[Login]
  subgraph Cloud [Cloud]
    C --> E[(Database)]
  end`,
  ],
  [
    "sequence",
    `sequenceDiagram
  participant C as Client
  participant H as Hub
  C->>H: GET /v1/sessions
  H-->>C: 200 JSON`,
  ],
  [
    "class",
    `classDiagram
  class Agent {
    +run(task) string
  }
  class Session {
    +id string
  }
  Agent --> Session : owns`,
  ],
  [
    "state",
    `stateDiagram-v2
  [*] --> Idle
  Idle --> Working: start
  Working --> Idle: done`,
  ],
  [
    "er",
    `erDiagram
  AGENT ||--o{ SESSION : runs
  AGENT {
    string id
    string name
  }
  SESSION {
    string id
    string title
  }`,
  ],
  [
    "gantt",
    `gantt
  title Release
  dateFormat YYYY-MM-DD
  section Build
  Compile :a1, 2026-01-01, 3d
  Test :a2, after a1, 2d`,
  ],
  [
    "pie",
    `pie title Messages
  "Claude" : 60
  "Codex" : 30
  "Other" : 10`,
  ],
  [
    "mindmap",
    `mindmap
  root((Droppen))
    Sessions
      Transcript
    Devices
      Remote`,
  ],
  [
    "timeline",
    `timeline
  title Releases
  2025 : v0.1
  2026 : v0.2 : v0.3`,
  ],
  [
    "gitGraph",
    `gitGraph
  commit id: "init"
  branch feat
  checkout feat
  commit id: "diagram"
  checkout main
  merge feat`,
  ],
  ["invalid fence (the code block)", "flowchart TD\n  A--"],
];

function DiagramsFixture() {
  return (
    <div data-fixture-root="" className="@container/settings mx-auto max-w-3xl p-4">
      <header className="mb-3 flex h-8 items-center border-b border-border text-[13px] font-semibold">Transcript</header>
      <DiagramRendering.Provider value>
        {DIAGRAMS.map(([name, source]) => (
          <section key={name} data-audit-row={name} className="border-b border-border py-3">
            <div className="mb-1 font-mono text-2xs text-muted">{name}</div>
            <Markdown text={"```mermaid\n" + source + "\n```"} />
          </section>
        ))}
      </DiagramRendering.Provider>
    </div>
  );
}

/**
 * The pictures a transcript message draws, through the real `<Markdown>`: one
 * from the web the reader has not asked for (its card, which this page never
 * fetches), the same kind once asked for (a picture this server serves, asked
 * for before the first paint), one inside a link, and a src nothing can draw
 * (the failure card the first one is drawn like).
 */
const LOADED_IMAGE = `${location.origin}/og.png`;
const IMAGES: ReadonlyArray<readonly [name: string, text: string]> = [
  ["web image, not asked for", "The quarterly chart: ![quarterly chart](https://images.example.com/chart.png?q=report)"],
  ["web image, after load image", `The cover: ![Droppen cover](${LOADED_IMAGE})`],
  ["web image inside a link", "[![build](https://img.shields.io/badge/build-passing-111111)](https://github.com/getdroppen/droppen) is green."],
  ["src nothing can draw", "See ![architecture](docs/arch.png) in the repository."],
];

function ImagesFixture() {
  return (
    <div data-fixture-root="" className="@container/settings mx-auto max-w-3xl p-4">
      <header className="mb-3 flex h-8 items-center border-b border-border text-[13px] font-semibold">Transcript</header>
      {IMAGES.map(([name, text]) => (
        <section key={name} data-audit-row={name} className="border-b border-border py-3">
          <div className="mb-1 font-mono text-2xs text-muted">{name}</div>
          <Markdown text={text} />
        </section>
      ))}
    </div>
  );
}

/**
 * The fences from the security review's reproduction (`/tmp/pele-eval/rv/`),
 * plus one normal diagram. They are rendered through the real `MermaidBlock`
 * (the transcript's diagram block) rather than through Markdown, so a
 * long-task measurement sees the diagram engine's own work and not the
 * markdown parser's. The normal diagram is first in the queue.
 */
const HOSTILE: ReadonlyArray<readonly [name: string, source: string]> = (() => {
  const range = (n: number, make: (i: number) => string, sep = "\n") => Array.from({ length: n }, (_, i) => make(i)).join(sep);
  const tasks = (n: number) => range(n, (i) => `T${i} :2024-01-01, 1d`);
  return [
    ["normal diagram (renders)", "flowchart TD\n  A[Start] --> B[End]"],
    ["gantt dateFormat [x20000, 150 tasks", "gantt\ndateFormat " + "[".repeat(20000) + "YYYY-MM-DD\nsection S\n" + tasks(150)],
    [
      "venn union of 3231 sets",
      "venn-beta\n" + range(3231, (i) => `set S${i}`) + "\nunion " + range(3231, (i) => `S${i}`, ",") + '["all"]',
    ],
    ["pie accDescr 48k spaces", 'pie\naccDescr {\nx' + " ".repeat(48000) + 'y\n}\n"a": 1'],
    ["over the 50k source cap", "flowchart TD\n%%x\n" + "\n".repeat(400000) + "A-->B"],
    // Five cached adversarial fences from the round-2 review: 32,000 spaces
    // + x + 17,900 spaces, under both caps. The theme toggle must not re-scan
    // them (one linear pass at mount; the cache is consulted first after).
    ...Array.from({ length: 5 }, (_, i) => [
      `whitespace guard ${i + 1}`,
      `pie\n"a${i}": 1\n%%` + " ".repeat(32_000) + "x" + " ".repeat(17_900) + "\n",
    ] as const),
  ];
})();

function HostileFixture() {
  return (
    <div data-fixture-root="" className="@container/settings mx-auto max-w-3xl p-4">
      <header className="mb-3 flex h-8 items-center border-b border-border text-[13px] font-semibold">Transcript</header>
      {HOSTILE.map(([name, source]) => (
        <section key={name} data-audit-row={name} className="border-b border-border py-3">
          <div className="mb-1 font-mono text-2xs text-muted">{name}</div>
          <MermaidBlock
            code={source}
            // The block receives the full source; the harness shows only a
            // bounded stand-in for the code, or the page would lay out tens
            // of thousands of preformatted lines while the measurement runs.
            fallback={
              <pre className="my-1.5 overflow-x-auto border border-border bg-raised px-2.5 py-2">
                <code className="block font-mono text-2xs leading-[18px]">
                  {source.length > 2_000 ? `source of ${source.length} chars (not laid out in this harness)` : source}
                </code>
              </pre>
            }
          />
        </section>
      ))}
    </div>
  );
}

/**
 * The cloud bar (`features/cloud/cloud-bar.tsx`) in each state of the page's
 * way to the device, one strip per state as the app draws it: the real dot,
 * name, device switcher (`MachineSwitcher`, drawn as for an account with two
 * devices, under a router of its own), connection words (`ConnectionLabel`),
 * path indicator and buttons on the bar's own strip (`BAR_STRIP`), with the
 * offline copy's notice, or the refusal's, under it where the app shows one.
 * Each state is drawn at its widest: both countdowns have two digits (12 s
 * and 16 s from the load), and a relay step names the longest relay city as
 * well as Tokyo, since at 390 px every one of them must show whole beside a
 * short device name and the switcher.
 */
const loadedAt = Date.now();
const COPY_AS_OF = Date.UTC(2026, 10, 8, 9, 30);
const TOKYO = { id: "nrt-1", city: "tokyo", region: "nrt", origin: "wss://nrt-1.relay.example", state: "up", load: 0.1 };
const BAR_MACHINES: readonly MachineInfo[] = [
  { key: "studio-mac", name: "studio-mac", state: "online", since: 0, canManage: true, live: "copy" },
  { key: "macbook-air", name: "macbook-air", state: "online", since: 0, canManage: true, live: "copy" },
];
const barMachines = constantStore(BAR_MACHINES);
/** What the switcher reads of droppen.app's shell: the account's devices, and the account (its hints). */
const BAR_SHELL = {
  kind: "hub" as const,
  home: "studio-mac",
  account: { account: "fixture-account", isAdmin: true },
  machines: () => barMachines,
  runtime: () => {
    throw new Error("the cloud-bar fixture opens no device");
  },
  machine: () => null,
  setMain: noop,
};
const CLOUD_BAR_STATES: ReadonlyArray<
  readonly [name: string, view: { label: BarLabel; step?: ConnectStep; retryAt?: number; notice?: boolean; refusal?: ChannelRefusal }]
> = [
  ["hub", { label: "hub" }],
  ["hub-retry", { label: "hubRetry", retryAt: loadedAt + 12_000, notice: true }],
  ["presence", { label: "connecting", step: { kind: "presence" } }],
  ["relay", { label: "connecting", step: { kind: "relay", city: "tokyo" } }],
  ["relay-long", { label: "connecting", step: { kind: "relay", city: "san-francisco" } }],
  ["handshake", { label: "connecting", step: { kind: "handshake" } }],
  ["connecting", { label: "connecting" }],
  ["device-retry", { label: "deviceRetry", retryAt: loadedAt + 16_000, notice: true }],
  ["offline", { label: "offline", notice: true }],
  ["refused", { label: "refused", refusal: { kind: "notAccepted", by: "hub", why: null } }],
  ["unconfirmed", { label: "unconfirmed", refusal: { kind: "unconfirmed" } }],
  ["unsaved", { label: "unsaved", refusal: { kind: "unsaved" } }],
  ["live", { label: "e2e" }],
];

/**
 * `&states=catch-up`: the offline copy catching up from the hub, drawn as the
 * states above are (the switcher, the same shell and router): in the notice
 * under the strip (the device is offline, or keeps failing, so the notice is
 * up) and, with no notice (the device is being connected to, or no copy is on
 * this device yet), in the strip after the connection's words or on a line of
 * its own under it: the session list alone, sessions 3/7, and sessions 3/7
 * with a big checkpoint download 42 % of the way (17 of 40 pages, running for
 * 5 s) — the longest. With the longest relay city, and beside a two-digit
 * retry countdown, which must stay whole.
 */
const counts = (index: number, status: number, session: number) => ({ index, status, session });
const CATCH_UP_INDEX: StreamProgress = { ...NO_PROGRESS, catchingUp: counts(1, 0, 0), synced: counts(0, 1, 0) };
const CATCH_UP_SESSIONS: StreamProgress = { ...NO_PROGRESS, catchingUp: counts(0, 0, 4), synced: counts(1, 1, 3) };
const CATCH_UP_PAGES: StreamProgress = { ...CATCH_UP_SESSIONS, pages: { done: 17, total: 40 }, downloadingSince: loadedAt - 5_000, episode: { done: 17, total: 40 } };
const still = (progress: StreamProgress): CatchUpSource => ({ progress, onProgress: () => noop });
type CatchUpView = { label: BarLabel; dot: "online" | "offline"; step?: ConnectStep; retryAt?: number; notice: boolean; catchUp: StreamProgress };
const CATCH_UP_STATES: ReadonlyArray<readonly [name: string, view: CatchUpView]> = [
  ["catch-up-index", { label: "offline", dot: "offline", notice: true, catchUp: CATCH_UP_INDEX }],
  ["catch-up-sessions", { label: "offline", dot: "offline", notice: true, catchUp: CATCH_UP_SESSIONS }],
  ["catch-up-pages", { label: "offline", dot: "offline", notice: true, catchUp: CATCH_UP_PAGES }],
  ["catch-up-bar-index", { label: "connecting", dot: "online", step: { kind: "handshake" }, notice: false, catchUp: CATCH_UP_INDEX }],
  ["catch-up-bar-sessions", { label: "connecting", dot: "online", step: { kind: "presence" }, notice: false, catchUp: CATCH_UP_SESSIONS }],
  ["catch-up-bar-pages", { label: "connecting", dot: "online", step: { kind: "relay", city: "san-francisco" }, notice: false, catchUp: CATCH_UP_PAGES }],
  ["catch-up-retry", { label: "deviceRetry", dot: "online", retryAt: loadedAt + 16_000, notice: false, catchUp: CATCH_UP_PAGES }],
];

function CatchUpFixture() {
  return (
    <div data-fixture-root="" className="flex flex-col gap-4 py-4">
      {CATCH_UP_STATES.map(([name, view]) => (
        <section key={name} data-audit-row={name}>
          <div className="mb-1 px-3 font-mono text-2xs text-muted">{name}</div>
          <div data-catch-up-shot="">
            <div data-cloud-bar="" className={BAR_STRIP}>
              <DeviceDot state={view.dot} />
              <BarMachineName machine="studio-mac" mirror={null} />
              <MachineSwitcher shell={BAR_SHELL as unknown as HubShell} current="studio-mac" />
              <ConnectionLabel label={view.label} step={view.step} retryAt={view.retryAt} link={{ wake: noop }} controller={{ poke: noop }} />
              {!view.notice && <BarCatchUp source={still(view.catchUp)} />}
              <span className="flex-1" />
              <DevicesButton />
              <AccountButton />
              <SignOutButton onClick={noop} />
            </div>
            {!view.notice && <BarCatchUpLine source={still(view.catchUp)} />}
            {view.notice && <OfflineNotice freshestMs={COPY_AS_OF} catchUp={still(view.catchUp)} />}
          </div>
        </section>
      ))}
    </div>
  );
}

function CloudBarFixture() {
  if (query.get("states") === "catch-up") return <CatchUpFixture />;
  return (
    <div data-fixture-root="" className="flex flex-col gap-4 py-4">
      {CLOUD_BAR_STATES.map(([name, view]) => (
        <section key={name} data-audit-row={name}>
          <div className="mb-1 px-3 font-mono text-2xs text-muted">{name}</div>
          <div data-cloud-bar="" className={BAR_STRIP}>
            <DeviceDot state={view.label === "offline" ? "offline" : "online"} />
            <BarMachineName machine="studio-mac" mirror={null} />
            <MachineSwitcher shell={BAR_SHELL as unknown as HubShell} current="studio-mac" />
            <ConnectionLabel label={view.label} step={view.step} retryAt={view.retryAt} link={{ wake: noop }} controller={{ poke: noop }} />
            {view.label === "e2e" && (
              <PathIndicator
                paths={{ direct: false, relayRttMs: 38.4, directRttMs: null, ups: 0, downs: 0, resent: 0, duplicates: 0 }}
                route={{ kind: "relay", relay: TOKYO, mode: "auto" }}
                onChangeRelay={noop}
              />
            )}
            <span className="flex-1" />
            <DevicesButton />
            <AccountButton />
            <SignOutButton onClick={noop} />
          </div>
          {view.notice && <OfflineNotice freshestMs={COPY_AS_OF} />}
          {view.refusal && <RefusalNotice refusal={view.refusal} limited={view.refusal.kind === "notAccepted"} others={["macbook-air"]} onDevices={noop} />}
        </section>
      ))}
    </div>
  );
}

/** The cloud bar's states, under a router (the switcher navigates) and with the shell it reads in the registry's slot. */
async function mountCloudBar(): Promise<void> {
  setCurrentShell(BAR_SHELL as unknown as AppShell);
  await applyLanguage((query.get("lang") as LanguagePref | null) ?? "en");
  const router = await import("@tanstack/react-router");
  const rootRoute = router.createRootRoute({ component: CloudBarFixture });
  const routes = router.createRouter({ routeTree: rootRoute, history: router.createMemoryHistory({ initialEntries: ["/"] }) });
  await routes.load();
  createRoot(document.getElementById("root")!).render(<router.RouterProvider router={routes} />);
}

/**
 * A session being opened with nothing to show yet
 * (`features/sessions/session-opening.tsx`), in each stage of its wait, as the
 * session view draws it in place of the transcript: connecting, waiting for
 * the server's answer, the offline copy downloading from the hub (with its
 * pages and bar, drawn wide at 128/256, and before a download runs) and
 * retrying after `closed`. The line is said once the wait has gone on for
 * `OPENING_DELAY_MS`, so the audit waits for it.
 */
const SESSION_OPENING_STATES: ReadonlyArray<readonly [name: string, opening: FeedOpening]> = [
  ["connecting", { stage: "connecting" }],
  ["loading", { stage: "loading" }],
  ["copy-pages", { stage: "copy", done: 128, total: 256 }],
  ["copy", { stage: "copy" }],
  ["retrying", { stage: "retrying" }],
];

function SessionOpeningFixture() {
  return (
    <div data-fixture-root="" className="flex flex-col gap-4 py-4">
      {SESSION_OPENING_STATES.map(([name, opening]) => (
        <section key={name} data-audit-row={name}>
          <div className="mb-1 px-3 font-mono text-2xs text-muted">{name}</div>
          <div data-session-view="" className="flex h-28 border-y border-border">
            {/* The session view's own pending branch (`session-detail.tsx`). */}
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              <SessionOpening opening={opening} live="live" />
            </div>
          </div>
        </section>
      ))}
    </div>
  );
}

const NEW_CHAT_ACCOUNT = "11111111-1111-4111-8111-111111111111";
const newChatChats = [
  { kind: "claude", id: "chat-1", title: "Release notes for 0.6", model: "claude-sonnet-5", updatedAt: minutesAgo(12) },
  { kind: "codex", id: "chat-2", title: "Why the relay drops idle sockets", model: "gpt-5.5", updatedAt: minutesAgo(95) },
  { kind: "api", id: "chat-3", title: "Draft the changelog", model: "openai/gpt-5.5", updatedAt: minutesAgo(60 * 26) },
];

/** A device answering the new-chat form's routes with synthetic data only. */
function newChatDevice(signedOut: boolean): Transport {
  const authRest = {
    scope: "all", class: "auth", untilAt: null, reason: "the fixture's login expired", observedAt: "2026-10-19T05:11:15Z",
  };
  const answers: Record<string, unknown> = {
    "/api/accounts": {
      accounts: signedOut
        ? []
        : [{
            id: NEW_CHAT_ACCOUNT, label: "work", kind: "oauth_token", provider: "claude", usageEnabled: false,
            rotation: true, rest: [], createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z",
          }],
      defaultAccountId: null,
      defaultCodexAccountId: null,
      local: {
        claude: { state: signedOut ? "signed_out" : "unknown", rest: signedOut ? [authRest] : [], isDefault: false },
        codex: { state: "unknown", rest: [], isDefault: false },
      },
    },
    ...(signedOut ? {} : { "/api/accounts/start": { accountId: NEW_CHAT_ACCOUNT, reason: "default", model: "fable", placed: true } }),
    "/api/models": {
      models: [
        { id: "fable", label: null, efforts: ["low", "medium", "high"], defaultEffort: "medium" },
        { id: "sonnet", label: null, efforts: ["low", "medium", "high"], defaultEffort: "medium" },
      ],
      source: "cache",
    },
    "/api/composer-prefs": { defaults: {}, sessions: {} },
    "/api/prefs": { composerSend: "enter", sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: "en" },
    "/api/stats/quota": { accounts: [], claude: null, codex: null },
    // Deliberately unconfigured: the api backend's warning is one of the states.
    "/api/chat-provider": { baseUrl: "", hasKey: false, model: "" },
    "/api/chats": { chats: newChatChats },
  };
  return {
    fetch: async (request: Request) => {
      // The chat form uploads through the real composer, so the fixture answers
      // the same route the composer does — with a synthetic path under uploads.
      if (new URL(request.url).pathname === "/api/uploads" && request.method === "POST") {
        const file = (await request.formData()).get("file") as File;
        return Response.json({
          path: `/home/studio/.local/share/remote-hand/uploads/fixture/${file.name}`,
          name: file.name,
          sizeBytes: file.size,
          isImage: file.type.startsWith("image/"),
        });
      }
      // The chat start, answering as the server does for what a tool-less
      // claude chat cannot carry: the refusal state's screenshot is that
      // notice, not a mocked component.
      if (new URL(request.url).pathname === "/api/chats/new" && request.method === "POST") {
        const body = (await request.json()) as { backend?: string; files?: string[] };
        const refused = (body.files ?? []).find((path) => path.endsWith(".bin"));
        if (refused) {
          return Response.json(
            { error: `${refused.split("/").pop()} is not a UTF-8 text file a claude chat can take` },
            { status: 400 },
          );
        }
        return Response.json({ kind: body.backend ?? "claude", id: "chat-new" });
      }
      const answer = answers[new URL(request.url).pathname];
      return answer === undefined ? Response.json({ error: "not here" }, { status: 404 }) : Response.json(answer);
    },
    socketUrl: async (path: string) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

/**
 * The instant-chat home, in the real chat layout: the list beside it on a
 * desktop, the form alone on a phone (`/chat/new`), as the app routes it.
 * `state` picks the account situation; `&state=api` starts on claude and the
 * audit clicks the api tab, which no query can select.
 */
async function mountNewChat(): Promise<void> {
  const state = query.get("state") ?? "claude";
  setShell("local", LOCAL);
  openMachine(LOCAL, newChatDevice(state === "signed-out"), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  try {
    localStorage.clear();
  } catch {
    // storage unavailable: the fixture still draws
  }
  await applyLanguage("en");

  const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
  const router = await import("@tanstack/react-router");
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const chatRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "chat", component: ChatLayout });
  const homeRoute = router.createRoute({ getParentRoute: () => chatRoute, path: "/", component: ChatHome });
  const newRoute = router.createRoute({ getParentRoute: () => chatRoute, path: "new", component: ChatHome });
  const detail = (path: string) => router.createRoute({ getParentRoute: () => chatRoute, path, component: () => null });
  const settingsRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "settings", component: () => null });
  // Below md the list is the page at /chat, and the form has its own address.
  const wide = window.matchMedia("(min-width: 768px)").matches;
  const routes = router.createRouter({
    routeTree: rootRoute.addChildren([
      chatRoute.addChildren([homeRoute, newRoute, detail("claude/$id"), detail("codex/$id"), detail("api/$id")]),
      settingsRoute,
    ]),
    history: router.createMemoryHistory({ initialEntries: [wide ? "/chat" : "/chat/new"] }),
  });
  await routes.load();

  createRoot(document.getElementById("root")!).render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MachineProvider machine={LOCAL}>
        <GlobalFeedProvider>
          <div className="flex h-full min-h-0 min-w-0 flex-1 flex-col md:flex-row">
            <router.RouterProvider router={routes} />
          </div>
        </GlobalFeedProvider>
      </MachineProvider>
    </QueryClientProvider>,
  );
}

/** Nothing attached, and nothing can be: a read-only composer's attachments. */
const NO_ATTACHMENTS = { attachments: [], addFiles: noop, remove: noop, clear: noop, take: () => [], putBack: noop, anyUploading: false, anyFailed: false } as unknown as ComposerAttachments;

/**
 * The foot of a session with no copy on this device yet, as the session
 * composer draws it (`features/sessions/composer.tsx`, without its chips):
 * read-only, its placeholder the reason the offline copy gives
 * (`features/cloud/hub-shell.ts`, `offlineMessages`) — the device offline,
 * only when the hub says so, or else only the copy missing (the page still
 * connecting to the device).
 */
function NoCopyFixture() {
  const { t } = useTranslation();
  const states: [string, string][] = [
    ["offline", t("the device is offline, and there is no copy of this session here yet")],
    ["no-copy", t("there is no copy of this session on this device yet")],
  ];
  return (
    <div data-fixture-root="" className="flex flex-col gap-4 py-4">
      {states.map(([name, reason]) => (
        <section key={name} data-audit-row={name}>
          <div className="mb-1 px-3 font-mono text-2xs text-muted">{name}</div>
          <div className="pb-safe shrink-0 border-t border-border bg-panel">
            <div className="px-3 pt-0 pb-3 max-md:px-2 max-md:pb-2">
              <ComposerBox attachments={NO_ATTACHMENTS} text="" onText={noop} disabled placeholder={reason} onSubmit={noop} send={{ disabled: true, label: t("send"), busy: false }} />
            </div>
          </div>
        </section>
      ))}
    </div>
  );
}

/** The no-copy fixture, in `&lang=`, over a device that answers the composer's preferences. */
async function mountNoCopy(): Promise<void> {
  setShell("local", LOCAL);
  openMachine(LOCAL, newChatDevice(false), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  await applyLanguage((query.get("lang") as LanguagePref | null) ?? "en");
  const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
  createRoot(document.getElementById("root")!).render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MachineProvider machine={LOCAL}>
        <NoCopyFixture />
      </MachineProvider>
    </QueryClientProvider>,
  );
}

/**
 * droppen.app on the machine itself (`live`) opening a session its feed pool
 * does not hold, with a copy of it on this device: the real session view,
 * routed as the app routes it, drawn from the copy at once
 * (`features/sessions/feed-from-copy.ts`) under the banner that says so, its
 * composer waiting for the machine's state before it sends anything, and
 * neither it nor the header showing what only that state can say. The
 * machine never answers here, so both stay; with `&state=live` it does, and
 * the same session is drawn under the machine's own state. With
 * `&state=offline` the machine is away and the page is on its copy, as cloud
 * mode is then (`lib/transport/switching.ts`): every read and the session's
 * feed answered by the offline transport over the copy
 * (`lib/transport/offline.ts`), read-only, and again nothing shown that only
 * the machine's state can say. The copy is a stand-in for the machine's
 * offline copy (`MachineMirror`) holding one session, already verified; the
 * device answers the view's own reads with synthetic data only.
 */
const COPY_MACHINE = "studio-mac";
const copyMode = query.get("state") === "offline" ? "offline" : "live";
const COPY_SESSION = "7c1e2a90-5b3d-4f6e-9a21-0d8c4b7e3f15";
const copyItem = (n: number, role: "user" | "assistant", kind: "text" | "tool_use" | "tool_result", text: string, more: Record<string, unknown> = {}) => ({
  role,
  kind,
  text,
  detail: null,
  isError: false,
  meta: null,
  question: null,
  timestamp: new Date(Date.UTC(2026, 10, 8, 9, 12 + n)).toISOString(),
  toolName: null,
  truncated: false,
  ...more,
});
const COPY_ITEMS = [
  copyItem(0, "user", "text", "The relay drops idle sockets after a minute. Find where that timeout is set, and make it follow the heartbeat instead."),
  copyItem(1, "assistant", "tool_use", "idle_timeout", { toolName: "Grep", toolUseId: "t-1", detail: "rg -n idle_timeout crates/droppen-relay", model: "claude-sonnet-5" }),
  copyItem(2, "user", "tool_result", "3 matches", { resultOf: "t-1", detail: "crates/droppen-relay/src/conn.rs:42:const IDLE_TIMEOUT: Duration = Duration::from_secs(60);\ncrates/droppen-relay/src/conn.rs:118:        if last.elapsed() > IDLE_TIMEOUT {\ncrates/droppen-relay/src/conn.rs:131:    idle_timeout: IDLE_TIMEOUT," }),
  copyItem(3, "assistant", "text", "It is a constant in `conn.rs`: a socket that has said nothing for 60 s is closed, whatever the heartbeat is set to. The heartbeat is every 30 s, so one lost ping is enough to drop a healthy connection.\n\nI will derive it from the heartbeat: three missed beats.", { model: "claude-sonnet-5" }),
  copyItem(4, "assistant", "tool_use", "crates/droppen-relay/src/conn.rs", { toolName: "Edit", toolUseId: "t-2", model: "claude-sonnet-5" }),
  copyItem(5, "user", "tool_result", "The file crates/droppen-relay/src/conn.rs has been updated.", { resultOf: "t-2" }),
  copyItem(6, "assistant", "text", "Done. `IDLE_TIMEOUT` is now `3 * HEARTBEAT`, so a socket is let go only after three beats go unanswered, and the test that pinned 60 s now pins the multiple.", { model: "claude-sonnet-5" }),
];
const COPY_ROW = {
  source: "claude", id: COPY_SESSION, title: "Make the relay's idle timeout follow the heartbeat", cwd: "/home/studio/droppen", filePath: "/home/studio/.claude/projects/droppen/7c1e2a90.jsonl",
  gitBranch: "main", repoKey: null, createdAt: "2026-11-08T09:12:00Z", updatedAt: "2026-11-08T09:19:00Z", sizeBytes: 48_213, scope: "owned",
  status: "idle", activity: "idle", attentionAt: null, unread: false, unreadCount: 0, resident: null, claudeRoot: "managed",
};
/** The account the session runs on, which only the machine's state names. */
const COPY_ACCOUNT = { id: "5d0c6f3e-8a41-4b7e-9f12-3c6e0a9b2d74", label: "work" };
/** A login's two quota windows, by what is left of each. */
const copyQuota = (fiveHours: number, week: number) => [
  { label: "5h", windowMinutes: 300, usedPercent: 100 - fiveHours, remainingPercent: fiveHours, resetsAt: null },
  { label: "weekly", windowMinutes: 10_080, usedPercent: 100 - week, remainingPercent: week, resetsAt: null },
];
/**
 * The machine's own state for the session (`&state=live`), not the defaults:
 * it last ran sonnet at low effort in plan mode, on the account, with its
 * context 42% full.
 */
const COPY_LIVE_STATE = {
  summary: COPY_ROW,
  canSend: true,
  sendBlockReason: null,
  cliChoice: { model: "sonnet", effort: "low", permission: "plan", at: "2026-11-08T09:18:00Z" },
  injection: null,
  contextTokens: 84_000,
  contextWindow: 200_000,
  claudeRoot: "managed",
  account: COPY_ACCOUNT,
  rotation: null,
  receipts: [],
  questions: [],
};
const COPY_STATE: FeedCopy = { started: true, startIndex: 0, endIndex: COPY_ITEMS.length, items: COPY_ITEMS as FeedCopy["items"], fp: "fixture-fp", prefixFp: "fixture-prefix" };
const isCopied = (source: string, id: string) => source === "claude" && id === COPY_SESSION;
/** What the feed reads of the machine's offline copy (`MachineMirror`): the list's row and the session's copy. */
const SESSION_COPY = {
  row: (source: string, id: string) => (isCopied(source, id) ? COPY_ROW : undefined),
  sessionNow: (source: string, id: string) => (isCopied(source, id) ? { source, id, state: COPY_STATE, seq: 12, freshestMs: COPY_AS_OF } : null),
  whenLoaded: async () => undefined,
  sessionHeld: async (source: string, id: string) => SESSION_COPY.sessionNow(source, id),
};
const COPY_INFO: MachineInfo =
  copyMode === "offline"
    ? { key: COPY_MACHINE, name: COPY_MACHINE, state: "offline", since: 0, copyAsOf: COPY_AS_OF, canManage: true, live: "copy" }
    : { key: COPY_MACHINE, name: COPY_MACHINE, state: "online", since: 0, canManage: true, live: "live" };
/** droppen.app's shell as the feed reads it: the machine, its copy, and whether the page is on the machine itself or on the copy. */
const COPY_SHELL = {
  kind: "hub" as const,
  home: COPY_MACHINE,
  account: { account: "fixture-account", isAdmin: true },
  machines: () => constantStore<readonly MachineInfo[]>([COPY_INFO]),
  runtime: () => {
    throw new Error("the session-from-copy fixture has one device, opened by hand");
  },
  machine: (key: string | null | undefined) => (key === COPY_MACHINE ? { mirror: SESSION_COPY, transport: { current: copyMode } } : null),
  setMain: noop,
};
/**
 * What the machine last wrote to its status, as the copy keeps it: the
 * account the session runs on, the local login, and both their quotas. The
 * offline transport serves the accounts and the quota from it.
 */
const COPY_STATUS = {
  asOf: COPY_AS_OF,
  settings: null,
  accounts: {
    accounts: [{ ...COPY_ACCOUNT, kind: "oauth_token", provider: "claude", rotation: false }],
    defaultAccountId: null,
    local: { claude: { state: "signed_in", isDefault: true }, codex: { state: "unknown", isDefault: null } },
  },
  quota: {
    accounts: [{ ...COPY_ACCOUNT, kind: "oauth_token", usageEnabled: true, updatedAt: new Date(COPY_AS_OF).toISOString(), windows: copyQuota(37, 21) }],
    claude: { plan: "Max 5x", tier: null, updatedAt: new Date(COPY_AS_OF).toISOString(), windows: copyQuota(88, 64) },
    codex: null,
  },
  cli: { claude: "2.1.0", codex: null },
  mkEpoch: 0,
};
/** The machine's copy as the offline transport reads it (`MachineMirror` is one): the session's row, its copy and the status. */
const COPY_SOURCE: OfflineSource = {
  whenLoaded: async () => undefined,
  rows: () => [COPY_ROW as never],
  row: (source, id) => SESSION_COPY.row(source, id) as never,
  status: { state: COPY_STATUS },
  session: async (source, id) => SESSION_COPY.sessionNow(source, id),
  sessionNow: (source, id) => SESSION_COPY.sessionNow(source, id),
  sessionProgress: () => null,
  onSessionProgress: () => noop,
  touch: noop,
  onChange: () => noop,
};
/** What the copy says in the app, as `offlineMessages` (`features/cloud/hub-shell.ts`) has it while the hub says the device is offline. */
const OFFLINE_WORDS: OfflineOptions["messages"] = {
  get readOnly() {
    return i18next.t("the device is offline — this is a read-only copy");
  },
  get noCopy() {
    return i18next.t("the device is offline, and there is no copy of this session here yet");
  },
  get unavailable() {
    return i18next.t("the device is offline — this needs it to be online");
  },
};

/**
 * The machine answering at last (`&state=live`): its one socket, as a server
 * that carries session feeds on it answers (`app/feed-hub.ts`). The session is
 * resumed from the copy's own marker with nothing new, and its state follows,
 * as it does on the wire.
 */
function machineSocket() {
  let greeted = false;
  const socket = {
    readyState: 0,
    binaryType: "blob",
    onopen: null as ((event: unknown) => void) | null,
    onmessage: null as ((event: { data: unknown }) => void) | null,
    onclose: null as ((event: unknown) => void) | null,
    send(data: string) {
      const m = JSON.parse(data) as { type?: string; feeds?: { sub: number; source: string; id: string }[] };
      if (m.type === "ping") answer({ type: "pong" });
      if (m.type !== "sub") return;
      if (!greeted) {
        greeted = true;
        answer({ type: "hello" });
      }
      for (const feed of m.feeds ?? []) {
        if (!isCopied(feed.source, feed.id)) continue;
        const end = COPY_ITEMS.length;
        answer({ sub: feed.sub, type: "delta", sig: "live:1", fromIndex: end, items: [], endIndex: end, totalItems: end, prefixFp: COPY_STATE.prefixFp });
        answer({ sub: feed.sub, type: "state", ...COPY_LIVE_STATE });
      }
    },
    close() {},
  };
  const answer = (frame: unknown) => setTimeout(() => socket.onmessage?.({ data: JSON.stringify(frame) }), 0);
  setTimeout(() => {
    socket.readyState = 1;
    socket.onopen?.({});
  }, 0);
  return socket;
}

/**
 * The device behind the view: its reads answered with synthetic data, its
 * sockets never open (the machine is still being asked) unless `live`. The
 * model and effort the list leads with, the local login's quota and the
 * permission a CLI defaults to are all other than the session's own.
 */
function copyDevice(live: boolean): Transport {
  const now = new Date().toISOString();
  const answers: Record<string, unknown> = {
    "/api/prefs": { composerSend: "enter", sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: "en" },
    "/api/composer-prefs": { defaults: {}, sessions: {} },
    "/api/models": {
      models: [
        { id: "fable", label: null, efforts: ["low", "medium", "high"], defaultEffort: "medium" },
        { id: "sonnet", label: null, efforts: ["low", "medium", "high"], defaultEffort: "medium" },
      ],
      source: "cache",
    },
    "/api/accounts": {
      accounts: [{ ...COPY_ACCOUNT, kind: "oauth_token", provider: "claude", usageEnabled: true, rotation: false, rest: [], createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z" }],
      defaultAccountId: null,
      defaultCodexAccountId: null,
    },
    "/api/stats/quota": {
      accounts: [{ ...COPY_ACCOUNT, kind: "oauth_token", usageEnabled: true, updatedAt: now, windows: copyQuota(37, 21) }],
      claude: { plan: "Max 5x", tier: null, updatedAt: now, windows: copyQuota(88, 64) },
      codex: null,
    },
  };
  return {
    fetch: async (request: Request) => {
      const answer = answers[new URL(request.url).pathname];
      return answer === undefined ? Response.json({ error: "not here" }, { status: 404 }) : Response.json(answer);
    },
    socketUrl: async (path: string) => path,
    openSocket: (url: string) =>
      live && url.startsWith("/api/events")
        ? machineSocket()
        : { readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null },
  };
}

async function mountSessionFromCopy(): Promise<void> {
  const device = copyDevice(query.get("state") === "live");
  // On the copy: cloud mode's transport, switched to its offline side, as it
  // stays while the machine is away.
  const transport =
    copyMode === "offline"
      ? new SwitchingTransport({ live: device, offline: offlineTransport(COPY_SOURCE, { user: "cloud:fixture-device", messages: OFFLINE_WORDS, machineOnline: () => false }) }, "offline")
      : device;
  openMachine(COPY_MACHINE, transport, {
    info: constantStore(COPY_INFO),
    ensureLive: noop,
    hold: () => noop,
    dispose: noop,
  });
  setShell("hub", COPY_MACHINE);
  setCurrentShell(COPY_SHELL as unknown as AppShell);
  try {
    localStorage.clear();
  } catch {
    // storage unavailable: the fixture still draws
  }
  await applyLanguage((query.get("lang") as LanguagePref | null) ?? "en");

  const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
  const router = await import("@tanstack/react-router");
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const detailRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "sessions/$source/$id", component: SessionDetail });
  const listRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "sessions", component: () => null });
  const routes = router.createRouter({
    routeTree: rootRoute.addChildren([detailRoute, listRoute]),
    history: router.createMemoryHistory({ initialEntries: [`/sessions/claude/${COPY_SESSION}`] }),
  });
  await routes.load();

  createRoot(document.getElementById("root")!).render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MachineProvider machine={COPY_MACHINE}>
        <GlobalFeedProvider>
          <div data-fixture-root="" data-session-view="" className="flex h-full min-h-0 min-w-0 flex-col">
            <router.RouterProvider router={routes} />
          </div>
        </GlobalFeedProvider>
      </MachineProvider>
    </QueryClientProvider>,
  );
}

// The agents, diagrams, images and session-opening fixtures' words go through
// i18n (counts, "5 min ago", the diagram and picture controls' labels, what an
// opening session waits for): English unless `&lang=` names another of the
// app's languages.
if (fixture === "new-chat") {
  await mountNewChat();
} else if (fixture === "cloud-bar") {
  await mountCloudBar();
} else if (fixture === "no-copy") {
  await mountNoCopy();
} else if (fixture === "session-from-copy") {
  await mountSessionFromCopy();
} else {
  if (fixture === "agents" || fixture === "diagrams" || fixture === "hostile" || fixture === "images" || fixture === "session-opening")
    await applyLanguage((query.get("lang") as LanguagePref | null) ?? "en");
  if (fixture === "images") {
    // Cloud mode is the hub's marker in the shell (`lib/hub/config.ts`),
    // where no picture from elsewhere is ever asked for.
    if (query.get("cloud")) {
      const marker = document.createElement("meta");
      marker.name = "droppen-hub";
      marker.content = "wss://hub.example";
      document.head.prepend(marker);
    } else {
      requestImage(LOADED_IMAGE);
    }
  }
  createRoot(document.getElementById("root")!).render(
    fixture === "devices" ? (
      <DevicesFixture />
    ) : fixture === "agents" ? (
      <AgentsFixture />
    ) : fixture === "diagrams" ? (
      <DiagramsFixture />
    ) : fixture === "hostile" ? (
      <HostileFixture />
    ) : fixture === "images" ? (
      <ImagesFixture />
    ) : fixture === "session-opening" ? (
      <SessionOpeningFixture />
    ) : (
      <SettingsFixture />
    ),
  );
}
