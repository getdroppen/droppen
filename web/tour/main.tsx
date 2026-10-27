/*
 * `scripts/landing-shots/cloud` renders this for the homepage's product tour:
 * droppen.app's screens as real components in their real composition, with
 * made-up data and no hub, API, router or account. It is a fixture, not a
 * page; keep it in step with `features/cloud/` and `features/machines/`.
 *
 * `?shot=devices` (default; `&as=phone` when a phone is looking) or
 * `?shot=pair`, plus `?lang=en|zh-CN` and `?theme=dark`.
 * `web/src/index.css` excludes this directory from Tailwind's scan
 * (`@source not`), so nothing here can change the production stylesheet.
 * That also means every class below must already occur in `web/src`: what is
 * not a component of its own is copied from the page that draws it.
 */
import "@/index.css";
import { applyLanguage } from "@/i18n";
import { createRoot } from "react-dom/client";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2, Plus, UserRound } from "lucide-react";
import type { components } from "@/api/schema";
import { cn } from "@/lib/cn";
import type { CloudAccount } from "@/lib/hub/account";
import type { AccountView } from "@/lib/hub/api";
import { meStore } from "@/lib/hub/me";
import type { MachineKey } from "@/lib/machines/key";
import { type DeviceSessions, needsYou, recentSessions } from "@/features/machines/needs-you";
import { NeedsYouLedger } from "@/features/machines/needs-you-ledger";
import { DeviceRow, type DeviceRowView } from "@/features/cloud/devices";
import { AccountName } from "@/features/cloud/my-devices";
import { QrCode } from "@/features/cloud/qr";
import { SignOutChoice } from "@/features/cloud/sign-out-dialog";
import { Centered, CloudPage, CopyButton, Fingerprint, Panel, primary, quiet, SectionHead, Tabs } from "@/features/cloud/ui";

type SessionSummary = components["schemas"]["SessionSummary"];

const query = new URLSearchParams(location.search);
const lang = query.get("lang") === "zh-CN" ? "zh-CN" : "en";
const zh = lang === "zh-CN";
document.documentElement.classList.toggle("dark", query.get("theme") === "dark");
const shot = query.get("shot") ?? "devices";
const viewer = query.get("as") === "phone" ? "phone" : "browser";

const now = Date.now();
const ago = (minutes: number) => new Date(now - minutes * 60_000).toISOString();
const noop = () => undefined;

// Made-up ids, never shaped like a real one (`[amd]-` and 16 base32 characters).
const MACBOOK = "m-q8r2t9w3k5n7v8x2";
const SERVER = "m-h9c4p8z2w6t3k9r8";
const MINI = "m-v8n3x9q2r7k8w4t9";
const BROWSER = "d-k8tq3w9z2n5r7v8m";
const PHONE = "d-p9w2m8x3t7q8n4z9";

const NAMES: Record<string, string> = { [MACBOOK]: "MacBook Pro", [SERVER]: "Linux server", [MINI]: "Mac mini" };

// The account the page is signed in to: only its e-mail is shown (the bar's `AccountName`, off `/v1/me`).
const account = { account: "a-tour", api: { me: () => new Promise<never>(noop) }, session: { token: "" } } as unknown as CloudAccount;
meStore(account).view = { account: "a-tour", email: { address: "dev@example.com", verified: true } } as unknown as AccountView;

/** One row of a device's session list, as its index copy has it. */
function session(over: Partial<SessionSummary> & Pick<SessionSummary, "id" | "title">): SessionSummary {
  return {
    source: "claude",
    scope: "owned",
    status: "waiting",
    activity: "idle",
    asking: false,
    attentionAt: null,
    createdAt: ago(120),
    updatedAt: ago(45),
    cwd: null,
    filePath: "",
    gitBranch: "main",
    repoKey: null,
    resident: null,
    sizeBytes: 0,
    unread: false,
    unreadCount: 0,
    ...over,
  } as SessionSummary;
}

const say = (en: string, cn: string) => (zh ? cn : en);

// The scenario the local app's shots tell too: a question in `shop-api` on the MacBook Pro,
// a finished upgrade in `infra` on the Linux server, and work going on in `web` and `docs`.
const lists: DeviceSessions[] = [
  {
    machine: MACBOOK,
    from: "copy",
    asOf: now - 60_000,
    rows: [
      session({ id: "s-shop", title: say("Add rate limiting to the orders API", "给订单接口加限流"), cwd: "~/code/shop-api", asking: true, activity: "working", status: "running", updatedAt: ago(2), attentionAt: ago(2), unread: true, unreadCount: 1 }),
      session({ id: "s-web", title: say("Fix the checkout page on phones", "修好手机上的结账页"), cwd: "~/code/web", activity: "working", status: "running", updatedAt: ago(0.5), resident: { turnStartedAt: ago(7) } as SessionSummary["resident"] }),
      session({ id: "s-logs", title: say("Tidy up the logging config", "整理日志配置"), cwd: "~/code/shop-api", updatedAt: ago(180) }),
    ],
  },
  {
    machine: SERVER,
    from: "copy",
    asOf: now - 60_000,
    rows: [
      session({ id: "s-node", source: "codex", title: say("Bump Node to 22", "升级 Node 到 22"), cwd: "~/infra", updatedAt: ago(6), attentionAt: ago(6), unread: true, unreadCount: 3 }),
      session({ id: "s-docs", title: say("Write the deploy guide", "写部署指南"), cwd: "~/docs", activity: "working", status: "running", updatedAt: ago(1), resident: { turnStartedAt: ago(3) } as SessionSummary["resident"] }),
    ],
  },
  {
    machine: MINI,
    from: "copy",
    asOf: now - 26 * 3_600_000,
    away: true,
    rows: [session({ id: "s-img", title: say("Compress the product images", "压缩商品图片"), cwd: "~/code/web", updatedAt: ago(27 * 60) })],
  },
];
const needs = needsYou(lists);
const listOf = new Map(lists.map((l) => [l.machine, l]));

const view = (over: Partial<DeviceRowView> & Pick<DeviceRowView, "id">): DeviceRowView => ({
  fingerprint: "",
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
const host = (id: MachineKey, over: Partial<DeviceRowView>): DeviceRowView => {
  const rows = listOf.get(id)?.rows ?? null;
  return view({
    id,
    name: NAMES[id] ?? null,
    kind: "computer",
    host: true,
    accept: { type: "admins" },
    attention: rows ? (needs.byMachine.get(id) ?? null) : null,
    recent: recentSessions(rows, 3),
    hasCopy: rows !== null,
    ...over,
  });
};

// `DevicesPanel`'s order: this device, then the devices that run AI, then the other browsers and phones.
// None is marked the main device: next to "can run AI" its badge cuts an English name short in a 1280px card.
const browser = view({ id: BROWSER, name: "Mac · Chrome", self: viewer === "browser", kind: viewer === "browser" ? "browser" : "browserOrPhone", full: true, presence: viewer === "browser" ? "online" : null });
const phone = view({ id: PHONE, name: "iPhone · Safari", self: viewer === "phone", kind: viewer === "phone" ? "phone" : "browserOrPhone", full: true, presence: viewer === "phone" ? "online" : null });
const devices: DeviceRowView[] = [
  viewer === "phone" ? phone : browser,
  host(MACBOOK, { presence: "online", path: { relay: "fra-1", rttMs: 18 } }),
  host(SERVER, { presence: "online", path: { relay: "fra-1", rttMs: 24 } }),
  host(MINI, { presence: "offline", copyAsOf: now - 26 * 3_600_000, lastSeenMs: now - 26 * 3_600_000 }),
  viewer === "phone" ? browser : phone,
];

/** 「我的设备」 as `MyDevicesScreen` draws it: the frame (`MyDevicesFrame`), the head and the list (`DevicesPanel`), the ledger. */
function DevicesShot() {
  const { t } = useTranslation();
  const offline = devices.filter((d) => d.host && d.presence === "offline").length;
  const summary = [t("{{n}} devices", { n: devices.length }), offline > 0 ? t("{{n}} offline", { n: offline }) : null, needs.entries.length > 0 ? t("{{n}} need you", { n: needs.entries.length }) : null]
    .filter((part): part is string => part !== null)
    .join(" · ");
  const backName = NAMES[MACBOOK]!;
  return (
    <CloudPage
      aside={
        <>
          <AccountName account={account} link={null as never} />
          <button type="button" className={cn(quiet, "h-7 min-w-0 px-2")} title={t("back to {{name}}", { name: backName })} aria-label={t("back to {{name}}", { name: backName })}>
            <ArrowLeft className="size-3.5 shrink-0" />
            <span className="max-w-40 truncate max-sm:hidden">{backName}</span>
          </button>
          <button type="button" className={cn(quiet, "h-7 px-2")} title={t("account settings")} aria-label={t("account settings")}>
            <UserRound className="size-3.5 shrink-0" />
            <span className="max-sm:hidden">{t("my account")}</span>
          </button>
          <SignOutChoice account={account}>
            {(ask) => (
              <button type="button" className={cn(quiet, "h-7 px-2")} onClick={ask}>
                {t("sign out")}
              </button>
            )}
          </SignOutChoice>
        </>
      }
    >
      <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-4 px-6 pt-8 pb-2 max-md:px-4 max-md:pt-6">
        <div className="flex min-w-0 flex-col gap-1.5">
          <h1 data-testid="devices-title" className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-fg max-md:text-[28px]">
            {t("my devices")}
          </h1>
          <p data-testid="devices-summary" className="text-[13px] leading-5 text-muted">
            {summary}
          </p>
        </div>
        <button type="button" data-variant="primary" data-testid="add-device" className={cn(primary, "max-md:w-full")}>
          <Plus className="size-3.5" />
          {t("add a device")}
        </button>
      </div>
      <div className="flex flex-col gap-8 pb-12">
        <section data-testid="devices-section" className="@container/devices">
          <div className="px-6 max-md:px-4">
            <SectionHead index="03" title={t("devices")}>
              <span className="font-mono text-xs text-faint tabular-nums">{devices.length}</span>
            </SectionHead>
          </div>
          <ul
            data-testid="device-list"
            className="mt-4 grid grid-cols-1 items-start gap-2.5 px-6 max-md:px-4 @min-[44rem]/devices:grid-cols-[repeat(auto-fill,minmax(20rem,1fr))] @min-[44rem]/devices:gap-3"
          >
            {devices.map((v) => (
              <DeviceRow
                key={v.id}
                view={v}
                viewerAdmin
                upgraded
                writable
                onRemove={noop}
                onHost={noop}
                onControl={noop}
                onRename={v.host ? noop : undefined}
                onTerminal={v.host ? noop : undefined}
                onResend={noop}
                onOpen={v.host ? noop : undefined}
                onOpenSession={v.host ? noop : undefined}
              />
            ))}
          </ul>
        </section>
        <NeedsYouLedger needs={needs} nameOf={(key) => NAMES[key] ?? key} devices note={t("from each device's encrypted index")} onOpen={noop} />
      </div>
    </CloudPage>
  );
}

// A made-up offer and fingerprint: what a new browser shows while it waits to be let in (`PairingResponder`'s `offer`).
const OFFER = "RHP1-GAXQ6WXCVJ3XU2X42IJY5NUJ7QQ42C64NQ2KSOWYJIR6OKVPGD6RGXNKUKSE2SCTLEZARG5C7NXZXY4IXVL2JCA4CCKXRFLOPRZSVSBBZIQMEO7OK4KV2UVMSYQAFOAPPF5ADCCCKVVGWT3Q43RKCNLZ";
const FINGERPRINT = "NJAH-DG4J-AQFG-XZXW";

/** The new device's side of letting it in, as `PairingScreen` draws it on the QR code tab, waiting for the other device. */
function PairShot() {
  const { t } = useTranslation();
  const options = [
    { id: "qr" as const, label: t("QR code") },
    { id: "code" as const, label: t("use a code instead") },
    { id: "recover" as const, label: t("recovery words") },
  ];
  return (
    <Centered>
      <Panel title={t("add this browser to your account")}>
        <Tabs value="qr" onChange={noop} options={options} />
        <div className="flex flex-col gap-2">
          <p className="text-xs leading-5 text-muted">{t("On a device already in this account, open my devices → add a browser or phone, and scan this code — or paste its text there.")}</p>
          <QrCode text={OFFER} label={t("pairing code for this device")} />
          <code data-testid="pairing-text" className="max-h-16 overflow-y-auto border border-border bg-raised p-2 font-mono text-2xs break-all text-fg select-all">
            {OFFER}
          </code>
          <CopyButton text={OFFER} label={t("copy the text")} />
          <p className="text-2xs text-faint">
            {t("this device")}: <Fingerprint value={FINGERPRINT} />
          </p>
          <p className="flex items-center gap-2 text-2xs text-faint">
            <Loader2 className="size-3 animate-spin" />
            {t("waiting for the other device…")}
          </p>
        </div>
        <div className="flex items-center justify-between text-2xs text-faint">
          <span className="truncate font-mono">{PHONE}</span>
          <button type="button" className={quiet}>
            {t("sign out")}
          </button>
        </div>
      </Panel>
    </Centered>
  );
}

// The app fetches a language's tables when it first shows it: `applyLanguage` waits for them.
void applyLanguage(lang).then(() => {
  document.documentElement.lang = lang;
  createRoot(document.getElementById("root")!).render(shot === "pair" ? <PairShot /> : <DevicesShot />);
});
