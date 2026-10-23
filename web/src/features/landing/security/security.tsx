import { Fragment, type ReactNode, useEffect } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Info } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { cn } from "@/lib/cn";
import { landingBundles } from "../copy";
import { FooterLine, PublicHeader, SECURITY_EMAIL, Section, SectionHead } from "../page-kit";
import { Formula, FormulaProvider, MentionScope, withConcepts } from "./formula";
import { RepoLink } from "./repo-link";
import { securityBundles } from "./security-copy";

/*
 * droppen.app/security: how the end-to-end encryption works, for a technical
 * reader — the overview and what Droppen Hub sees, the signed device chain,
 * the keys, the channels and relays, the offline copies, the identity renewal
 * of a device that runs AI (switched on at droppen.app for early testing,
 * marked so),
 * the threat model, and how to check all of it. Signed in or not; its own
 * lazy chunk with its own words (`security-copy.ts`), drawn like the other
 * public pages: numbered sections with an on-page table of contents, each
 * mechanism in a section of its own with an anchor.
 *
 * Only what main ships is described as live; every claim traces to the
 * protocol code (`server-rs/crates/rh-proto`, `web/src/lib/e2e`) or the
 * public design notes (docs/cross-machine-architecture.md, docs/local-
 * aggregator.md §3, docs/web-integrity.md, docs/relay-and-plans.md §2,
 * docs/hub-api/, SECURITY.md, the README's "What the Hub sees"); the
 * renewal section follows docs/host-identity-rotation.md. The labels and
 * formulas in the mono blocks are the code's own bytes, quoted, so they are
 * not translated; every part of them explains itself on hover, focus or tap
 * (`formula.tsx`, its words and code references in `formula-data.ts`).
 * Product words as elsewhere: a device, a device that runs AI (the chain's
 * host), a full device (its admin), the content key (`MK`).
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const bundles of [landingBundles, securityBundles]) {
  for (const [lng, bundle] of Object.entries(bundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);
}

/** When the page last changed, as the page says it. */
export const SECURITY_UPDATED = "2026-10-24";

/** Where to report a vulnerability (`page-kit.tsx`, so the terms can name it without this page's chunk). */
export { SECURITY_EMAIL };

/** The page's sections, in order: the table of contents and the anchors. */
export const SECURITY_SECTIONS = ["overview", "chain", "keys", "channels", "offline", "renewal", "threats", "verify"] as const;
/** The identity renewal's parts, each with an anchor of its own. */
export const RENEWAL_PARTS = ["renewal-why", "renewal-entry", "renewal-staging", "renewal-policy", "renewal-disputes", "renewal-groups", "renewal-hub"] as const;

const prose = "max-w-[46em] text-[13px] leading-[21px] text-muted";

/**
 * A translated line in the page's mini-markup, written the same way in every
 * language, so each line stays one key (the English):
 * - `[[text|anchor]]`, a cross-reference: `<a href="#anchor">text</a>`;
 * - `[[path]]` or `[[text|path]]` (a target that is not an anchor): a path
 *   in the repository, linked on GitHub (`RepoLink`); `|/` is the repository;
 * - `**term**`, a key term: `<strong>` (it may hold a cross-reference);
 * - `**term** (reason)` or `**term**（理由）`: a parenthesis that follows a key
 *   term at once (after one space, or none) is that term's reason, kept in
 *   normal weight and the muted colour whatever holds it;
 * - `\n- `: a line that starts with "- " is a list item; items in a row make
 *   one `<ul>`, an ink square per item as in `List`, and any other line is a
 *   paragraph: the lead-in before the items, or a closing line after them.
 * A line with no "\n" stays inline (it fits in a `<p>` or a `<span>`); one
 * with lines becomes blocks (`<p>` and `<ul>` in a `<div>`), so `P` and `Note`
 * draw it in a `<div>`, and list items and table cells are `<div>`s. `small`:
 * in the 12px / 20px text of a table cell or a note.
 */
export function linked(node: ReactNode, small = false): ReactNode {
  if (typeof node !== "string") return node;
  if (!hasLines(node)) return inline(node);
  const blocks: ReactNode[] = [];
  let items: string[] = [];
  const close = () => {
    if (items.length > 0) blocks.push(<Items key={blocks.length} items={items} small={small} />);
    items = [];
  };
  for (const line of node.split("\n")) {
    if (line.startsWith("- ")) items.push(line.slice(2));
    else {
      close();
      blocks.push(<p key={blocks.length}>{inline(line)}</p>);
    }
  }
  close();
  return <div className={cn("flex flex-col", small ? "gap-1.5" : "gap-2")}>{blocks}</div>;
}

/** Whether a translated line is drawn as blocks (it has lines: list items, a lead-in, a closing line). */
function hasLines(node: ReactNode): node is string {
  return typeof node === "string" && node.includes("\n");
}

const MARKUP = /\*\*(.+?)\*\*|\[\[([^\]|]+)(?:\|([^\]|]+))?\]\]/g;

/** One line's key terms, their reasons and its cross-references. */
function inline(line: string): ReactNode {
  if (!line.includes("**") && !line.includes("[[")) return withConcepts(line);
  const out: ReactNode[] = [];
  const marks = new RegExp(MARKUP);
  let at = 0;
  for (let m = marks.exec(line); m; m = marks.exec(line)) {
    if (m.index > at) out.push(<Fragment key={`t${at}`}>{withConcepts(line.slice(at, m.index))}</Fragment>);
    if (m[1] !== undefined) {
      out.push(
        <strong key={m.index} className="font-semibold text-fg">
          {inline(m[1])}
        </strong>,
      );
      const why = reason(line, marks.lastIndex);
      if (why) {
        if (why.space) out.push(" ");
        out.push(
          <span key={`${m.index}-why`} className="font-normal text-muted">
            {inline(why.text)}
          </span>,
        );
        marks.lastIndex = why.end;
      }
    } else if (/^[a-z0-9-]+$/.test(m[3] ?? m[2]!)) {
      out.push(
        <a key={m.index} href={`#${m[3] ?? m[2]}`} className="text-fg underline underline-offset-4 hover:no-underline">
          {m[2]}
        </a>,
      );
    } else {
      out.push(
        <RepoLink key={m.index} path={m[3] ?? m[2]!}>
          {m[2]}
        </RepoLink>,
      );
    }
    at = marks.lastIndex;
  }
  if (at < line.length) out.push(<Fragment key={`t${at}`}>{withConcepts(line.slice(at))}</Fragment>);
  return out;
}

/** The parenthesis, `(…)` or `（…）`, that starts at `from` (after one space or none), up to the one that closes it. */
function reason(line: string, from: number): { text: string; end: number; space: boolean } | null {
  const space = line[from] === " ";
  const start = space ? from + 1 : from;
  if (line[start] !== "(" && line[start] !== "（") return null;
  let depth = 0;
  for (let i = start; i < line.length; i++) {
    if (line[i] === "(" || line[i] === "（") depth++;
    else if (line[i] === ")" || line[i] === "）") {
      depth--;
      if (depth === 0) return { text: line.slice(start, i + 1), end: i + 1, space };
    }
  }
  return null;
}

/** A line's list items: drawn like `List`, the square beside the first line of each. */
function Items({ items, small }: { items: string[]; small: boolean }) {
  return (
    <ul className={cn("flex flex-col", small ? "gap-1" : "gap-2")}>
      {items.map((item, i) => (
        <li key={i} className="flex gap-2.5">
          <span className={cn("size-1.5 shrink-0 bg-fg", small ? "mt-[7px]" : "mt-2")} />
          <span className="min-w-0">{inline(item)}</span>
        </li>
      ))}
    </ul>
  );
}

/** A paragraph of a topic (a `<div>` when it holds a list). */
function P({ children }: { children: ReactNode }) {
  const Block = hasLines(children) ? "div" : "p";
  return <Block className={prose}>{linked(children)}</Block>;
}

/** A topic's list: an ink square per item, as on the other public pages, in the topic's reading size. */
function List({ items }: { items: ReactNode[] }) {
  return (
    <ul className="flex max-w-[46em] flex-col gap-2">
      {items.map((item, i) => (
        <li key={i} className="flex gap-2.5 text-[13px] leading-[21px] text-muted">
          <span className="mt-2 size-1.5 shrink-0 bg-fg" />
          <div className="min-w-0">{linked(item)}</div>
        </li>
      ))}
    </ul>
  );
}

/** The code's own bytes, quoted, never translated (the formulas: `Formula`). */
function Spec({ children }: { children: ReactNode }) {
  return <div className="flex max-w-[46em] flex-col gap-1 border-l border-border-strong py-0.5 pl-3 font-mono text-2xs leading-4 text-fg [overflow-wrap:anywhere]">{children}</div>;
}

function Line({ children }: { children: ReactNode }) {
  return <code className="block">{children}</code>;
}

/** A small caps chip: what is still in testing. */
function Chip({ children }: { children: ReactNode }) {
  return <span className="inline-flex shrink-0 items-center border border-fg px-1.5 font-mono text-[10px] leading-4 font-semibold tracking-wider text-fg uppercase">{children}</span>;
}

/** One mechanism: its number and name on the left (above, when narrow), what it does on the right; `#t2-6` and the like unless it has an id of its own. */
function Topic({ id, n, title, children }: { id?: string; n: string; title: string; children: ReactNode }) {
  return (
    <div id={id ?? `t${n.replace(".", "-")}`} className="grid scroll-mt-14 border-t border-border lg:grid-cols-[minmax(0,1fr)_minmax(0,2.4fr)]">
      <div className="flex gap-3 px-6 pt-6 max-md:px-4 max-md:pt-5 lg:pb-6">
        <span className="w-7 shrink-0 font-mono text-2xs leading-5 text-faint tabular-nums">{n}</span>
        <h3 className="text-[13px] leading-5 font-semibold text-balance text-fg">{withConcepts(title)}</h3>
      </div>
      <div className="flex min-w-0 flex-col gap-4 px-6 pt-3 pb-6 max-md:px-4 max-md:pb-5 lg:border-l lg:border-border lg:pt-6">{children}</div>
    </div>
  );
}

/** Two lists side by side (one above the other when narrow). */
function Pair({ left, right }: { left: { title: string; items: string[] }; right: { title: string; items: string[] } }) {
  return (
    <div className="grid max-w-[60em] gap-6 md:grid-cols-2">
      {[left, right].map((side) => (
        <div key={side.title} className="flex min-w-0 flex-col gap-3">
          <span className="text-xs leading-5 font-semibold text-fg">{withConcepts(side.title)}</span>
          <List items={side.items} />
        </div>
      ))}
    </div>
  );
}

/** A table that reads row by row: a grid from 768px, each row's cells stacked under its name below. */
function Table({ label, heads, rows }: { label: string; heads: string[]; rows: string[][] }) {
  const cols = heads.length;
  const grid = cols === 3 ? "md:grid-cols-[minmax(0,1fr)_minmax(0,1.6fr)_minmax(0,1.6fr)]" : "md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.3fr)_minmax(0,1.3fr)_minmax(0,1fr)]";
  return (
    <div role="table" aria-label={label} className="border-t border-border">
      <div role="row" className={cn("grid border-b border-border max-md:hidden", grid)}>
        {heads.map((h, i) => (
          <span key={h} role="columnheader" className={cn("px-6 py-3", i > 0 && "border-l border-border")}>
            <CapsLabel className="text-fg">{h}</CapsLabel>
          </span>
        ))}
      </div>
      {rows.map((cells) => (
        <div key={cells[0]} role="row" className={cn("grid border-b border-border last:border-b-0", grid)}>
          {cells.map((cell, i) => (
            <div
              key={i}
              role={i === 0 ? "rowheader" : "cell"}
              className={cn(
                "min-w-0 px-6 text-xs leading-5 max-md:px-4",
                i === 0 ? "pt-3.5 font-semibold text-fg md:pb-3.5" : "py-3.5 text-muted max-md:pt-1 max-md:pb-0 max-md:last:pb-3.5",
                i > 0 && "md:border-l md:border-border",
              )}
            >
              {i > 0 && (
                <CapsLabel size="xs" className="block pb-0.5 md:hidden">
                  {heads[i]}
                </CapsLabel>
              )}
              {linked(cell, true)}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

/** A line for what the page leaves out on purpose, or what is not done. */
function Note({ children }: { children: ReactNode }) {
  const Block = hasLines(children) ? "div" : "p";
  return (
    <div className="flex items-start gap-3 border-t border-border px-6 py-4 max-md:px-4">
      <Info className="mt-0.5 size-4 shrink-0 text-muted" />
      <Block className="max-w-[60em] text-xs leading-5 text-muted">{linked(children, true)}</Block>
    </div>
  );
}

function Hero() {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
      <CapsLabel>{t("Security")}</CapsLabel>
      <h1 className="max-w-[18em] text-[44px] leading-[1.05] font-semibold tracking-[-0.035em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[30px]">
        {t("How Droppen's end-to-end encryption works.")}
      </h1>
      <p className="max-w-[44em] text-[15px] leading-[26px] text-muted max-md:text-sm max-md:leading-6">
        {t("A technical description for anyone who wants to check our claims: who holds which key, how your devices agree on who belongs to your account, what our servers can and cannot see, and what each of them could still do if it turned against you. It describes what Droppen does today; the parts that are not live yet, or still in testing, are marked as such.")}
      </p>
      <p className="font-mono text-2xs text-faint">{t("Last updated {{date}}", { date: SECURITY_UPDATED })}</p>
    </div>
  );
}

/**
 * The page's two kinds of device, defined before anything else says them:
 * what each is, which keys it holds, what it may do (the chain's host and
 * admin flags, `rh-proto` chain.rs: a host is never an admin, an admin may
 * control any host; the keys table below; with local aggregation, on by
 * default, a host also takes the content keys other hosts' accept lists let
 * it have, wrap.rs `HOST_KEY_PURPOSE`, and the local web app shows its own
 * device's view and holds no keys, server-rs/src/aggregator/mod.rs). Every
 * later mention explains itself with the same words (`withConcepts`), one
 * tab stop per word in each section (`MentionScope`).
 */
function Definitions() {
  const { t } = useTranslation();
  const cards = [
    {
      id: "host",
      name: t("A device that runs AI"),
      rows: [
        [t("What it is"), t("A device running the droppen program or the desktop app: your AI sessions run there, and so do their commands.")],
        [t("Keys it holds"), t("Its identity key and its content keys. With local aggregation (on by default), also the content keys of the other devices that run AI whose accept list lets it in.")],
        [t("What it may do"), t("Be controlled by your full devices, and by another device only if its accept list allows it. It is never a full device: it signs no chain entry and holds no user key.")],
      ],
    },
    {
      id: "client",
      name: t("Your browsers and phones"),
      rows: [
        [t("What it is"), t("Where you watch and control from: my.droppen.app, in a browser or on a phone. The local web app is different: it shows the view of the device that serves it and holds no keys.")],
        [t("Keys it holds"), t("Its device key, and the user key, which opens every content key in the account's keyring.")],
        [t("What it may do"), t("Each one you pair now is a full device: it adds, approves and removes devices, vouches for the chain, and may control your devices that run AI.")],
      ],
    },
  ];
  return (
    <div role="group" aria-label={t("Two words this page uses")} data-testid="security-terms" className="grid border-t border-border md:grid-cols-2">
      {cards.map((card, i) => (
        <div key={card.id} data-concept-def={card.id} className={cn("flex flex-col gap-2.5 border-border px-6 py-5 max-md:px-4", i > 0 && "max-md:border-t md:border-l")}>
          <dfn className="text-[13px] leading-5 font-semibold text-fg not-italic">{card.name}</dfn>
          <dl className="grid grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-4 gap-y-1.5">
            {card.rows.map(([label, text]) => (
              <div key={label} className="contents">
                <dt className="label-caps pt-[3px] text-[10px] leading-[14px] text-faint">{label}</dt>
                <dd className="text-xs leading-5 text-muted">{withConcepts(text!)}</dd>
              </div>
            ))}
          </dl>
        </div>
      ))}
    </div>
  );
}

function Contents() {
  const { t } = useTranslation();
  const entries: { id: (typeof SECURITY_SECTIONS)[number]; title: string; line: string; chip?: string }[] = [
    { id: "overview", title: t("Overview"), line: t("What Droppen Hub and the relays see, and what they never see.") },
    { id: "chain", title: t("The device chain"), line: t("The signed list every device checks for itself.") },
    { id: "keys", title: t("Keys"), line: t("Every key, where it lives, how it is delivered, and the recovery words.") },
    { id: "channels", title: t("Channels and relays"), line: t("Noise end to end, through a relay, Droppen Hub or a direct path.") },
    { id: "offline", title: t("Offline copies"), line: t("Encrypted streams with signed heads, readable while a device is off.") },
    { id: "renewal", title: t("Identity renewal"), line: t("Replacing the identity key of a device that runs AI."), chip: t("In testing") },
    { id: "threats", title: t("Threat model"), line: t("What a hostile Hub, a thief or the network can and cannot do.") },
    { id: "verify", title: t("Verify it yourself"), line: t("Algorithms, labels, code, test vectors, and how to report a flaw.") },
  ];
  return (
    <Section id="contents">
      <nav aria-label={t("On this page")}>
        <ol data-testid="security-contents" className="grid sm:grid-cols-2 lg:grid-cols-4">
          {entries.map((e, i) => (
            <li
              key={e.id}
              className={cn(
                "border-border",
                i > 0 && "max-sm:border-t",
                i % 2 === 1 && "sm:max-lg:border-l",
                i > 1 && "sm:max-lg:border-t",
                i % 4 > 0 && "lg:border-l",
                i > 3 && "lg:border-t",
              )}
            >
              <a href={`#${e.id}`} className="group flex h-full flex-col gap-1.5 px-6 py-5 hover:bg-hover max-md:px-4">
                <span className="flex items-center gap-2">
                  <span className="font-mono text-2xs leading-4 text-faint tabular-nums">{String(i + 1).padStart(2, "0")}</span>
                  <span className="text-[13px] leading-5 font-semibold text-fg group-hover:underline group-hover:underline-offset-4">{e.title}</span>
                  {e.chip && <Chip>{e.chip}</Chip>}
                </span>
                <span className="text-xs leading-5 text-muted">{e.line}</span>
              </a>
            </li>
          ))}
        </ol>
      </nav>
    </Section>
  );
}

/** Who holds what, and where the ciphertext goes: from 768px a figure; below it, the same four boxes in a column. */
function Overview() {
  const { t } = useTranslation();
  const devices = { title: t("Your browsers and phones"), lines: [t("device key · user key"), t("encrypt and decrypt here")] };
  const relay = { title: t("Droppen relay"), lines: [t("copies frames, keeps nothing"), t("none of your keys")] };
  const host = { title: t("A device that runs AI"), lines: [t("identity key · content keys"), t("the AI and its transcripts")] };
  const hub = { title: "Droppen Hub", lines: [t("device chain · mail · encrypted copies"), t("channels no relay carries"), t("none of your keys")] };
  const wire = t("Noise ciphertext");
  const e2e = t("one end-to-end channel: Noise IK");
  const sealed = t("sealed records · signed heads");
  const copy = t("encrypted copy");
  const label = t("Your browsers and phones and a device that runs AI share one end-to-end channel through a Droppen relay, or through Droppen Hub where no relay carries it; the device that runs AI also leaves sealed records with Droppen Hub, which your browsers and phones read back as an encrypted copy. Neither server holds any of your keys.");
  const card = (x: number, y: number, w: number, h: number, c: { title: string; lines: string[] }) => (
    <g key={c.title}>
      <rect x={x + 0.5} y={y + 0.5} width={w} height={h} className="fill-bg stroke-border-strong" strokeWidth={1} />
      <text x={x + 14} y={y + 26} className="fill-fg text-[13px] font-semibold">
        {c.title}
      </text>
      {c.lines.map((line, i) => (
        <text key={line} x={x + 14} y={y + 50 + i * 18} className="fill-muted font-mono text-[11px]">
          {line}
        </text>
      ))}
    </g>
  );
  return (
    <div className="border-t border-border" data-diagram="security-overview">
      <svg viewBox="0 0 1040 320" role="img" aria-label={label} className="block w-full max-md:hidden">
        {/* The end-to-end span over the three. */}
        <path d="M140.5 34v8M900.5 34v8M140.5 34.5H900.5" className="stroke-muted" strokeWidth={1} fill="none" />
        <text x={520} y={26} textAnchor="middle" className="fill-fg font-mono text-[11px]">
          {e2e}
        </text>
        {/* The channel: ink, 2px, through the relay. */}
        <path d="M256 92H404M636 92H784" className="stroke-fg" strokeWidth={2} fill="none" />
        <text x={330} y={84} textAnchor="middle" className="fill-muted font-mono text-[10px]">
          {wire}
        </text>
        <text x={710} y={84} textAnchor="middle" className="fill-muted font-mono text-[10px]">
          {wire}
        </text>
        {/* The copy: dashed, 1.5px, an L each way. */}
        <path d="M900 136V242.75H636M404 242.75H140V136" className="stroke-fg" strokeWidth={1.5} strokeDasharray="5 4" fill="none" />
        <text x={768} y={232} textAnchor="middle" className="fill-muted font-mono text-[10px]">
          {sealed}
        </text>
        <text x={272} y={232} textAnchor="middle" className="fill-muted font-mono text-[10px]">
          {copy}
        </text>
        {card(24, 48, 232, 88, devices)}
        {card(404, 48, 232, 88, relay)}
        {card(784, 48, 232, 88, host)}
        {card(404, 200, 232, 104, hub)}
      </svg>
      <ol className="flex flex-col gap-0 px-4 py-5 md:hidden" aria-label={label}>
        {[
          { c: devices, next: wire },
          { c: relay, next: wire },
          { c: host, next: sealed },
          { c: hub, next: copy },
        ].map(({ c, next }, i) => (
          <li key={c.title} className="flex flex-col">
            <div className="flex flex-col gap-1 border border-border-strong px-3 py-2.5">
              <span className="text-[13px] leading-5 font-semibold text-fg">{withConcepts(c.title)}</span>
              {c.lines.map((line) => (
                <span key={line} className="font-mono text-2xs leading-4 text-muted">
                  {line}
                </span>
              ))}
            </div>
            <span className="py-1.5 pl-3 font-mono text-[10px] leading-4 text-muted">
              {i < 3 ? "↓ " : "↑ "}
              {next}
              {i === 3 && <> → {withConcepts(devices.title)}</>}
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

function OverviewSection() {
  const { t } = useTranslation();
  return (
    <Section id="overview">
      <MentionScope>
        <SectionHead
          label={`01 · ${t("Overview")}`}
          title={withConcepts(t("Your devices hold the keys. Our servers carry ciphertext."))}
          lead={withConcepts(t("Droppen runs Claude Code and Codex on your devices that run AI (in the protocol, hosts), and your browsers and phones control them over channels encrypted end to end. Droppen Hub, at droppen.app, keeps your account, the device chain and the encrypted copies, and serves the web app at my.droppen.app; Droppen relays, or Droppen Hub where no relay carries a channel, pass a channel's ciphertext on. Neither holds a key that can decrypt anything."))}
        />
        <Overview />
        <Topic n="1.1" title={t("What Droppen Hub sees, and what it never sees")}>
          <Pair
            left={{
              title: t("What it sees"),
              items: [
                t("**Your account** (Hub):\n- the e-mail address and whether it is verified\n- a password hash or your passkeys' public keys\n- which provider made each (from its AAGUID) and the name you gave it\n- the plan and the invite it came with\n- the usage it is counted on (bytes stored and transferred)"),
                t("A record of the last 30 days' **changes to how you sign in**: which change, from which device, and the passkey's name (Hub)."),
                t("The **IP address** of every request and connection (Hub)."),
                t("What **kind of device** each browser or phone is, as a short name such as “iPhone · Safari” made from its User-Agent; it does not keep the User-Agent itself (Hub)."),
                t("Each device's **random id and public keys**, when it joined and was last seen, when a device that runs AI went online or offline, which relays it is on and its round trip to each (a rough location), and the version and protocol features it declares (Hub)."),
                t("**The device chain**: which devices belong to the account, which of them run AI, and which device added which."),
                t("Which device that runs AI is the account's **main device**."),
                t("For **what it stores encrypted** (session streams and their pages, sealed key mail, keyring entries):\n- how many\n- how large\n- when written\n- which device wrote each\n- which device each piece of mail is for\n- which device reads which stream, and when"),
                t("Which device **opens a channel** to which device that runs AI, through which relay or through Droppen Hub itself, and the **sizes and timing** of that traffic, which is not padded: when you work, how much output there is, and the rhythm of typing in a terminal."),
              ],
            }}
            right={{
              title: t("What it never sees"),
              items: [
                t("**A key that can decrypt anything**: there is no server-side master key, no key escrow and no recovery service."),
                t("What is said in a session, typed or shown in a terminal, and the names and contents of your files."),
                t("Session titles, folders, repositories and branches; model names, which AI tool, your AI accounts and their quotas."),
                t("The names of your devices."),
              ],
            }}
          />
          <P>{t("Stored records are **padded to fixed sizes** before they are encrypted and are not compressed, so a stored size says little; live channel traffic is **not padded**.")}</P>
        </Topic>
        <Topic n="1.2" title={t("With no account")}>
          <P>{t("On one device with no account, the local program **talks to no Droppen service at all**: it serves the app itself, and nothing goes through our servers. Everything below is about using Droppen with an account, across devices.")}</P>
        </Topic>
        <Note>{t("Droppen is an invite-only beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.")}</Note>
      </MentionScope>
    </Section>
  );
}

/** The chain as it starts for a new account and as a removal continues it: each entry names the one before by its hash. */
function ChainFigure() {
  const { t } = useTranslation();
  const entries = [
    { kind: "Genesis3", by: t("signed by the account key, then discarded"), what: t("the first full device") },
    { kind: "KeyEpochs", by: t("signed by a full device"), what: t("commits user key epoch 0") },
    { kind: "AddDevice3", by: t("signed by a full device"), what: t("a device that runs AI, its two public keys") },
    { kind: "KeyEpochs", by: t("signed by a full device"), what: t("commits its first content key") },
    { kind: "RemoveDevice3", by: t("signed by a full device"), what: t("a lost phone") },
    { kind: "KeyEpochs", by: t("signed by a full device"), what: t("commits the new epochs, before any is sent") },
  ];
  return (
    <ol data-diagram="security-chain" aria-label={t("A device chain: six entries, each naming the one before by its hash")} className="grid grid-cols-2 border-t border-border md:grid-cols-3 lg:grid-cols-6">
      {entries.map((e, i) => (
        <li
          key={i}
          className={cn(
            "flex min-w-0 flex-col gap-1.5 border-border px-6 py-4 max-md:px-4",
            i % 2 === 1 && "max-md:border-l",
            i > 1 && "max-md:border-t",
            i % 3 > 0 && "md:max-lg:border-l",
            i > 2 && "md:max-lg:border-t",
            i > 0 && "lg:border-l",
          )}
        >
          <span className="font-mono text-2xs leading-4 text-faint tabular-nums">#{i}</span>
          <span className="font-mono text-xs leading-4 font-semibold text-fg [overflow-wrap:anywhere]">{e.kind}</span>
          <span className="text-2xs leading-4 text-muted">{withConcepts(e.what)}</span>
          <span className="text-2xs leading-4 text-muted">{e.by}</span>
          <span className="mt-auto pt-1 font-mono text-[10px] leading-[14px] text-faint">{i === 0 ? t("prev: none") : `prev: H(#${i - 1})`}</span>
        </li>
      ))}
    </ol>
  );
}

function ChainSection() {
  const { t } = useTranslation();
  return (
    <Section id="chain">
      <MentionScope>
        <SectionHead
          label={`02 · ${t("The device chain")}`}
          title={withConcepts(t("One signed list decides who is in your account."))}
          lead={withConcepts(t("The device chain is an append-only, hash-linked log of the account's devices and their public keys. Each entry is signed; every device, and every device that runs AI, checks the whole chain for itself, with the same rules written twice, in Rust and in TypeScript. Droppen Hub stores the chain and passes it on, but cannot write to it."))}
        />
        <ChainFigure />
        <Topic n="2.1" title={t("Entries")}>
          <P>{t("Each entry carries its sequence number, the hash of the entry before it, the time, who signed it and one action. The signature covers that fixed binary structure behind a fixed label, so it can never be read as any other signature in the protocol. Ids are never reused, and every public key must be canonical, on the curve and not of small order.")}</P>
          <Formula block="chain" />
        </Topic>
        <Topic n="2.2" title={t("The genesis and the account key")}>
          <P>{t("The first browser makes the account key AK (Ed25519), signs the genesis entry with it, and **throws the private key away**: it is never stored and signs nothing else. Its fingerprint is the account's **root identity**. Each device that runs AI pins it when it joins; a new browser pins the genesis only once a pairing secret or the recovery words tie it to the real account, so Droppen Hub cannot hand it a chain of its own making. A new account starts on chain v3: the genesis names the first full device, and the app commits the first user key in the next entry.")}</P>
        </Topic>
        <Topic n="2.3" title={t("Chain v3: devices, AI and full devices")}>
          <P>{t("Every device on the chain has two flags: **host** (it runs AI) and **admin** (a full device: it signs entries, holds the user key and vouches for the chain). Every browser and phone you pair now is a full device; one paired earlier as a limited device (it holds no user key and signs nothing) stays limited until a full device makes it full. A device that runs AI is **never a full device**, and both verifiers refuse any entry that would make it one. Entries that change the account are signed by a current, active full device; the account key signs only the genesis, and the recovery key only a recovery join and its activation. A passkey slot, which a full device puts on the chain, signs only a limited device's pending join ([[3.6|t3-6]]) (the protocol also lets a slot marked self-activating activate that device after 24 hours; no app writes such a slot). Accounts made before chain v3 follow the earlier rules until you upgrade them in My devices. The last full device cannot be removed.")}</P>
        </Topic>
        <Topic n="2.4" title={t("Who may control which device")}>
          <P>{t("Each device that runs AI has an **accept list** on the chain (SetAccept). By default **only full devices** may control it; another device that runs AI, or a limited device, only if a full device lists it there. The device that runs AI enforces the list when it admits a channel and on channels already open; Droppen Hub also trims relay tickets by it, as defence in depth (Hub). Narrowing a list requires a new content key for that device in the very next entry.")}</P>
        </Topic>
        <Topic n="2.5" title={t("Forks and rollbacks")}>
          <P>{t("Every verifier keeps the highest sequence number and hash it has seen, and refuses a chain that **goes back or branches**. A channel handshake carries both sides' views and their latest entries, so a removal Droppen Hub holds back reaches the other side at the next connection, and any conflict raises a **fork alarm**: in the app, and on the device that runs AI in its log, its encrypted status and a push.")}</P>
          <P>{t("In a browser, every tab of the account shares one record of the newest head it has seen, written under a Web Lock and only ever moved forward. A tab that sees a conflicting head **latches the alarm**, and from then on attests nothing, opens no channel, signs no entry and seals no key. **The limit**: a record lowered while no tab that holds the newer view is open is trusted at the next load.")}</P>
        </Topic>
        <Topic n="2.6" title={t("Freshness and attestations")}>
          <P>{t("A Hub that withholds a removal could let the removed device keep control. So full devices sign an attestation of the chain head they see when they connect, whenever the chain changes, and every 3 hours while open. A device that runs AI opens a channel only if a full device other than the one asking (or that one, if it is the only full device) attested its current head **within 7 days**; requests over a channel that manage it (handing over a content key, renaming, re-encrypting, changing its settings, AI accounts or software) need such an attestation **within 12 hours**. A content key sent by mail needs none: it is taken only if the chain committed it. A channel already open closes once its last valid attestation is **7 days old**.")}</P>
          <P>{t("An honest Hub revokes a removed device's sessions and relay tickets at once (Hub); the 7 days bound only a Hub that is itself compromised. **The documented limit**: if the Hub also freezes every other full device on that same head, they keep vouching for it, and a removed device keeps control until one of them notices it is frozen; a new channel, which needs an attestation of exactly the current head, has the same limit.")}</P>
          <Formula block="attest" />
        </Topic>
        <Topic n="2.7" title={t("Every change is announced")}>
          <P>{t("Adding or removing a device, a recovery join, an activation, an approval, a change to who may control a device that runs AI or to which devices are full or run AI, a passkey slot added or removed, new recovery words and a key epoch written on its own all raise an alert on every device (a banner to confirm that it was you) and on every device that runs AI (its log, and a push where push or Bark is set up). They read it from their own copy of the chain, so Droppen Hub **can delay it, not hide it**.")}</P>
        </Topic>
        <Topic n="2.8" title={t("Removal and key rotation")}>
          <List
            items={[
              t("A full device **signs the removal**. Droppen Hub closes the removed device's connections and revokes its sessions and relay tickets (Hub); every device that runs AI drops its channels as soon as it sees the entry, and refuses its handshakes from then on."),
              t("Then the app makes **new keys** (the chain records each new epoch but does not demand one):\n- a **new user key** if the removed device ever held one, sealed to the full devices that remain and to the recovery words (the chain voids the words if the removed device knew them: then make new ones)\n- a **new content key** for every device that runs AI, which you may skip only when the removed device never held the user key and no device that runs AI let it control it\nEach new epoch is committed on the chain first, always exactly one up, and sealed only to the devices that remain."),
              t("Optionally, each device that runs AI **re-encrypts its history** under the new key from its own transcripts, and Droppen Hub is asked to delete the old ciphertext."),
              t("What the removed device already read, or can still read from old ciphertext a Hub kept, **cannot be taken back**. A device that could control a device that runs AI may also have copied that device's identity key: identity renewal ([[section 06|renewal]]), in early testing at droppen.app, replaces it in place; removing that device and adding it again also still works."),
            ]}
          />
        </Topic>
      </MentionScope>
    </Section>
  );
}

function KeysSection() {
  const { t } = useTranslation();
  const heads = [t("Key"), t("Kind"), t("Held by"), t("Used for"), t("Replaced")];
  const rows = [
    [t("Device key"), "Ed25519 + X25519", t("Each browser or phone:\n- made by WebCrypto\n- used as a non-extractable key\n- stored sealed under a key of this browser"), t("The device's identity:\n- signs chain entries, attestations and sign-ins\n- its Noise static key\n- receives sealed keys"), t("A new device, a new key")],
    [t("Identity key of a device that runs AI"), t("Ed25519 + X25519 from one random 32-byte seed"), t("That device only, in a file only its user can read"), t("- Signs in to Droppen Hub and the relays and signs its stream heads\n- Its X25519 half is its Noise static key and receives its content keys"), t("When it is renewed in place (in testing) or added again")],
    [t("Account key AK"), "Ed25519", t("Nobody: discarded after the genesis"), t("- Signs the genesis only\n- Its fingerprint is the account's root"), t("Never: starting an account over is not offered yet")],
    [t("User key UK"), t("256-bit symmetric, by epoch"), t("Full devices"), t("Wraps every content key in the account's keyring"), t("When a device that held it is removed")],
    [t("Content key MK"), t("Symmetric, by epoch, one per device that runs AI"), t("- That device\n- Your devices get it wrapped or sealed"), t("Only an input to HKDF: stream names and stream keys"), t("On a removal, when its accept list narrows, and when it starts running AI again")],
    [t("Stream key SK"), t("HKDF of the content key and the stream id"), t("Nobody: derived when needed"), t("Encrypts one stream's records and pages"), t("With the content key")],
    [t("Channel keys"), t("From the Noise handshake's ephemeral keys"), t("Memory only"), t("A live channel, with forward secrecy"), t("- New with each channel\n- Rekeyed by the sender every hour or 2^20 messages; the receiver refuses to go past 2^20 without one")],
    [t("Recovery key RK"), t("128 random bits, shown as 12 words"), t("You only: no device stores it"), t("Derives the recovery signing key, whose X25519 twin opens the user keys sealed to it"), t("When you make new words")],
  ];
  return (
    <Section id="keys">
      <MentionScope>
        <SectionHead
          label={`03 · ${t("Keys")}`}
          title={withConcepts(t("Every key, where it lives, and what it opens."))}
          lead={withConcepts(t("No server holds a key that can decrypt anything, and nothing is held in escrow: if you lose every browser and phone and have no recovery words, the copies on Droppen Hub can never be read again, by us either; only what is still on your devices that run AI remains."))}
        />
        <Table label={t("The keys")} heads={heads} rows={rows} />
        <Topic n="3.1" title={t("Delivering a key")}>
          <P>{t("A key sent to one device is sealed to its X25519 key with **sender authentication**, in the manner of HPKE's Auth mode: the key-encryption key depends on the sender's static key as well as a fresh one, and the context (purpose, account, sender, recipient, epoch) is both an input to the KDF and the AAD. Keys go through Droppen Hub's mailbox; a content key for its own device that runs AI goes straight over the end-to-end channel when that device is online.")}</P>
          <P>{t("A recipient takes a key only if it is addressed to itself, in this account, from a current full device (for the recovery package, any device the chain has named, since the commitment check is what makes it the account's key), and matches the hash the chain committed for that epoch. So neither Droppen Hub nor a removed device can **slip in a key** of its own choosing, or **push an epoch forward**.")}</P>
          <Formula block="seal" />
        </Topic>
        <Topic n="3.2" title={t("Adding a device")}>
          <List
            items={[
              t("A new browser or phone shows a **QR code**, or text to paste, with its public keys and a one-time secret. A device you already have reads it, you **compare fingerprints**, and it signs the addition and delivers the keys under a MAC made with that secret."),
              t("Without a camera, an **8-digit code** instead runs CPace over X25519 through Droppen Hub: **one guess per run**, enforced at both ends, and the code is spent after one try."),
              t("A device that runs AI joins with a **one-time code** from a full device. It proves it knows the code's secret over its own two public keys, and the full device signs exactly those keys into the chain. Droppen Hub sees only values derived from the secret (a lookup token and the proof), never the secret, so it **cannot enroll a device of its own**."),
            ]}
          />
        </Topic>
        <Topic n="3.3" title={t("Keys in the browser")}>
          <P>{t("- The **device key** is used as a non-extractable WebCrypto key; it is stored as PKCS #8 sealed under a non-extractable AES-GCM key of this browser (WebKit cannot store non-extractable X25519 keys), or, if made before that change, as the non-extractable key itself.\n- **User keys** are stored only encrypted, under another such key.\n- **Content keys** are not stored in the clear (a browser from the earliest test builds keeps its old copy only until it has moved it into the keyring, or, on a limited device, into its own sealed store; signing out and removing the browser deletes it in any case).\n- **Offline copies** are encrypted under a third such key and cleared when you sign out.\n- **Compatibility mode** (a browser whose WebCrypto lacks Ed25519 or X25519, once you agree to it there) computes the same keys and protocol in JavaScript (the noble-curves library) and stores them sealed the same way, but while the page is open the private keys are plain bytes in the page's memory rather than keys the browser holds, that curve arithmetic is not hardened against side channels as the browser's own is, and one more library runs in the page; the app shows “compatibility mode”.\nThis keeps keys out of storage in the clear. It does not stop code running on the page itself, which can unseal them just as the page does: see [[the threat model|threats]].")}</P>
        </Topic>
        <Topic n="3.4" title={t("Recovery words")}>
          <List
            items={[
              t("Offered when you create the account, and **optional**: **12 words** that encode 128 random bits (only the BIP-39 encoding, with no passphrase stretching), shown once, three of them typed back. You can make new ones from a full device at any time."),
              t("They derive the **recovery signing key**, and every user key epoch is sealed to that key's X25519 twin in the keyring. A signature of the recovery key over the account key is on the chain, so Droppen Hub cannot put your real recovery key into a chain of its own."),
              t("A device that comes back with the words holds the keys to the history at once, but **joins as pending**: it can neither sign entries nor open a channel, and an honest Droppen Hub does not serve it the stored copies, until a full device approves it or it activates itself with the words, which each verifier that already follows the account accepts only **24 hours** after it first saw the join, by its own clock. Meanwhile every device and every device that runs AI warns you, and any full device can veto by removing it; a full device that holds the activation back signs no entry but the veto and pairs no device, not even one already listed, until you approve or remove it, or its 24 hours are up. A browser, or a device that runs AI, that first follows the account after both the join and the activation (a browser that then joins with the recovery words itself, say) takes them as the account's history. The delay holds back control; against a Hub that is itself compromised, it does not hold back reading."),
              t("If Droppen Hub itself holds the words, it can **order the activation before any veto**: then veto, make new words, and, if you suspect the Hub, add your devices that run AI again on the devices themselves."),
            ]}
          />
        </Topic>
        <Topic n="3.5" title={t("Losing every browser and phone")}>
          <P>{t("With every browser and phone lost and no recovery words, **nobody can decrypt** the copies Droppen Hub stores, us included. You can still sign in, but no device is left to add a new one, and starting the account over is not offered yet. Each device that runs AI **pinned its account key and genesis** and refuses any other: a new genesis, made by you, by someone with your password or mailbox, or by Droppen Hub, reaches it only if you add it again on the device itself.")}</P>
        </Topic>
        <Topic n="3.6" title={t("Passkeys, today")}>
          <P>{t("A passkey **signs you in** to Droppen Hub: it gives the same short sign-in session a password does (Hub): 15 minutes, enough to see the account and its devices, read the chain and register this browser's device key. Everything else needs a **device session**, signed by a device key, and what a device may do is **decided by the chain**: a device the chain does not list gets only ciphertext. Adding or removing a passkey and setting, changing or removing the password take a full device and a fresh passkey or the current password, and each such change shows on your other full devices.")}</P>
          <P>{t("A passkey can also let a new browser **ask to join**: when you add one on a full device it does, unless you untick that (it needs a passkey that supports PRF); the passkey you signed up with does only once you switch it on. That puts a **passkey slot** on the chain: a key derived from the passkey's PRF output, which only that passkey can produce. A new browser that signs in with the passkey derives the same key, checks the slot on the chain, and adds itself as a pending, limited device, which every device announces. It gets no key until a full device pairs with it, by its 8-digit code or QR code, and approves exactly the keys on its screen; then it becomes a full device and receives the user key through the pairing. Any full device can remove it instead. A passkey **holds no encryption key and unlocks none**: a slot only lets a device ask.")}</P>
        </Topic>
        <Note>{t("**Not live**: a **passkey letting a new device in** without a full device's approval.\n**In testing**: **renewing the identity key of a device that runs AI in place** ([[section 06|renewal]]).")}</Note>
      </MentionScope>
    </Section>
  );
}

function ChannelsSection() {
  const { t } = useTranslation();
  return (
    <Section id="channels">
      <MentionScope>
        <SectionHead
          label={`04 · ${t("Channels and relays")}`}
          title={withConcepts(t("Every channel is end to end, whichever way it goes."))}
          lead={withConcepts(t("Your device and a device that runs AI talk over one Noise channel. A Droppen relay, or Droppen Hub where no relay carries the channel, only forwards its frames; when the two ends can reach each other, the same channel can also take a direct path."))}
        />
        <Topic n="4.1" title={t("The handshake")}>
          <P>{t("Noise IK with X25519, AES-256-GCM and SHA-256: WebCrypto in the browser, snow in Rust. The browser starts it with the other side's static key taken from the chain, and the other side admits the browser by its own copy of the chain. The prologue **binds the account and both ids**, so neither a relay nor Droppen Hub can **splice two sessions together**. Keys are renewed every hour or every 2^20 messages, whichever comes first.")}</P>
          <Formula block="noise" />
        </Topic>
        <Topic n="4.2" title={t("The first message asks for nothing")}>
          <P>{t("An IK first message can be replayed, and whoever holds the responder's static key could forge one in any device's name. So it carries only the device's view of the chain and its latest entries; anything that changes state **waits for the first transport message**, which needs the device's own private key. The device that runs AI merges the entries first (a conflict is a fork alarm), then checks that the device is current, on its accept list and fresh, and only then the replay window: **10 minutes back, 60 seconds ahead**. Its reply carries its own view and the entries the device lacks.")}</P>
        </Topic>
        <Topic n="4.3" title={t("Droppen relays")}>
          <P>{t("A relay pairs a device's channel with the other end's on tickets that Droppen Hub signs, valid for 15 minutes; the holder must also sign the relay's own challenge with the ticket's key, so a stolen ticket is useless, and the relay refuses a ticket that runs longer than 15 minutes or was issued more than 5 minutes ahead of its own clock. The relay\n- copies frames **without parsing them**\n- has **no database**\n- holds **no key to your data** (only its own signing key and Droppen Hub's public ticket keys)\n- keeps **nothing about your channels** across a restart\nA relay that misreports who is who only gets a failed handshake. Each account is kept on one relay, with an optional standby, and moves at once when its relay goes away; on it, each account's devices that run AI are kept apart from every other account's. While its link to Droppen Hub is down, a relay keeps sessions already connected for up to 2 hours past their tickets: a new sign-in still needs a valid ticket, the revocations it missed are applied when the link is back, and all it ever sees is ciphertext.")}</P>
          <Pair
            left={{ title: t("A relay sees"), items: [t("Random account and device ids, and both ends' IP addresses."), t("When connections and channels open and close, the sizes and timing of frames, the bytes per account, and the limits its ticket carries, such as whether the free transfer is used up.")] }}
            right={{ title: t("A relay never sees"), items: [t("Your e-mail address, the device chain, mail, the keyring or the stored streams."), t("Any content, and the signalling of a direct path.")] }}
          />
        </Topic>
        <Topic n="4.4" title={t("Direct paths")}>
          <P>{t("When both ends can reach each other, the channel adds a WebRTC DataChannel. Its signalling (offers, candidates, DTLS fingerprints) travels **inside the Noise channel**, and a fresh Noise IK handshake runs over the DataChannel, bound to both DTLS fingerprints and to the relayed channel's handshake, with the same admission rules. It closes with its channel. STUN servers (Cloudflare, Google) **see each end's public address**; there is no TURN. Turn off “Allow direct connections” on a device, and it and your devices that run AI never learn each other's IP address.")}</P>
          <Formula block="direct" />
        </Topic>
      </MentionScope>
    </Section>
  );
}

function OfflineSection() {
  const { t } = useTranslation();
  return (
    <Section id="offline">
      <MentionScope>
        <SectionHead
          label={`05 · ${t("Offline copies")}`}
          title={withConcepts(t("History you can read while the device is off, and only you."))}
          lead={withConcepts(t("A device that runs AI writes its session list, its status and its sessions as encrypted, append-only streams that Droppen Hub stores and cannot read. Your devices read them back and accept a record only once a head signed by that device covers it: each record is authenticated under the stream's key and linked to the one before, and the signed head names the newest link."))}
        />
        <Topic n="5.1" title={t("Records")}>
          <P>{t("Each record is sealed with AES-256-GCM under a stream key derived from the content key. The AAD binds the stream, the sequence number and the epoch, so Droppen Hub **cannot move or reorder records**, and each record's plaintext carries a hash link to the one before. Records are padded to fixed sizes. A stream's name is an HMAC under a key derived from the content key, so it **cannot be matched to a session**.")}</P>
          <Formula block="stream" />
        </Topic>
        <Topic n="5.2" title={t("Signed stream heads")}>
          <P>{t("After every append, and as a heartbeat every 5 minutes, the device that runs AI signs the head of the stream: how many records it holds, the epoch, the link value and the time. Readers release only records a signed head covers.\n- A head beyond the records received means Droppen Hub is **holding some back**.\n- A link that does not match means a record was **forged or replaced**.\n- The newest head's time is **how current the copy is**, shown as “as of”.\nA removed device that still holds an old stream key can seal a record, but not sign a head, so nobody accepts what it writes.")}</P>
          <Formula block="head" />
        </Topic>
        <Topic n="5.3" title={t("Snapshots and rollback")}>
          <P>{t("Older history is compacted into **immutable pages** named by their SHA-256 and listed in a manifest, which the device that runs AI signs once more over a signed head. A reader starts only from a signed head and checks every page against the manifest. A browser keeps its copy together with the signed head it accepted, in one IndexedDB transaction, and checks that signature again when it loads; the local program's reader also keeps the newest head it accepted **as a floor**, and refuses a Droppen Hub that replays the device's own older heads.")}</P>
          <Formula block="pages" />
        </Topic>
        <Topic n="5.4" title={t("When keys change")}>
          <P>{t("Each content key epoch has its **own stream names** once your devices read them so (every current app does): after a rotation the device that runs AI moves its index and status to the new names at once, and its sessions within six hours, in random order — at once when it re-encrypts. With re-encryption the device that runs AI rebuilds its streams from its own transcripts under the new key, and Droppen Hub is asked to delete the old ciphertext. Drafts still being written are sent only to live readers, **never stored**, signed by the device that runs AI and accepted only under the current epoch.")}</P>
        </Topic>
      </MentionScope>
    </Section>
  );
}

function RenewalSection() {
  const { t } = useTranslation();
  return (
    <Section id="renewal">
      <MentionScope>
        <SectionHead
          label={`06 · ${t("Identity renewal")}`}
          title={withConcepts(t("Renewing the identity key of a device that runs AI."))}
          lead={
            <span className="flex flex-col items-start gap-3">
              <Chip>{t("In testing")}</Chip>
              <span>{withConcepts(t("Switched on at droppen.app since 2026-10-23 (Hub), in early testing; Droppen 0.2.127 has the web app's side and that of the device that runs AI. It is still being tested and parts of it may still change. A device that runs AI whose identity key may have been copied can also still be removed and added again."))}</span>
            </span>
          }
        />
        <Note>{t("**A test on droppen.app** on 2026-10-23 (Hub) showed:\n- the window was **2 minutes** ([[6.3|renewal-staging]])\n- as soon as a full device asked Droppen Hub for the renewal, a relay ticket issued earlier to the device that runs AI **in its role as a device** (in which it can control another device that runs AI that lets it in) was **revoked**, on relays that can revoke one role's tickets alone; its ticket for running AI **stayed valid until the entry was stored**\n- an entry written **before the window was over** was refused\n- an **automatic** renewal and one **confirmed with the code on the device** both went through")}</Note>
        <Topic id="renewal-why" n="6.1" title={t("Why renew")}>
          <P>{t("Being allowed to control a device that runs AI means being able to read its files, including the one that holds its identity seed. So when a device that could control it is removed, loses its full rights, or is taken off its accept list, that device **may hold a copy of the identity key**. New content keys **do not help** against that: the copy could still sign in as the device that runs AI, answer handshakes and receive keys sealed to it. Renewal **replaces the identity key in place** and keeps the device's id, its accept list and everything else kept by id. It runs for every device that runs AI the lost device could reach, directly or through other devices that run AI that let it in.")}</P>
        </Topic>
        <Topic id="renewal-entry" n="6.2" title={t("The signed RotateHostKey entry")}>
          <P>{t("A renewal is a chain v3 entry of kind 20, RotateHostKey, signed by a full device and followed at once by that device's next content key commitment. **Both the old and the new identity key** sign one proof: the account, the device, the chain position it was made against (a sequence number and that entry's hash), all four public keys, a nonce and the assurance level. The entry carries the new keys, that position, the nonce, the assurance and both signatures; each verifier rebuilds the proof with the account and the old keys from its own chain. So neither a stolen full device alone nor whoever holds the old key alone can move the id to another key.")}</P>
          <P>{t("Both verifiers check\n- that the device is a **current device that runs AI**\n- that **neither new key** is one any device on the chain holds or ever held\n- that the **hash** matches the chain at that position\n- **both signatures**\nRetired keys stay on the chain and can never join or be activated again. Droppen Hub stores an entry of kind 20 or 21, or one that starts a pause (kind 22; both below), only while renewal is switched on and every device that runs AI on the account runs a Droppen that can renew; otherwise it **refuses to store** them.")}</P>
          <Formula block="rotate" />
        </Topic>
        <Topic id="renewal-staging" n="6.3" title={t("Staging on the real device, and confirming it")}>
          <List
            items={[
              t("A full device asks the device that runs AI over the end-to-end channel, from a view vouched for within 12 hours, to **make a new seed**. The device keeps it beside the current seed, never over it, and answers with the new keys, both signatures and a MAC that proves it holds the new X25519 key."),
              t("The device that runs AI hands the proof to Droppen Hub **itself, over its own connection** signed in with the old key. A copy that answered through a relay would have to sign in as the device too, pushing the real one off."),
              t("For a window (2 minutes by default), every sign-in with the old key **must confirm with the new key within 20 seconds**; any sign-in that does not voids the staging (Hub). Only then does the full device write the entry. The device that runs AI switches to the new seed only after it has verified the entry on its own chain, so a lying Hub cannot make it give up its key."),
              t("The next content key is **sealed only to the new X25519 key**, and readers accept stream heads from the network only under the current key."),
            ]}
          />
          <Pair
            left={{
              title: t("Automatic"),
              items: [
                t("**Stops a copy that is not in use** at that moment: the common case of a lost phone, an old browser, or a script that copied a file."),
                t("**Cannot tell** the real device from a copy that is online while the real one is off. With an honest Droppen Hub that case is not silent: when the real device comes back, it reads the renewal on its own chain, raises its alarm and disputes it. For a hostile Hub, see [[6.7|renewal-hub]]."),
              ],
            }}
            right={{
              title: t("Confirmed on the device"),
              items: [
                t("The device that runs AI shows an **8-character code** for 10 minutes, in its own settings or in droppen agent status, and you type it on your full device, which works out the same code from the answer it received."),
                t("A copy **cannot put its code on the real device's screen**, so the renewal can only go to the real device."),
              ],
            }}
          />
          <Formula block="confirm" />
        </Topic>
        <Topic id="renewal-policy" n="6.4" title={t("“Always confirm on the device”")}>
          <P>{t("An account-wide setting, written on the chain by a full device as an entry of kind 21 (SetRenewalPolicy). While it is on, both verifiers refuse **any renewal that was not confirmed on the device**, so a colluding Hub cannot switch it off for you; turning it off is an alert on every device. Your devices start no renewal, and Droppen Hub opens none, until a full device has chosen this setting once. Each browser can also refuse, for itself, to sign any renewal not confirmed on the device.")}</P>
        </Topic>
        <Topic id="renewal-disputes" n="6.5" title={t("Disputes and isolation")}>
          <List
            items={[
              t("If the real device that runs AI sees, on a chain it has verified, its id renewed to keys it never made, it\n- **raises an alarm** (in its status and by push)\n- **stops**\n- **keeps its old seed**\n- **signs a dispute** with that now-retired key"),
              t("The dispute names the renewal's sequence number, its entry hash and the key it installed, so it **can only exist after that entry**, with no clocks compared. Droppen Hub can withhold a dispute but **cannot forge one**."),
              t("Each of your browsers and phones that verifies it **isolates the device that runs AI**: it opens no channel to it, seals it no keys, and renews it only with the code. Other devices that run AI keep following their accept lists until it is renewed with the code or removed. Each browser keeps the disputes it has verified, so the Hub cannot undo them later. A page counts only those it has verified itself, except that one another page of the browser verified, about a renewal this page has not loaded yet, isolates the device until this page catches up and checks it. Only a later renewal confirmed on the device, or removing the device, ends the isolation."),
              t("A key retired by a confirmed renewal **never isolates**, so a stolen old key can force at most one confirmation. If the real device holds only the retired key, it **cannot recover in place**: remove it, and add it again with a new identity."),
            ]}
          />
          <Formula block="dispute" />
        </Topic>
        <Topic id="renewal-groups" n="6.6" title={t("Exposed groups: controllers first, and a pause")}>
          <P>{t("If a lost device could control A, and A may control B, a copy of A's key reaches B as well: **B is exposed too**. **Controllers renew first**: when B lets A in, A renews before B, so by the time B renews, A's old key is already useless.")}</P>
          <P>{t("Only devices that can reach one another in a cycle cannot be put in order. For those, a full device first **writes a pause**, an entry of kind 22 (RenewalPause), that names each control edge it removes. Every verifier, and every device that runs AI on every path (Hub, relay, direct), computes access as its accept list minus the edges of every open pause, and closes channels that lose it; the accept lists themselves are never rewritten. Once the devices in the cycle have renewed, an entry marks the pause completed and the accept lists apply in full again. An edge with an end still exposed is carried into a new pause in the same batch. A pause **only ever takes access away**, so one left open is safe and shows on every device. You can end it by hand (a cancellation entry) after confirming the control it gives back, once none of those devices is still exposed. Any full device may end it, not only the one that started it.")}</P>
        </Topic>
        <Topic id="renewal-hub" n="6.7" title={t("What a hostile Hub can and cannot do here")}>
          <Pair
            left={{
              title: t("It cannot"),
              items: [
                t("**Renew an identity** without a full device's signature and the signatures of both identity keys."),
                t("**Replay an old proof**, or **install a key** that a device on the chain holds or once held."),
                t("Make a device that runs AI **switch to, or give up, a key** without an entry it has verified itself."),
                t("**Switch off “always confirm on the device”**, **forge a dispute**, or **undo an isolation a device has verified**."),
              ],
            }}
            right={{
              title: t("It can"),
              items: [
                t("**Withhold a renewal**, and every pause, policy change and dispute, from one device for a while, as it can a removal: bounded by freshness, and the chain also travels inside handshakes."),
                t("**Withhold disputes**, so that a device that never receives one does not isolate."),
                t("Together with someone holding a copy of the old key, **let that copy win an automatic renewal**, even while the real device is online (only an honest Hub holds the window), and hide the evidence. **Only confirming on the device** prevents this. What remains is on the real device: it can no longer sign in, and it raises its alarm once it receives the renewal, which such a Hub can also hold back."),
              ],
            }}
          />
          <P>{t("Renewal protects what comes after it. It **cannot take back** what a copy already had:\n- that device's transcripts and files\n- credentials stored on it such as AI and SSH logins\n- earlier content keys and the ciphertext under them\nNor can it **remove a backdoor** left on the device, which needs checking or reinstalling. Droppen Hub sees which device renewed and when.")}</P>
        </Topic>
      </MentionScope>
    </Section>
  );
}

function ThreatsSection() {
  const { t } = useTranslation();
  const heads = [t("Who"), t("Can"), t("Cannot")];
  const rows = [
    [
      t("Droppen Hub, or whoever runs or compels it"),
      t("- **See the metadata in [[section 01|overview]]**\n- **Serve a modified page** to a browser that uses my.droppen.app (last row; [[7.1|t7-1]])\n- **Carry a channel's ciphertext** where no relay does\n- **Refuse service**\n- **Delete data**\n- **Delay the newest records or chain entries**\n- **Analyse traffic**"),
      t("Through the protocol alone:\n- **decrypt content**\n- **forge commands** (only devices on the chain are admitted, and the first message asks for nothing)\n- **add a device of its own**\n- **slip in keys** (they are sender-authenticated and committed on the chain)\n- **forge or quietly change records** (signed heads)\n- **take over a device that runs AI with a new genesis** (it pinned the account key)"),
    ],
    [t("A Droppen relay"), t("- See **ids, IP addresses, and the sizes and timing** of the channels it carries\n- **Drop them**"), t("- **Read or change them**\n- **Splice two sessions together** (the prologue binds both ids)")],
    [t("The network, or a STUN server"), t("See **IP addresses, and the sizes and timing** of traffic."), t("- **Read signalling or content**\n- **Sit in the middle of a direct path** (both DTLS fingerprints are bound into its handshake)")],
    [t("A stolen phone or browser"), t("Until you remove it, a full browser or phone can:\n- **read all history** (its user key opens every content key in the keyring)\n- **control your devices that run AI**\n- **sign chain entries** such as adding a device of its own (which every device announces) or removing your other devices\nA limited one reads and controls only the devices that let it in."), t("After removal:\n- **sign entries**\n- **open channels**\n- **read anything sealed under the new keys**\n- **forge records with old stream keys**\n(against a Hub that withholds the removal, see [[2.6|t2-6]])")],
    [
      t("A stolen device that runs AI, or a program running as its user"),
      t("**Anything you can do on that device**, including reading its identity seed and acting as it, and **control the devices that run AI that let it in**, until it is removed or added again."),
      t("- **Read streams of devices that never let it control them**\n- **Sign chain entries or hold the user key** (it is never a full device)"),
    ],
    [t("A stolen password or passkey"), t("- **Use the service and see the account and its devices**\n- **Register devices of its own**, which cannot join without a full device\n- With a passkey that may let devices join, **add one as a pending device**, which every device announces"), t("**Read or delete data**, or **take over a device**: a sign-in session sees only the account, its devices and the chain, and a device the chain does not list as active gets only ciphertext.")],
    [
      t("Stolen recovery words and password"),
      t("**Decrypt all history at once**."),
      t("**Control a device that runs AI or sign entries** for **24 hours** unless a full device approves; meanwhile every device warns, and any full device can veto."),
    ],
    [
      t("A modified web page"),
      t("Served to you once, do anything this browser can:\n- **read all history** (its user key opens every content key in the keyring)\n- **control your devices that run AI**\n- **sign chain entries** such as adding a device of its own (which every device announces, [[2.7|t2-7]])\n- since code on the same origin can unseal what the page stores, **export this browser's private keys and user keys**"),
      t("**Change the code of the local web app or the desktop app**, which load no code from Droppen Hub."),
    ],
  ];
  return (
    <Section id="threats">
      <MentionScope>
        <SectionHead
          label={`07 · ${t("Threat model")}`}
          title={withConcepts(t("Who could attack, and what each of them can do."))}
          lead={withConcepts(t("The design assumes Droppen Hub may be malicious, or compelled. Here is what each position can and cannot do, as the protocol stands today."))}
        />
        <Table label={t("Threat model")} heads={heads} rows={rows} />
        <Topic n="7.1" title={t("The web-delivery gap")}>
          <P>{t("The app at **my.droppen.app** comes from Droppen Hub, so end-to-end encryption protects your data from the Hub, but **not from a page the Hub has changed**: such a page can do anything this browser can. WebCrypto's non-extractable keys **do not stop code on the same origin**: such code can unseal the stored device key and user keys just as the real page does, and send them anywhere.")}</P>
          <P>{t("**www.droppen.app** shows this page and the other public ones: the same attested build, which never reads or stores a key there. droppen.app is the Hub's own address, where devices that run AI and relays connect. A browser set up at droppen.app before the app moved to my.droppen.app either moves its keys there once, from one page to the other inside the browser, or is added again as a new device; either way they never pass through the Hub.")}</P>
          <List
            items={[
              t("Each release carries **web-manifest.json**, the SHA-256 of every file of its web app, and since v0.2.127 a **signed attestation** of it, both made by the release workflow ([[.github/workflows/release.yml]]); the page's Content-Security-Policy allows only its own scripts. A script, [[scripts/verify-web.mjs]], compares what my.droppen.app and www.droppen.app serve with the latest release's manifest by default, and fails a page served without a sufficient Content-Security-Policy ([[docs/web-integrity.md]] says how)."),
              t("A check **cannot see a page served once, to one person**, nor clean a browser that already cached a bad one."),
              t("For high assurance, use the **desktop app**, signed with Apple's Developer ID and notarised, or the **local web app** that the droppen program serves from its own binary: neither loads code from Droppen Hub."),
            ]}
          />
        </Topic>
        <Topic n="7.2" title={t("Limits we know of")}>
          <List
            items={[
              t("**Metadata and traffic analysis**: padding hides a stored record's size within its bucket; it hides neither when you work nor, on a live channel, how much output there is or the rhythm of your typing."),
              t("**A Hub that withholds a removal** can keep a removed device in control for up to 7 days, and longer if it also freezes every other full device on the same head until one of them notices ([[section 2.6|t2-6]])."),
              t("On a device that runs AI, the **identity seed is a file** that any program running as its user can read. Keeping it in the system keychain comes later."),
              t("The pairing code's **Elligator 2 map is not constant-time**; the code is single-use and spent after one try."),
              t("**Notifications sent through Bark**, if you set it up, go to Bark's server unencrypted."),
              t("**Internal security reviews only**; no external audit yet."),
            ]}
          />
        </Topic>
      </MentionScope>
    </Section>
  );
}

function VerifySection() {
  const { t } = useTranslation();
  const mail = (
    <a href={`mailto:${SECURITY_EMAIL}`} data-testid="security-email" className="text-fg underline underline-offset-4 hover:no-underline">
      {SECURITY_EMAIL}
    </a>
  );
  const choices = [
    [t("Signatures"), t("Ed25519, verified strictly, always behind a fixed label")],
    [t("Key agreement"), t("X25519; small-order points refused")],
    [t("Encryption"), "AES-256-GCM"],
    [t("Hash and KDF"), "SHA-256, HKDF-SHA256, HMAC-SHA256"],
    [t("Channel"), "Noise_IK_25519_AESGCM_SHA256"],
    [t("Pairing code"), t("CPace over X25519, with SHA-512")],
    [t("Recovery words"), t("BIP-39 encoding, 12 words")],
  ];
  return (
    <Section id="verify">
      <MentionScope>
        <SectionHead
          label={`08 · ${t("Verify it yourself")}`}
          title={withConcepts(t("Don't take our word for it."))}
          lead={withConcepts(t("Statements about the protocol, the apps and the local program come from this repository's code and design notes; Droppen Hub is closed, and lines that rest on its own behaviour or policy, or on test results on droppen.app, are marked (Hub). Here is where to look."))}
        />
        <Topic n="8.1" title={t("Algorithms")}>
          <dl className="grid max-w-[46em] grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-6 gap-y-1.5">
            {choices.map(([name, value]) => (
              <div key={name} className="contents">
                <dt className="text-xs leading-5 font-semibold text-fg">{name}</dt>
                <dd className="font-mono text-2xs leading-5 text-muted [overflow-wrap:anywhere]">{value}</dd>
              </div>
            ))}
          </dl>
        </Topic>
        <Topic n="8.2" title={t("Labels")}>
          <P>{t("Every Ed25519 signature in the protocol covers a fixed binary structure behind a fixed label that ends in a NUL byte, so no label is a prefix of another; **no key ever signs bytes Droppen Hub chose**, and its sign-in challenge sits inside a structure of our own. The signature labels:")}</P>
          <Formula block="labels" />
          <P>{t("Renewal adds one more, {{label}} ([[section 06|renewal]]).", { label: "rh/v1 rotate-dispute" })}</P>
        </Topic>
        <Topic n="8.3" title={t("The code")}>
          <P>{t("The protocol is written twice, in Rust and in TypeScript, and each must read the other's test vectors. The app, the local program, the desktop app and the relay program are licensed under the **AGPL-3.0**, and the protocol and client libraries ({{crates}}) under **MIT or Apache-2.0**; their source code is at [[github.com/getdroppen/droppen|/]]. Droppen Hub is not open source.", { crates: "rh-proto, rh-relay, rh-client" })}</P>
          <Spec>
            <Line>
              <RepoLink path="server-rs/crates/rh-proto" /> (Rust) · <RepoLink path="web/src/lib/e2e" /> (TypeScript)
            </Line>
            <Line>
              <RepoLink path="server-rs/crates/rh-relay" /> · <RepoLink path="server-rs/crates/droppen-relay" /> · <RepoLink path="server-rs/crates/rh-client" />
            </Line>
          </Spec>
        </Topic>
        <Topic n="8.4" title={t("Shared test vectors")}>
          <P>{t("Each implementation writes test vectors ([[vectors/rust.json|server-rs/crates/rh-proto/vectors/rust.json]], [[vectors/ts.json|server-rs/crates/rh-proto/vectors/ts.json]]) and **must read the other's**:\n- Noise transcripts through rekeys\n- chains in which every refusal gives the same error\n- key wraps\n- records and pages\nThe direct-path handshake is checked one way, from Rust to TypeScript. Attacks found in our reviews stay as regression tests on both sides.")}</P>
        </Topic>
        <Topic n="8.5" title={t("The page you are reading")}>
          <P>{t("node [[scripts/verify-web.mjs]] checks what my.droppen.app and www.droppen.app serve against the latest release's manifest by default (an older one only when you ask), and fails a page without a sufficient Content-Security-Policy; /.well-known/droppen-build says which build each is. Releases made since the source went public carry a **signed attestation** of their manifest; Droppen Hub may also run a build that is not a release, which only a build of that commit can match ([[docs/web-integrity.md]]). [[scripts/install.sh]] checks a release's checksums (SHA256SUMS-server) against a signature by the release key it pins before it installs anything; it goes without one only if you set DROPPEN_ALLOW_UNSIGNED=1 (meant for a release from before the checksums were signed, or a system without openssl), and a signature that does not check out is never accepted.")}</P>
        </Topic>
        <Topic n="8.6" title={t("Report a vulnerability")}>
          <P>
            {linked(t("Please report privately, not in a public issue: use GitHub's private vulnerability reporting at [[github.com/getdroppen/droppen|/]] (its Security tab), or write to"))} {mail}
          </P>
          <P>{t("The design assumes a malicious Hub: a way for Droppen Hub, a relay or the network to read or change what should be end-to-end encrypted, or for a device not on your account to act on your account or your devices, **is a vulnerability**. When testing droppen.app, **use only accounts you own, and no load testing**.")}</P>
        </Topic>
      </MentionScope>
    </Section>
  );
}

function useDocumentMeta() {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("How Droppen's encryption works — Droppen");
    document.documentElement.lang = active.language;
    const meta = document.querySelector('meta[name="description"]');
    const description = meta?.getAttribute("content") ?? null;
    meta?.setAttribute("content", t("The keys, the signed device chain, the channels, the offline copies and the threat model of Droppen's end-to-end encryption, and how to check them."));
    return () => {
      document.title = before;
      if (meta && description !== null) meta.setAttribute("content", description);
    };
  }, [t, active.language]);
}

/** droppen.app/security, signed in or not. */
export default function SecurityPage({ signedIn = false }: { signedIn?: boolean }) {
  useDocumentMeta();
  // Arrived at a part of the page (`/security#renewal`): the page was not there to scroll to yet.
  useEffect(() => {
    const id = location.hash.slice(1);
    if (id) requestAnimationFrame(() => document.getElementById(id)?.scrollIntoView());
  }, []);
  return (
    <div data-testid="security" className="h-full w-full overflow-y-auto bg-bg [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1152px] md:border-x md:border-border">
          <FormulaProvider>
            <Hero />
            <Definitions />
            <Contents />
            <OverviewSection />
            <ChainSection />
            <KeysSection />
            <ChannelsSection />
            <OfflineSection />
            <RenewalSection />
            <ThreatsSection />
            <VerifySection />
          </FormulaProvider>
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
