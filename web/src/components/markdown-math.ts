/**
 * LaTeX in an agent's reply, typeset with KaTeX: the part of `markdown.tsx`
 * that is fetched only once a message that may hold math is drawn with the
 * `math` pref on, so a page without formulas never downloads KaTeX.
 *
 * What counts as math, and what does not:
 *
 * - `$…$` and `\(…\)` inline; `$$…$$` and `\[…\]` displayed, on lines of their
 *   own or inside a sentence (as LaTeX itself does, and as Claude and Codex
 *   write them).
 * - Never inside inline code or a fenced block, ` ```math ` included: code is
 *   shown as written.
 * - A single `$` pair is math only the way Pandoc reads it: no space just
 *   inside either `$`, and no digit right after the closing one; nor when a
 *   backtick is between them. That keeps prices (`$5 and $10`, `$5-$10`) and
 *   shell variables in prose (`$HOME and $PATH`) as text, and the Markdown
 *   between them (`$5 for **pro**`, `` `echo $PATH` ``) as Markdown.
 * - TeX KaTeX cannot read stays as the text the agent wrote.
 *
 * A typeset formula carries its TeX (`data-tex`) and is a button: clicking it
 * copies the TeX (`Markdown` in `markdown.tsx` does the copying).
 */
import type { Element, ElementContent, Root as HastRoot } from "hast";
import { fromHtmlIsomorphic } from "hast-util-from-html-isomorphic";
import katex from "katex";
import type { Paragraph, PhrasingContent, Root as MdastRoot, RootContent } from "mdast";
import type { Options } from "react-markdown";
import remarkMath from "remark-math";

// The two nodes `remark-math` adds to mdast (its types put them in mdast's
// content maps; the names live in `mdast-util-math`, not a dependency here).
type InlineMath = Extract<PhrasingContent, { type: "inlineMath" }>;
type Math = Extract<RootContent, { type: "math" }>;

/** The start of a fence: ``` or ~~~, under any indent or `>` a list or quote puts before it. */
const FENCE = /^(?:[ \t]*>)*[ \t]*(`{3,}|~{3,})/;

/**
 * The text as the math parser should read it, before it does.
 *
 * `\(…\)` and `\[…\]` are rewritten as `$…$` and `$$…$$`, which is all the
 * parser understands. And every single `$` the parser would pair but that is
 * not math (see the top of this file) is escaped: decided after parsing
 * would be too late, as the pair would already have swallowed what is
 * between them — a code span, a bold word — as TeX.
 *
 * Code is left alone: fenced blocks line by line, inline code by its
 * backtick runs, as CommonMark pairs them.
 */
export function prepareMath(text: string): string {
  if (!/\$|\\[([]/.test(text)) return text;
  const out: string[] = [];
  let prose: string[] = [];
  let fence: string | null = null;
  const flush = () => {
    if (prose.length > 0) out.push(rewriteProse(prose.join("\n")));
    prose = [];
  };
  for (const line of text.split("\n")) {
    const open = FENCE.exec(line);
    if (fence) {
      out.push(line);
      // Closed by a run of the same character at least as long, with nothing after it.
      if (open && open[1]![0] === fence[0] && open[1]!.length >= fence.length && line.slice(open[0].length).trim() === "") {
        fence = null;
      }
      continue;
    }
    // A backtick fence's info string cannot hold a backtick: a line that has
    // one is inline code, not a fence.
    if (open && !(open[1]![0] === "`" && line.slice(open[0].length).includes("`"))) {
      flush();
      fence = open[1]!;
      out.push(line);
      continue;
    }
    prose.push(line);
  }
  flush();
  return out.join("\n");
}

/** One stretch of text between fences, its code spans skipped. */
function rewriteProse(s: string): string {
  let out = "";
  let i = 0;
  while (i < s.length) {
    const c = s[i]!;
    if (c === "`") {
      let n = 1;
      while (s[i + n] === "`") n++;
      const close = closingRun(s, i + n, n);
      const end = close < 0 ? i + n : close + n;
      out += s.slice(i, end);
      i = end;
      continue;
    }
    if (c === "\\" && (s[i + 1] === "(" || s[i + 1] === "[")) {
      const display = s[i + 1] === "[";
      const end = s.indexOf(display ? "\\]" : "\\)", i + 2);
      const body = end < 0 ? "" : s.slice(i + 2, end);
      // Not across a blank line (that is the paragraph's end), and not around
      // a `$`, which would close the math early.
      if (end >= 0 && body.trim() !== "" && !/\n[ \t>]*\n/.test(body) && !body.includes("$")) {
        out += display ? displayMath(s, i, body, end + 2) : `$${body.trim()}$`;
        i = end + 2;
        continue;
      }
    }
    if (c === "\\") {
      // An escape: whatever follows is literal, a backtick or `$` included.
      out += s.slice(i, i + 2);
      i += 2;
      continue;
    }
    if (c === "$") {
      let n = 1;
      while (s[i + n] === "$") n++;
      const close = n === 1 ? nextSingleDollar(s, i + 1) : -1;
      if (close >= 0 && isInlineMath(s.slice(i + 1, close), s[close + 1])) {
        out += s.slice(i, close + 1);
        i = close + 1;
      } else {
        // A `$$` run the parser pairs on its own; a `$` with nothing to pair
        // with, as is; one that would pair but is not math, escaped.
        out += close >= 0 ? "\\$" : s.slice(i, i + n);
        i += close >= 0 ? 1 : n;
      }
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

/** The next lone `$` in the paragraph, the one the parser would close `$` with; -1 if none. */
function nextSingleDollar(s: string, from: number): number {
  const paragraphEnd = s.slice(from).search(/\n[ \t>]*\n/);
  const end = paragraphEnd < 0 ? s.length : from + paragraphEnd;
  for (let i = s.indexOf("$", from); i >= 0 && i < end; i = s.indexOf("$", i)) {
    let n = 1;
    while (s[i + n] === "$") n++;
    if (n === 1) return i;
    i += n;
  }
  return -1;
}

/** Pandoc's test for `$body$` followed by `after`, plus: no code span inside. */
function isInlineMath(body: string, after: string | undefined): boolean {
  return body !== "" && !/^\s|\s$/.test(body) && !/^\d/.test(after ?? "") && !body.includes("`");
}

/** Where the next run of exactly `n` backticks starts, or -1. */
function closingRun(s: string, from: number, n: number): number {
  for (let i = s.indexOf("`", from); i >= 0; i = s.indexOf("`", i)) {
    let m = 1;
    while (s[i + m] === "`") m++;
    if (m === n) return i;
    i += m;
  }
  return -1;
}

/**
 * `\[…\]` as dollars. With each delimiter on a line of its own it becomes a
 * `$$` block in place, every line and its list indent or `>` kept. Anything
 * else is folded onto one line: a `$$` that starts a line with more after it
 * would open a block that never closes.
 */
function displayMath(s: string, start: number, body: string, end: number): string {
  const lineStart = s.lastIndexOf("\n", start - 1) + 1;
  const lineEnd = s.indexOf("\n", end);
  const before = s.slice(lineStart, start);
  const after = s.slice(end, lineEnd < 0 ? s.length : lineEnd);
  if (/^[ \t>]*$/.test(before) && after.trim() === "" && /^[ \t]*\n/.test(body) && /\n[ \t>]*$/.test(body)) {
    return `$$${body}$$`;
  }
  return `$$${body.trim().replace(/[ \t]*\n[ \t>]*/g, " ")}$$`;
}

/** The `data` that makes mdast-to-hast draw a math node as an element the rehype step below finds. */
function mathData(tex: string, display: boolean, block: boolean) {
  return {
    hName: block ? "div" : "span",
    hProperties: { className: ["math", display ? "math-display" : "math-inline"] },
    hChildren: [{ type: "text" as const, value: tex }],
  };
}

/**
 * After parsing: which math is displayed. A `$$…$$` inside a line is
 * displayed, and one alone in its paragraph becomes a block.
 */
function remarkTidyMath() {
  return (tree: MdastRoot, file: { value: unknown }) => {
    const source = String(file.value);
    const visit = (parent: { children: RootContent[] }) => {
      for (let i = 0; i < parent.children.length; i++) {
        const node = parent.children[i]!;
        if (node.type === "math") {
          node.data = mathData(node.value, true, true);
        } else if (node.type === "inlineMath") {
          parent.children[i] = inlineMath(node, source);
        } else if ("children" in node) {
          visit(node as { children: RootContent[] });
        }
        if (node.type === "paragraph") {
          const block = loneDisplay(node);
          if (block) parent.children[i] = block;
        }
      }
    };
    visit(tree);
  };
}

function inlineMath(node: InlineMath, source: string): InlineMath {
  const from = node.position?.start.offset;
  const display = from !== undefined && source.startsWith("$$", from);
  node.data = mathData(node.value, display, false);
  return node;
}

/** A paragraph that holds one displayed formula and nothing else, as a block. */
function loneDisplay(paragraph: Paragraph): Math | null {
  const parts = paragraph.children.filter((child) => !(child.type === "text" && child.value.trim() === ""));
  const only = parts.length === 1 ? parts[0]! : null;
  if (only?.type !== "inlineMath") return null;
  const className = (only.data?.hProperties as { className?: string[] } | undefined)?.className;
  if (!className?.includes("math-display")) return null;
  return { type: "math", value: only.value, position: paragraph.position, data: mathData(only.value, true, true) };
}

/**
 * KaTeX's output for a formula, as hast, or null for TeX it cannot read.
 *
 * Untrusted TeX in. No command that links, loads or styles (`\href`, `\url`,
 * `\includegraphics`, `\htmlClass` and the like) is trusted, and a formula
 * that uses one is left as written: refused, KaTeX would draw the command's
 * name in red and drop its argument from view. Sizes are capped, and the
 * markup goes through hast to React rather than into `innerHTML`.
 */
function typeset(tex: string, displayMode: boolean): ElementContent[] | null {
  let refused = false;
  try {
    const html = katex.renderToString(tex, {
      displayMode,
      throwOnError: true,
      trust: () => {
        refused = true;
        return false;
      },
      // Unicode text in math mode (a Chinese word in `\text{}` or bare) is
      // drawn as written instead of warned about in the console.
      strict: "ignore",
      maxSize: 20,
    });
    if (refused) return null;
    return fromHtmlIsomorphic(html, { fragment: true }).children as ElementContent[];
  } catch {
    return null;
  }
}

/**
 * Typeset every math element the remark step marked. `copyTitle` is the
 * tooltip of a typeset formula, in the page's language.
 */
export function rehypeTypeset({ copyTitle }: { copyTitle: string }) {
  return (tree: HastRoot) => {
    const visit = (parent: { children: (HastRoot["children"][number] | ElementContent)[] }) => {
      for (const node of parent.children) {
        if (node.type !== "element") continue;
        const className = node.properties.className;
        const classes = Array.isArray(className) ? className : [];
        if (classes.includes("math")) {
          render(node, classes.includes("math-display"), copyTitle);
          continue;
        }
        visit(node);
      }
    };
    visit(tree);
  };
}

function render(node: Element, display: boolean, copyTitle: string) {
  const tex = node.children.map((child) => (child.type === "text" ? child.value : "")).join("");
  const drawn = typeset(tex, display);
  if (drawn) {
    node.children = drawn;
    Object.assign(node.properties, { dataTex: tex, role: "button", tabIndex: 0, title: copyTitle });
    return;
  }
  // Not TeX KaTeX can read: the source, with the delimiters it was written in.
  const delimiter = display ? "$$" : "$";
  node.properties.className = ["math-error"];
  node.children = [{ type: "text", value: `${delimiter}${tex}${delimiter}` }];
}

export const remarkPlugins: NonNullable<Options["remarkPlugins"]> = [remarkMath, remarkTidyMath];
