/**
 * Keeping a fence's own config out of the picture's palette.
 *
 * Pele reads config from two places: `%%{init: …}%%` directives and the YAML
 * frontmatter's `config:` block. Of everything a fence can put there, the one
 * key Pele honours for how the diagram looks is `themeVariables`: it becomes
 * the series palette (pie slices, git branches, …) and the ink of the labels
 * on those colours. That palette is the fence's to choose in Mermaid, but here
 * `transparent`, or `var(--pele-bg)`, can hide a slice and its legend, so it
 * must not reach the picture.
 *
 * The host `config` option cannot override it: Pele merges a fence's config
 * over the host's (`{ ...extra, ...pre.config }` in its core), so the source
 * is stripped before Pele sees it. The returned SVG is swept once more for
 * `--pele-series-*` declarations as a defence in depth, for a config form
 * this stripper does not know.
 *
 * Only `themeVariables` is removed; every other config key (a gantt's
 * `displayMode`, a flowchart's `curve`, a frontmatter `title`) is left where
 * it was, so legitimate diagrams are unchanged.
 */

const THEME = "themeVariables";

/** Pele's own directive regex (`src/preprocess.ts`): `%%{init: …}%%` and friends. */
const RE_DIRECTIVE = /%{2}{\s*(?:(\w+)\s*:|(\w+))\s*(?:(\w+)|((?:(?!}%{2}).|\r?\n)*))?\s*(?:}%{2})?/gi;

/**
 * Remove `themeVariables` from every init directive that is a JSON object.
 * A directive Pele would ignore (invalid JSON, not an object) is left as it
 * is; an unclosed one is too, as rewriting it could eat the diagram.
 */
function stripDirectives(source: string): string {
  const matches = [...source.matchAll(RE_DIRECTIVE)].filter((match) => {
    const kind = (match[1] ?? match[2] ?? "").toLowerCase();
    return (kind === "init" || kind === "initialize") && match[4] !== undefined && /}%{2}$/.test(match[0]);
  });
  let out = source;
  for (let i = matches.length - 1; i >= 0; i--) {
    const match = matches[i]!;
    let args: unknown;
    try {
      // Pele normalises every single quote to a double quote before parsing.
      args = JSON.parse(match[4]!.replace(/'/g, '"').trim());
    } catch {
      continue;
    }
    if (typeof args !== "object" || args === null || Array.isArray(args) || !(THEME in args)) continue;
    const { [THEME]: _removed, ...rest } = args as Record<string, unknown>;
    const body = JSON.stringify(rest);
    // A `}%%` in a string would end Pele's directive early; leave that one
    // for the output sweep rather than change what the rest of the config says.
    if (body.includes("}%%")) continue;
    out = out.slice(0, match.index!) + `%%{init:${body}}%%` + out.slice(match.index! + match[0].length);
  }
  return out;
}

interface FrontMatter {
  bodyStart: number;
  bodyEnd: number;
}

/**
 * Pele's frontmatter is a `---` fence at the start (after spaces or tabs) and
 * the last `---` line after it (`matchFrontMatter` in its preprocess). Its
 * body is what the config stripper works on.
 */
function frontMatter(source: string): FrontMatter | null {
  const open = /^([ \t]*)---[ \t]*(?:\n|$)/.exec(source);
  if (!open) return null;
  const bodyStart = open[0].length;
  const closing = new RegExp(`^${escapeRegExp(open[1]!)}---[ \\t]*(?:\\n|$)`, "gm");
  closing.lastIndex = bodyStart;
  let last: RegExpExecArray | null = null;
  for (let match = closing.exec(source); match !== null; match = closing.exec(source)) last = match;
  if (last === null) return null;
  return { bodyStart, bodyEnd: last.index };
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** A YAML key at the start of a line, with its indentation. */
function blockKey(line: string): { indent: number; key: string } | null {
  const match = /^([ \t]*)(?:"([^"]+)"|'([^']+)'|([^\s:][^:]*?))[ \t]*:/.exec(line);
  if (!match) return null;
  return { indent: match[1]!.length, key: (match[2] ?? match[3] ?? match[4] ?? "").trim() };
}

/**
 * Remove every `themeVariables` block inside a `config:` block, including its
 * indented children. A `title: |` block outside `config` is not touched.
 */
function stripBlockConfig(lines: string[]): string[] {
  for (let i = 0; i < lines.length; i++) {
    const config = /^([ \t]*)config[ \t]*:[ \t]*(?:#.*)?$/.exec(lines[i]!);
    if (!config) continue;
    const childIndent = config[1]!.length;
    let end = i + 1;
    while (end < lines.length) {
      const line = lines[end]!;
      if (line.trim() === "") {
        end++;
        continue;
      }
      if (indentation(line) <= childIndent) break;
      end++;
    }
    for (let j = i + 1; j < end; j++) {
      const key = blockKey(lines[j]!);
      if (key?.key !== THEME) continue;
      let stop = j + 1;
      while (stop < end) {
        const line = lines[stop]!;
        if (line.trim() === "") {
          stop++;
          continue;
        }
        if (indentation(line) <= key.indent) break;
        stop++;
      }
      lines.splice(j, stop - j);
      end -= stop - j;
      j--;
    }
  }
  return lines;
}

function indentation(line: string): number {
  return /^[ \t]*/.exec(line)![0].length;
}

/**
 * Remove `themeVariables` keys from a YAML flow mapping (`config: { … }`).
 * Keys and values are scanned with their quotes respected.
 */
function stripFlowConfig(text: string): string {
  const starts: number[] = [];
  for (const match of text.matchAll(/(^|\n)([ \t]*)config[ \t]*:[ \t]*\{/gm)) {
    starts.push(match.index! + match[0].length - 1);
  }
  let out = text;
  for (let i = starts.length - 1; i >= 0; i--) {
    const open = starts[i]!;
    const close = balancedEnd(out, open);
    if (close < 0) continue;
    const cleaned = removeFlowKey(out.slice(open + 1, close), THEME);
    out = out.slice(0, open + 1) + cleaned + out.slice(close);
  }
  return out;
}

/** The index of the closer matching the opener at `open`, or -1. */
function balancedEnd(text: string, open: number): number {
  const stack: string[] = [];
  let quote = "";
  for (let i = open; i < text.length; i++) {
    const char = text[i]!;
    if (quote !== "") {
      if (char === "\\" && quote === '"') i++;
      else if (char === quote) quote = "";
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
    } else if (char === "{" || char === "[") {
      stack.push(char === "{" ? "}" : "]");
    } else if (char === "}" || char === "]") {
      if (stack.pop() !== char) return -1;
      if (stack.length === 0) return i;
    }
  }
  return -1;
}

function removeFlowKey(text: string, key: string): string {
  let out = text;
  let from = 0;
  for (;;) {
    const found = flowKey(out, key, from);
    if (found === null) return out;
    let start = found.start;
    let end = found.end;
    let before = start - 1;
    while (before >= 0 && /\s/.test(out[before]!)) before--;
    let after = end;
    while (after < out.length && /[ \t]/.test(out[after]!)) after++;
    if (out[after] === ",") {
      after++;
      while (after < out.length && /[ \t]/.test(out[after]!)) after++;
    } else if (out[before] === ",") {
      start = before;
    }
    out = out.slice(0, start) + out.slice(after);
    from = start;
  }
}

interface FlowKey {
  start: number;
  end: number;
}

/** `key:` in a flow mapping, outside a quoted string, starting at `from`. */
function flowKey(text: string, key: string, from: number): FlowKey | null {
  let quote = "";
  let previous = "{";
  for (let i = from; i < text.length; i++) {
    const char = text[i]!;
    if (quote !== "") {
      if (char === "\\" && quote === '"') i++;
      else if (char === quote) quote = "";
      continue;
    }
    if (char === '"' || char === "'") {
      if (previous === "{" || previous === ",") {
        const closing = text[i + key.length + 1];
        if (text.startsWith(key, i + 1) && closing === char) {
          const colon = skipSpace(text, i + key.length + 2);
          if (text[colon] === ":") return { start: i, end: flowValueEnd(text, colon + 1) };
        }
      }
      quote = char;
      continue;
    }
    if (char === "{") {
      previous = "{";
      continue;
    }
    if (char === "}") {
      previous = "}";
      continue;
    }
    if (char === ",") {
      previous = ",";
      continue;
    }
    if (/\s/.test(char)) continue;
    if ((previous === "{" || previous === ",") && text.startsWith(key, i) && !/[\w-]/.test(text[i + key.length] ?? "")) {
      const colon = skipSpace(text, i + key.length);
      if (text[colon] === ":") return { start: i, end: flowValueEnd(text, colon + 1) };
    }
    previous = char;
  }
  return null;
}

function skipSpace(text: string, at: number): number {
  while (at < text.length && /[ \t]/.test(text[at]!)) at++;
  return at;
}

/** The index just past a flow value starting at `from`. */
function flowValueEnd(text: string, from: number): number {
  let at = skipSpace(text, from);
  if (text[at] === "{" || text[at] === "[") {
    const close = balancedEnd(text, at);
    return close < 0 ? text.length : close + 1;
  }
  let quote = "";
  for (; at < text.length; at++) {
    const char = text[at]!;
    if (quote !== "") {
      if (char === "\\" && quote === '"') at++;
      else if (char === quote) quote = "";
      continue;
    }
    if (char === '"' || char === "'") quote = char;
    else if (char === "," || char === "}" || char === "]" || char === "\n") break;
  }
  return at;
}

/**
 * `source` with every `themeVariables` removed from its init directives and
 * its frontmatter config. Line endings are normalised as Pele normalises them.
 */
export function withoutFenceThemeVariables(source: string): string {
  let out = source.replace(/\r\n?/g, "\n");
  const front = frontMatter(out);
  if (front !== null) {
    const body = out.slice(front.bodyStart, front.bodyEnd);
    let cleaned = stripFlowConfig(stripBlockConfig(body.split("\n")).join("\n"));
    // The block remover takes blank lines that followed the removed key with
    // it; the newline before the closing fence is structural.
    if (body.endsWith("\n") && !cleaned.endsWith("\n")) cleaned += "\n";
    if (cleaned !== body) out = out.slice(0, front.bodyStart) + cleaned + out.slice(front.bodyEnd);
  }
  return stripDirectives(out);
}

/**
 * Defence in depth: Pele writes a fence's series palette as
 * `--pele-series-N:<colour>` declarations on the SVG root. Remove them, so
 * the `var(--pele-series-N, <theme default>)` references fall back to the
 * host's colours even if a config form slipped past the source stripper.
 */
export function withoutSeriesPalette(svg: string): string {
  return svg.replace(/--pele-series-\d+:[^;"]*;?/g, "");
}
