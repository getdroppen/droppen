/*
 * `scripts/ui-audit` renders this: real components in their real composition,
 * with no API, router or machine context. It is a fixture, not a page; keep it
 * in step with `features/settings/`, `features/cloud/` and `components/ui/`.
 *
 * `?fixture=settings` (default), `?fixture=devices` or `?fixture=agents`
 * (`&view=cards|details|empty`), plus `?theme=dark`.
 * `web/src/index.css` excludes this directory from Tailwind's scan
 * (`@source not`), so nothing here can change the production stylesheet. That
 * also means every class below must already occur in `web/src`; the layout
 * strings are imported from the page that owns them.
 */
import "@/index.css";
import { createRoot } from "react-dom/client";
import { Button } from "@/components/ui/button";
import { CHECKBOX_INPUT, CHECKBOX_LABEL, CheckboxMark } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { SelectField } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { DeviceRow, type DeviceRowView } from "@/features/cloud/devices";
import { SectionHead } from "@/features/cloud/ui";
import type { AcpAgentInfo } from "@/features/sessions/acp-agent";
import type { AcpAgentPreset } from "@/features/sessions/acp-agent-presets";
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
const devicesFixture: DeviceRowView[] = [
  device({ id: "m-1", name: "kilerd's Mac mini", self: true, kind: "computer", host: true, presence: "online", path: { relay: "sin-1", rttMs: 32 } }),
  device({ id: "m-2", name: "studio", kind: "computer", host: true, presence: "online", path: { relay: "sin-2", rttMs: 74 } }),
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

// The agents fixture's words go through i18n (counts, "5 min ago"): English
// unless `&lang=` names another of the app's languages.
if (fixture === "agents") await applyLanguage((query.get("lang") as LanguagePref | null) ?? "en");
createRoot(document.getElementById("root")!).render(
  fixture === "devices" ? <DevicesFixture /> : fixture === "agents" ? <AgentsFixture /> : <SettingsFixture />,
);
