import { Fragment, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode, createContext, useContext, useEffect, useId, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { Check, Minus, Pin } from "lucide-react";
import { cn } from "@/lib/cn";
import { BLOCKS, type BlockId, type FormulaBlock, type Kind, KINDS, type Lang, LANGS, PARTIES, type Party, type Roles, TERMS, type Words } from "./formula-data";
import { type Analysis, analyze, flowOf, type Lit, lights } from "./formula-model";

/*
 * The formulas of droppen.app/security, explorable (data: `formula-data.ts`,
 * tokens and flow: `formula-model.ts`).
 *
 * Every part of a formula is a focusable term, drawn by its kind (colour, and
 * weight and underline for whoever cannot tell the colours apart). Pointing
 * at one, or focusing it from the keyboard, lights the same value in every
 * formula on the page, inks where it is defined, outlines the calls that make
 * it and take it, and shows a short card: what it is and why it is there,
 * where it comes from and goes, who can know it. A click, a tap or Enter pins
 * the card, so two can be read side by side; Esc or a click elsewhere
 * unpins. Above each block, "view as" hides every value that party cannot
 * know. Screen readers get the same through `aria-describedby` (the hidden
 * glossary). Nothing moves: highlights are backgrounds and outlines, the
 * cards float in a portal, and the formula's text, selected and copied, is
 * the formula.
 */

const ANALYSIS: Analysis = analyze();

const RANK: Record<Lit, number> = { def: 3, same: 2, flow: 1, "": 0 };

/** Who is pointing where, what is pinned, and whose view: the page's one state, read token by token. */
class FormulaStore {
  hover: string | null = null;
  focus: string | null = null;
  pin: string | null = null;
  /** The occurrence whose passing card was dismissed (Esc, or unpinned by a click) until the pointer or focus moves on. */
  dismissed: string | null = null;
  view: Party | null = null;
  /** Whether the last input was a pointer: a term focused by a click or a tap shows no passing card (the click pins it). */
  pointing = false;
  private lit = new Map<string, Lit>();
  private readonly listeners = new Set<() => void>();

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  /** The card that is showing because of the pointer or the keyboard, not a pin. */
  passing(): string | null {
    const at = this.hover ?? this.focus;
    return at && at !== this.dismissed ? at : null;
  }

  update(patch: Partial<Pick<FormulaStore, "hover" | "focus" | "pin" | "dismissed" | "view">>) {
    Object.assign(this, patch);
    const lit = new Map<string, Lit>();
    for (const active of new Set([this.pin, this.passing()])) {
      if (!active) continue;
      for (const [key, value] of lights(ANALYSIS, active)) if (RANK[value] > RANK[lit.get(key) ?? ""]) lit.set(key, value);
    }
    this.lit = lit;
    for (const listener of this.listeners) listener();
  }

  litOf(occ: string): Lit {
    return this.lit.get(occ) ?? "";
  }

  /** Whether one of `concept`'s words on the page is pointed at, focused or pinned: then all of them light. */
  conceptLit(concept: ConceptId): boolean {
    return [this.pin, this.passing()].some((occ) => occ?.startsWith(`c:${concept}:`));
  }

  togglePin(occ: string) {
    if (this.pin === occ) this.update({ pin: null, dismissed: occ });
    else this.update({ pin: occ, dismissed: null });
  }
}

const StoreContext = createContext<FormulaStore | null>(null);

function useStore(): FormulaStore {
  const store = useContext(StoreContext);
  if (!store) throw new Error("a formula outside <FormulaProvider>");
  return store;
}

/** The page's language as the formulas' words know it. */
function useLang(): Lang {
  const { i18n } = useTranslation();
  const lng = i18n.language ?? "en";
  if (lng.startsWith("zh")) return /TW|HK|MO|Hant/i.test(lng) ? "zh-TW" : "zh-CN";
  return (LANGS as readonly string[]).includes(lng) ? (lng as Lang) : lng.startsWith("ja") ? "ja" : "en";
}

const say = (words: Words, lang: Lang) => words[LANGS.indexOf(lang)] || words[0];

/** Each kind's look: colour, and weight and underline so that colour is never the only cue. */
export const KIND_CLASS: Record<Kind, string> = {
  secret: "text-key-secret font-semibold underline decoration-solid",
  derived: "text-key-derived font-semibold underline decoration-double",
  public: "text-key-public underline decoration-dashed",
  value: "text-fg underline decoration-dotted",
  label: "text-muted underline decoration-dotted",
  fn: "text-fg font-semibold underline decoration-dotted",
  op: "text-muted underline decoration-dotted",
};

export const LIT_CLASS: Record<Lit, string> = {
  def: "bg-fg text-bg! decoration-bg!",
  same: "bg-fg/15 outline outline-1 outline-fg/50",
  flow: "outline outline-1 outline-dashed outline-fg/70",
  "": "",
};

/** Hatched in place of a value the chosen party cannot know: the text stays, for copying and for screen readers. */
export const HIDDEN_CLASS = "text-transparent! decoration-transparent! [background-image:repeating-linear-gradient(135deg,var(--color-border-strong)_0_1.5px,transparent_1.5px_4px)]";

function useKindName(): Record<Kind, string> {
  const { t } = useTranslation();
  return {
    secret: t("Secret key"),
    derived: t("Derived secret"),
    public: t("Public key"),
    value: t("Public value"),
    label: t("Label or constant"),
    fn: t("Function"),
    op: t("Operator"),
  };
}

function usePartyName(): Record<Party, string> {
  const { t } = useTranslation();
  return { me: t("Your device"), other: t("The other device"), hub: "Droppen Hub", relay: t("Relay") };
}

const descId = (term: string) => `fx-${term.replace(/[^\w-]/g, "-")}`;
const hiddenId = (party: Party) => `fx-hidden-${party}`;
const HINT_ID = "fx-hint";

/** Who "your device" and "the other device" are in `block`; a block of labels has no parties. */
const rolesOf = (block: BlockId): Roles | undefined => (BLOCKS[block] as FormulaBlock).roles;

/** Whether `term`, in a block with parties, is a value `view` cannot know. */
function hiddenFrom(block: BlockId, term: string, view: Party | null): boolean {
  return !!view && !!rolesOf(block) && !TERMS[term]!.known.includes(view);
}

/** Pointing shows a card, focusing from the keyboard too; a click, a tap, Enter or Space pins it. */
function interactions(store: FormulaStore, occ: string) {
  return {
    onPointerEnter: (e: ReactPointerEvent) => {
      if (e.pointerType === "mouse") store.update({ hover: occ, dismissed: null });
    },
    onPointerLeave: (e: ReactPointerEvent) => {
      if (e.pointerType === "mouse" && store.hover === occ) store.update({ hover: null });
    },
    onFocus: () => {
      if (!store.pointing) store.update({ focus: occ, dismissed: null });
    },
    onBlur: () => {
      if (store.focus === occ) store.update({ focus: null });
    },
    onClick: () => store.togglePin(occ),
    onKeyDown: (e: ReactKeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        store.togglePin(occ);
      }
    },
  };
}

function Term({ block, occ, text, tabbable }: { block: BlockId; occ: string; text: string; tabbable: boolean }) {
  const store = useStore();
  const term = ANALYSIS.occurrences.get(occ)!.term;
  const snapshot = useSyncExternalStore(
    store.subscribe,
    () => `${store.litOf(occ)}|${hiddenFrom(block, term, store.view) ? store.view : ""}|${store.pin === occ ? 1 : 0}`,
    () => "||0",
  );
  const [lit, hidden, pinned] = snapshot.split("|") as [Lit, Party | "", string];
  const kind = TERMS[term]!.kind;
  return (
    <span
      role="button"
      tabIndex={tabbable ? 0 : -1}
      data-occ={occ}
      data-term={term}
      data-kind={kind}
      data-lit={lit || undefined}
      data-hidden-from={hidden || undefined}
      aria-pressed={pinned === "1"}
      aria-describedby={hidden ? `${descId(term)} ${hiddenId(hidden)}` : descId(term)}
      className={cn(
        "cursor-help decoration-1 underline-offset-[3px] outline-offset-1 transition-[background-color,color,outline-color] duration-100 motion-reduce:transition-none",
        KIND_CLASS[kind],
        LIT_CLASS[lit],
        hidden && HIDDEN_CLASS,
        pinned === "1" && "outline-2 outline-solid outline-fg",
      )}
      {...interactions(store, occ)}
    >
      {text}
    </span>
  );
}

/*
 * The page's two words for the two kinds of device — a device that runs AI
 * (the chain's host) and your browsers and phones (its full devices) — are
 * defined once at the top (`Definitions` in security.tsx) and explain
 * themselves wherever the page says them, with the same cards.
 */

export const CONCEPTS = ["host", "client"] as const;
export type ConceptId = (typeof CONCEPTS)[number];

/**
 * The two words as the page spells them in each language: a device (or
 * devices) that run(s) AI, and browsers and phones (or a browser or phone).
 */
export const CONCEPT_WORDS =
  /(\b[Dd]evices? that runs? AI\b|可运行 AI 的设备|可執行 AI 的裝置|AI を実行できる(?:あなたの|どの|各|すべての|別の|ほかの)?デバイス)|(\b[Bb]rowsers? (?:and|or) phones?\b|\b[Pp]hones? or browsers?\b|浏览器(?:和|或|、)手机|手机或浏览器|瀏覽器(?:和|或|、)手機|手機或瀏覽器|ブラウザ(?:や|と|、|・)スマホ|スマホ(?:や|と)ブラウザ)/g;

/** `text` with each of the two words made explorable. */
export function withConcepts(text: string): ReactNode {
  const out: ReactNode[] = [];
  let at = 0;
  for (const m of text.matchAll(new RegExp(CONCEPT_WORDS))) {
    const index = m.index!;
    if (index > at) out.push(text.slice(at, index));
    out.push(
      <Concept key={index} id={m[1] ? "host" : "client"}>
        {m[0]}
      </Concept>,
    );
    at = index + m[0].length;
  }
  if (out.length === 0) return text;
  if (at < text.length) out.push(text.slice(at));
  return <Fragment>{out}</Fragment>;
}

function useConceptWords(): Record<ConceptId, { name: string; says: string }> {
  const { t } = useTranslation();
  return {
    host: {
      name: t("A device that runs AI"),
      says: t("A device running the droppen program or the desktop app, where your AI sessions run. It holds its identity key and its content keys, and it is never a full device."),
    },
    client: {
      name: t("Your browsers and phones"),
      says: t("Where you watch and control from: droppen.app, in a browser or on a phone. Each one you pair now is a full device: it holds the user key and signs the account's chain entries."),
    },
  };
}

const conceptDescId = (concept: ConceptId) => `fx-c-${concept}`;

/**
 * A part of the page (a section) in which the two words are one tab stop
 * each: the first mention of each is reached with Tab, and the arrow keys
 * move from mention to mention inside the part.
 */
interface MentionPart {
  first: Map<ConceptId, string>;
}

const MentionContext = createContext<MentionPart | null>(null);

export function MentionScope({ children }: { children: ReactNode }) {
  const [part] = useState<MentionPart>(() => ({ first: new Map() }));
  return (
    <MentionContext.Provider value={part}>
      <div
        className="contents"
        onKeyDown={(e) => {
          const target = e.target as HTMLElement;
          if (!target.dataset.concept) return;
          const mentions = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-concept]")];
          const at = mentions.indexOf(target);
          const next =
            e.key === "ArrowRight" || e.key === "ArrowDown"
              ? mentions[at + 1]
              : e.key === "ArrowLeft" || e.key === "ArrowUp"
                ? mentions[at - 1]
                : e.key === "Home"
                  ? mentions[0]
                  : e.key === "End"
                    ? mentions[mentions.length - 1]
                    : undefined;
          if (!next) return;
          e.preventDefault();
          next.focus();
        }}
      >
        {children}
      </div>
    </MentionContext.Provider>
  );
}

function Concept({ id, children }: { id: ConceptId; children: string }) {
  const store = useStore();
  const occ = `c:${id}:${useId()}`;
  const part = useContext(MentionContext);
  // The first mention of the word in its part is the tab stop (decided once, in render order: the same on the server).
  const [stop] = useState(() => {
    if (!part) return true;
    if (!part.first.has(id)) part.first.set(id, occ);
    return part.first.get(id) === occ;
  });
  const snapshot = useSyncExternalStore(
    store.subscribe,
    () => `${store.conceptLit(id) ? 1 : 0}|${store.pin === occ ? 1 : 0}`,
    () => "0|0",
  );
  const [lit, pinned] = snapshot.split("|");
  return (
    <span
      role="button"
      tabIndex={stop ? 0 : -1}
      data-occ={occ}
      data-concept={id}
      data-lit={lit === "1" ? "same" : undefined}
      aria-pressed={pinned === "1"}
      aria-describedby={conceptDescId(id)}
      className={cn(
        "cursor-help underline decoration-dotted decoration-1 underline-offset-4 outline-offset-1 transition-[background-color,outline-color] duration-100 motion-reduce:transition-none",
        lit === "1" && "bg-fg/10",
        pinned === "1" && "outline-2 outline-solid outline-fg",
      )}
      {...interactions(store, occ)}
    >
      {children}
    </span>
  );
}

function ConceptCard({ concept, pinned }: { concept: ConceptId; pinned: boolean }) {
  const { t } = useTranslation();
  const words = useConceptWords()[concept];
  return (
    <>
      <div className="flex items-baseline gap-2">
        <span className="text-xs leading-[18px] font-semibold text-fg">{words.name}</span>
        <span className="label-caps text-[10px] leading-[14px] text-muted">{t("Term")}</span>
        {pinned && <Pin className="ml-auto size-3 shrink-0 self-center text-muted" />}
      </div>
      <p className="text-xs leading-[18px] text-fg">{words.says}</p>
    </>
  );
}

/** "View as", with this block's roles, and the legend. */
function Tools({ block, legendId, showLegend, onLegend }: { block: BlockId; legendId: string; showLegend: boolean; onLegend: () => void }) {
  const { t } = useTranslation();
  const store = useStore();
  const lang = useLang();
  const names = usePartyName();
  const view = useSyncExternalStore(store.subscribe, () => store.view, () => null);
  const labelId = useId();
  const roles = rolesOf(block);
  const options: (Party | null)[] = [null, ...PARTIES];
  const hiddenCount = view
    ? ANALYSIS.lines[block].flat().filter((tok) => tok.occ && hiddenFrom(block, tok.term!, view)).length
    : 0;
  const choose = (party: Party | null) => store.update({ view: party });
  const role = view === "me" ? roles?.me : view === "other" ? roles?.other : undefined;
  return (
    <div data-formula-tools="" className="flex flex-wrap items-center gap-x-3 gap-y-1.5 font-sans">
      {roles && (
        <>
          <span id={labelId} className="label-caps text-[10px] leading-[14px] text-faint">
            {t("View as")}
          </span>
          <div
            role="radiogroup"
            aria-labelledby={labelId}
            className="flex flex-wrap"
            onKeyDown={(e) => {
              const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
              if (!step) return;
              e.preventDefault();
              const next = options[(options.indexOf(view) + step + options.length) % options.length]!;
              choose(next);
              (e.currentTarget.querySelector(`[data-party="${next ?? "all"}"]`) as HTMLElement | null)?.focus();
            }}
          >
            {options.map((party) => (
              <button
                key={party ?? "all"}
                type="button"
                role="radio"
                data-party={party ?? "all"}
                aria-checked={view === party}
                tabIndex={view === party ? 0 : -1}
                onClick={() => choose(party)}
                className={cn(
                  "-ml-px h-6 border border-border-strong px-2 font-mono text-[10px] leading-none text-muted first:ml-0 hover:text-fg focus-visible:relative focus-visible:-outline-offset-2",
                  view === party && "border-fg bg-fg text-bg hover:text-bg",
                )}
              >
                {party ? names[party] : t("All")}
              </button>
            ))}
          </div>
          <span className="text-[10px] leading-[14px] text-muted">
            {role && say(role, lang)}
            {role && view && " · "}
            {view && t("Hidden here: {{count}}", { count: hiddenCount })}
          </span>
        </>
      )}
      <button
        type="button"
        aria-expanded={showLegend}
        aria-controls={legendId}
        onClick={onLegend}
        className="ml-auto label-caps text-[10px] leading-[14px] text-faint underline decoration-dotted underline-offset-[3px] hover:text-fg"
      >
        {t("Legend")}
      </button>
    </div>
  );
}

function Legend({ id, open }: { id: string; open: boolean }) {
  const { t } = useTranslation();
  const kinds = useKindName();
  const sample = "font-mono text-[10px] leading-4 decoration-1 underline-offset-[3px]";
  return (
    <ul id={id} hidden={!open} className="flex flex-wrap gap-x-3 gap-y-1 font-sans">
      {KINDS.map((kind) => (
        <li key={kind} className={cn(sample, KIND_CLASS[kind])}>
          {kinds[kind]}
        </li>
      ))}
      <li className={cn(sample, "text-fg", LIT_CLASS.same)}>{t("Same value")}</li>
      <li className={cn(sample, LIT_CLASS.def)}>{t("Where it is defined")}</li>
      <li className={cn(sample, "text-fg", LIT_CLASS.flow)}>{t("Made by or used by")}</li>
      <li className={cn(sample, "text-muted")}>
        <span className={cn("inline-block w-6 align-middle", HIDDEN_CLASS)}>&nbsp;</span> {t("Hidden from this party")}
      </li>
    </ul>
  );
}

/** One formula block, in the page's mono-block style, every part of it explorable. */
export function Formula({ block }: { block: BlockId }) {
  const { t } = useTranslation();
  const [legend, setLegend] = useState(false);
  const legendId = useId();
  // One tab stop per formula: the part last focused (at first, the first part); the arrow keys move between parts.
  const lines = ANALYSIS.lines[block];
  const [current, setCurrent] = useState(() => lines.flat().find((tok) => tok.occ)!.occ!);
  const move = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const parts = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-occ]")];
    const at = parts.indexOf(e.target as HTMLElement);
    if (at < 0) return;
    const line = (el: HTMLElement) => el.closest("code");
    let next: HTMLElement | undefined;
    if (e.key === "ArrowRight") next = parts[at + 1];
    else if (e.key === "ArrowLeft") next = parts[at - 1];
    else if (e.key === "Home") next = parts[0];
    else if (e.key === "End") next = parts[parts.length - 1];
    else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      // The first part of the next (or the previous) line.
      const codes = [...e.currentTarget.querySelectorAll("code")];
      const to = codes[codes.indexOf(line(parts[at]!)!) + (e.key === "ArrowDown" ? 1 : -1)];
      next = to ? parts.find((p) => line(p) === to) : undefined;
    } else return;
    e.preventDefault();
    next?.focus();
  };
  return (
    <div data-formula={block} className="flex max-w-[46em] flex-col gap-2 border-l border-border-strong py-0.5 pl-3">
      <Tools block={block} legendId={legendId} showLegend={legend} onLegend={() => setLegend((on) => !on)} />
      <Legend id={legendId} open={legend} />
      <div
        role="group"
        aria-label={t("Formula")}
        aria-describedby={HINT_ID}
        className="flex flex-col gap-1 font-mono text-2xs leading-4 text-fg [overflow-wrap:anywhere]"
        onKeyDown={move}
        onFocus={(e) => {
          const occ = (e.target as HTMLElement).dataset.occ;
          if (occ) setCurrent(occ);
        }}
      >
        {lines.map((tokens, li) => (
          <code key={li} className="block">
            {tokens.map((tok, ti) => (tok.occ ? <Term key={ti} block={block} occ={tok.occ} text={tok.text} tabbable={tok.occ === current} /> : tok.text))}
          </code>
        ))}
      </div>
    </div>
  );
}

/** What a screen reader hears for a term: its kind, what it is, its flow, who knows it. */
function useDescribe(): (term: string) => string {
  const { t } = useTranslation();
  const lang = useLang();
  const kinds = useKindName();
  const names = usePartyName();
  const list = (items: string[]) => items.join(lang === "en" ? ", " : "、");
  return (term) => {
    const def = TERMS[term]!;
    const { madeBy, usedBy } = flowOf(ANALYSIS, term);
    const unknown = PARTIES.filter((p) => !def.known.includes(p));
    const parts = [`${kinds[def.kind]}.`, say(def.says, lang)];
    if (madeBy.length) parts.push(t("Made by {{fns}}.", { fns: list(madeBy) }));
    if (usedBy.length) parts.push(t("Used by {{fns}}.", { fns: list(usedBy) }));
    if (def.kind === "label" || def.kind === "fn" || def.kind === "op") parts.push(t("Public: everyone can know it."));
    else {
      parts.push(t("Known to: {{parties}}.", { parties: list(def.known.map((p) => names[p])) }));
      if (unknown.length) parts.push(t("Not known to: {{parties}}.", { parties: list(unknown.map((p) => names[p])) }));
    }
    return parts.join(" ");
  };
}

/** The descriptions `aria-describedby` points at: one per term on the page, never shown. */
function Glossary() {
  const { t } = useTranslation();
  const describe = useDescribe();
  const names = usePartyName();
  const concepts = useConceptWords();
  return (
    <div hidden data-formula-glossary="">
      {CONCEPTS.map((concept) => (
        <span key={concept} id={conceptDescId(concept)}>
          {`${concepts[concept].name}: ${concepts[concept].says}`}
        </span>
      ))}
      <span id={HINT_ID}>{t("Point at or focus any part of a formula to see what it is and where its value goes; the arrow keys move between its parts. Click it or press Enter to pin it; Esc unpins.")}</span>
      {PARTIES.map((party) => (
        <span key={party} id={hiddenId(party)}>
          {t("{{party}} cannot know this value.", { party: names[party] })}
        </span>
      ))}
      {[...ANALYSIS.byTerm.keys()].map((term) => (
        <span key={term} id={descId(term)}>
          {describe(term)}
        </span>
      ))}
    </div>
  );
}

/** A card beside a block (or a word of the page): above it, or below when there is no room or the other card is above. */
function Card({ occ, pinned, below }: { occ: string; pinned: boolean; below: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const [at, setAt] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const place = () => {
      const anchor = document.querySelector(`[data-occ="${occ}"]`);
      const card = ref.current;
      if (!anchor || !card) return;
      const a = anchor.getBoundingClientRect();
      const b = (anchor.closest("[data-formula]") ?? anchor).getBoundingClientRect();
      const vw = document.documentElement.clientWidth || window.innerWidth;
      const vh = document.documentElement.clientHeight || window.innerHeight;
      const w = card.offsetWidth;
      const h = card.offsetHeight;
      const m = 8;
      const gap = 6;
      const above = b.top - gap - h;
      const under = b.bottom + gap;
      const fitsAbove = above >= m;
      const fitsUnder = under + h <= vh - m;
      let top = below ? (fitsUnder || !fitsAbove ? under : above) : fitsAbove || !fitsUnder ? above : under;
      if (!fitsAbove && !fitsUnder) top = b.top - m > vh - b.bottom - m ? Math.max(m, above) : Math.min(under, vh - h - m);
      const left = Math.max(m, Math.min(a.left, vw - w - m));
      setAt({ left, top });
    };
    place();
    let frame = 0;
    const later = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(place);
    };
    window.addEventListener("resize", later);
    document.addEventListener("scroll", later, true);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", later);
      document.removeEventListener("scroll", later, true);
    };
  }, [occ, below]);

  return createPortal(
    <div
      ref={ref}
      data-formula-popup={occ}
      data-pinned={pinned ? "" : undefined}
      aria-hidden="true"
      style={at ? { left: at.left, top: at.top } : { left: 0, top: 0, visibility: "hidden" }}
      className="fixed z-50 flex w-max max-w-[min(26rem,calc(100vw-16px))] flex-col gap-1.5 border border-border-strong bg-bg px-3 py-2.5 text-left font-sans"
    >
      {occ.startsWith("c:") ? <ConceptCard concept={occ.split(":")[1] as ConceptId} pinned={pinned} /> : <TermCard occ={occ} pinned={pinned} />}
    </div>,
    document.body,
  );
}

function TermCard({ occ, pinned }: { occ: string; pinned: boolean }) {
  const { t } = useTranslation();
  const lang = useLang();
  const kinds = useKindName();
  const names = usePartyName();
  const o = ANALYSIS.occurrences.get(occ)!;
  const def = TERMS[o.term]!;
  const { madeBy, usedBy } = flowOf(ANALYSIS, o.term);
  const roles = !!rolesOf(o.block);
  return (
    <>
      <div className="flex items-baseline gap-2">
        <code className={cn("font-mono text-xs decoration-1 underline-offset-[3px]", KIND_CLASS[def.kind])}>{o.text}</code>
        <span className="label-caps text-[10px] leading-[14px] text-muted">{kinds[def.kind]}</span>
        {pinned && <Pin className="ml-auto size-3 shrink-0 self-center text-muted" />}
      </div>
      <p className="text-xs leading-[18px] text-fg">{say(def.says, lang)}</p>
      {(madeBy.length > 0 || usedBy.length > 0) && (
        <p className="font-mono text-[10px] leading-[14px] text-muted">
          {madeBy.length > 0 && `${madeBy.join(", ")} → `}
          <span className="text-fg">{o.text}</span>
          {usedBy.length > 0 && ` → ${usedBy.join(", ")}`}
        </p>
      )}
      {def.kind === "label" || def.kind === "fn" || def.kind === "op" || !roles ? (
        <p className="text-[10px] leading-[14px] text-muted">{t("Public: everyone can know it.")}</p>
      ) : (
        <ul className="flex flex-wrap gap-x-2.5 gap-y-0.5 text-[10px] leading-[14px]">
          {PARTIES.map((party) => {
            const knows = def.known.includes(party);
            const Icon = knows ? Check : Minus;
            return (
              <li key={party} className={cn("inline-flex items-center gap-0.5", knows ? "text-fg" : "text-faint line-through")}>
                <Icon className="size-2.5 shrink-0" />
                {names[party]}
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}

function Cards() {
  const store = useStore();
  const snapshot = useSyncExternalStore(
    store.subscribe,
    () => `${store.pin ?? ""}|${store.passing() ?? ""}`,
    () => "|",
  );
  const [pin, passing] = snapshot.split("|").map((s) => s || null) as [string | null, string | null];

  useEffect(() => {
    if (!pin && !passing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") store.update({ pin: null, dismissed: store.passing() ?? store.dismissed });
    };
    const onDown = (e: PointerEvent) => {
      const target = e.target as Element | null;
      if (store.pin && !target?.closest?.("[data-occ],[data-formula-popup],[data-formula-tools]")) store.update({ pin: null });
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onDown, true);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onDown, true);
    };
  }, [pin, passing, store]);

  const sameBlock = (a: string | null, b: string | null) => !!a && !!b && a.split(":")[0] === b.split(":")[0];
  return (
    <>
      {pin && <Card key={`pin-${pin}`} occ={pin} pinned below={false} />}
      {passing && passing !== pin && <Card key={`pass-${passing}`} occ={passing} pinned={false} below={sameBlock(pin, passing)} />}
    </>
  );
}

/** One announcement for the whole page when "view as" changes: whose view, and how many values it hides. */
function ViewAnnouncer() {
  const { t } = useTranslation();
  const store = useStore();
  const names = usePartyName();
  const view = useSyncExternalStore(store.subscribe, () => store.view, () => null);
  const [said, setSaid] = useState("");
  // Said only when the choice changes, never on load.
  const last = useRef<Party | null>(null);
  useEffect(() => {
    if (view === last.current) return;
    last.current = view;
    if (!view) return setSaid(t("Showing every value."));
    const hidden = (Object.keys(BLOCKS) as BlockId[]).reduce((n, block) => n + ANALYSIS.lines[block].flat().filter((tok) => tok.occ && hiddenFrom(block, tok.term!, view)).length, 0);
    setSaid(t("Viewing as {{party}}: {{count}} values on this page are hidden.", { party: names[view], count: hidden }));
  }, [view, t, names]);
  return (
    <span aria-live="polite" className="sr-only" data-formula-announcer="">
      {said}
    </span>
  );
}

/** The page's formulas share one state: what is lit and pinned, and whose view. */
export function FormulaProvider({ children }: { children: ReactNode }) {
  const [store] = useState(() => new FormulaStore());
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    setMounted(true);
    const pointer = () => void (store.pointing = true);
    const keyboard = () => void (store.pointing = false);
    document.addEventListener("pointerdown", pointer, true);
    document.addEventListener("keydown", keyboard, true);
    return () => {
      document.removeEventListener("pointerdown", pointer, true);
      document.removeEventListener("keydown", keyboard, true);
    };
  }, [store]);
  return (
    <StoreContext.Provider value={store}>
      {children}
      <Glossary />
      <ViewAnnouncer />
      {mounted && <Cards />}
    </StoreContext.Provider>
  );
}
