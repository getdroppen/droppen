import { BLOCKS, type BlockId, COMMON, type FormulaBlock, type FormulaLine, LABELS, TERMS } from "./formula-data";

/*
 * The formulas as tokens, and how their values flow (`formula.tsx` draws
 * them). Pure, so the tests read exactly what the page draws.
 *
 * A line splits into labels ("rh/v1 …"), names, numbers, operators (‖ | ← →
 * and the [0..5] slice) and plain punctuation; every token but punctuation
 * names a term (`formula-data.ts`): the line's own meaning first, then the
 * block's, then the common one. A name not found whole that has underscores
 * (the Noise suite) is split at them.
 *
 * The flow is read off the formula's shape: a name at the start of a line
 * (or after `;` or `·`) followed by `=` is where that value is defined, made
 * by the function call that follows the `=`; every other token is an input
 * of the innermost function call around it.
 */

export type TokenType = "label" | "name" | "number" | "op" | "punct";

export interface Token {
  text: string;
  type: TokenType;
  /** The term (`TERMS`); none for punctuation, or when nothing names it (a test catches that). */
  term?: string;
  /** This occurrence's key, `block:line:index`: unique on the page. */
  occ?: string;
}

export interface Occurrence {
  occ: string;
  term: string;
  block: BlockId;
  text: string;
  /** Where its value is defined (the name before `=`). */
  def: boolean;
  /** A function called here (followed by `(`). */
  call: boolean;
  /** The call that takes it as an input. */
  usedBy?: string;
  /** For a definition: the call that makes it. */
  madeBy?: string;
  /** For a call: its inputs, and the value it makes. */
  inputs: string[];
  makes?: string;
}

export interface Analysis {
  lines: Record<BlockId, Token[][]>;
  occurrences: Map<string, Occurrence>;
  /** Each term's occurrences on the page, in order. */
  byTerm: Map<string, string[]>;
}

const TOKEN = /("(?:[^"\\]|\\.)*")|(rh\/v1(?: [a-z-]+)?)|(\[\d+\.\.\d+\])|(\d+ [dh](?![\w]))|([A-Za-z_][\w\-.−]*(?:\[\])?)|(\d+)|(‖|\||←|→)|([\s\S])/gu;

/** A quoted label without its quotes and `\0`. */
function labelOf(text: string): string {
  return text.replace(/^"|"$/g, "").replace(/\\0$/, "");
}

function lex(text: string): { text: string; type: TokenType }[] {
  const out: { text: string; type: TokenType }[] = [];
  for (const m of text.matchAll(TOKEN)) {
    const type: TokenType = m[1] || m[2] ? "label" : m[3] || m[7] ? "op" : m[4] || m[6] ? "number" : m[5] ? "name" : "punct";
    const last = out[out.length - 1];
    if (type === "punct" && last?.type === "punct") last.text += m[0];
    else out.push({ text: m[0], type });
  }
  return out;
}

type Meanings = Readonly<Record<string, string | readonly string[]>>;

/** The term `text` names here: the n-th occurrence of it in its line and in its block. */
function meaning(text: string, type: TokenType, line: Meanings | undefined, block: FormulaBlock, nthInLine: number, nthInBlock: number): string | undefined {
  if (type === "label") return LABELS[labelOf(text)];
  const pick = (m: string | readonly string[] | undefined, n: number) => (typeof m === "string" ? m : m?.[n]);
  return pick(line?.[text], nthInLine) ?? pick(block.terms?.[text] as string | undefined, nthInBlock) ?? COMMON[text] ?? (type === "number" ? block.terms?.["#"] : undefined);
}

/** One line's tokens, each with its term. */
function tokenize(block: FormulaBlock, line: string | FormulaLine, counts: Map<string, number>): Token[] {
  const text = typeof line === "string" ? line : line.text;
  const own = typeof line === "string" ? undefined : line.terms;
  const inLine = new Map<string, number>();
  const out: Token[] = [];
  const take = (t: { text: string; type: TokenType }) => {
    if (t.type === "punct") return out.push({ ...t });
    const nl = inLine.get(t.text) ?? 0;
    const nb = counts.get(t.text) ?? 0;
    inLine.set(t.text, nl + 1);
    counts.set(t.text, nb + 1);
    out.push({ ...t, term: meaning(t.text, t.type, own, block, nl, nb) });
  };
  for (const t of lex(text)) {
    const whole = t.type === "name" ? meaning(t.text, t.type, own, block, 0, 0) : "x";
    if (!whole && t.text.includes("_") && t.text.split("_").every((part) => COMMON[part])) {
      t.text.split("_").forEach((part, i) => {
        if (i > 0) take({ text: "_", type: "punct" });
        take({ text: part, type: "name" });
      });
    } else take(t);
  }
  return out;
}

/** The whole page's formulas, tokenized, with each value's flow. */
export function analyze(): Analysis {
  const lines = {} as Record<BlockId, Token[][]>;
  const occurrences = new Map<string, Occurrence>();
  const byTerm = new Map<string, string[]>();
  for (const id of Object.keys(BLOCKS) as BlockId[]) {
    const block: FormulaBlock = BLOCKS[id];
    const counts = new Map<string, number>();
    lines[id] = block.lines.map((line, li) => {
      const tokens = tokenize(block, line, counts);
      const at: Occurrence[] = [];
      tokens.forEach((t, ti) => {
        if (t.type === "punct" || !t.term) return;
        t.occ = `${id}:${li}:${ti}`;
        const o: Occurrence = { occ: t.occ, term: t.term, block: id, text: t.text, def: false, call: false, inputs: [] };
        occurrences.set(t.occ, o);
        at[ti] = o;
        byTerm.set(t.term, [...(byTerm.get(t.term) ?? []), t.occ]);
      });
      // The shape: definitions, calls, and what each call takes.
      const stack: (Occurrence | null)[] = [];
      let segmentStart = true;
      tokens.forEach((t, ti) => {
        const next = tokens[ti + 1];
        if (t.type === "punct") {
          for (const ch of t.text) {
            if (ch === "(") stack.push(at[ti - 1]?.call ? at[ti - 1]! : null);
            else if (ch === ")") stack.pop();
            else if (ch === ";" || ch === "·") segmentStart = true;
          }
          return;
        }
        const o = at[ti];
        if (!o) return;
        const kind = TERMS[o.term]?.kind;
        o.call = kind === "fn" && !!next && next.type === "punct" && next.text.startsWith("(");
        const caller = [...stack].reverse().find(Boolean);
        if (caller && kind !== "op") {
          o.usedBy = caller.occ;
          caller.inputs.push(o.occ);
        }
        if (segmentStart && next?.type === "punct" && /^\s*=/.test(next.text) && kind !== "op") {
          o.def = true;
          const after = at[ti + 2];
          if (after && TERMS[after.term]?.kind === "fn" && tokens[ti + 3]?.text.startsWith("(")) {
            o.madeBy = after.occ;
            after.makes = o.occ;
          }
        }
        segmentStart = false;
      });
      return tokens;
    });
  }
  return { lines, occurrences, byTerm };
}

/** How a token is lit while another is pointed at, focused or pinned. */
export type Lit = "def" | "same" | "flow" | "";

/** Every occurrence `active` lights: the same value everywhere, where it is defined, and its flow. */
export function lights(a: Analysis, active: string): Map<string, Lit> {
  const out = new Map<string, Lit>();
  const occ = a.occurrences.get(active);
  if (!occ) return out;
  const term = TERMS[occ.term]!;
  const put = (key: string | undefined, lit: Lit) => {
    if (!key) return;
    const rank = { def: 3, same: 2, flow: 1, "": 0 };
    if (rank[lit] > rank[out.get(key) ?? ""]) out.set(key, lit);
  };
  for (const key of a.byTerm.get(occ.term) ?? []) put(key, a.occurrences.get(key)!.def ? "def" : "same");
  if (term.kind === "fn") {
    // A call: what it takes, and what it makes.
    for (const key of occ.inputs) put(key, "flow");
    put(occ.makes, "flow");
  } else if (term.kind !== "op") {
    // A value: the calls that make it and those that take it, wherever it is.
    for (const key of a.byTerm.get(occ.term) ?? []) {
      const o = a.occurrences.get(key)!;
      put(o.madeBy, "flow");
      put(o.usedBy, "flow");
    }
    for (const rel of term.rel ?? []) for (const key of a.byTerm.get(rel) ?? []) put(key, "flow");
  }
  return out;
}

/** The functions that make a term's value, and those that take it, by name, as the formulas spell them. */
export function flowOf(a: Analysis, term: string): { madeBy: string[]; usedBy: string[] } {
  const madeBy = new Set<string>();
  const usedBy = new Set<string>();
  for (const key of a.byTerm.get(term) ?? []) {
    const o = a.occurrences.get(key)!;
    if (o.madeBy) madeBy.add(a.occurrences.get(o.madeBy)!.text);
    if (o.usedBy) usedBy.add(a.occurrences.get(o.usedBy)!.text);
  }
  return { madeBy: [...madeBy], usedBy: [...usedBy] };
}
