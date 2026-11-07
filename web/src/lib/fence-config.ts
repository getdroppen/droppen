/**
 * Keeping a fence's own config out of the picture's palette.
 *
 * Pele reads config from two places: `%%{init: …}%%` directives and the YAML
 * frontmatter's `config:` block. Of everything a fence can put there, the one
 * key Pele honours for how the diagram looks is `themeVariables`: it becomes
 * the series palette (pie slices, git branches, …) and the ink of the labels
 * on those colours. That palette is the fence's to choose in Mermaid, but here
 * it can hide a slice and its legend, so it must not reach the picture.
 *
 * The host `config` option cannot override it: Pele merges a fence's config
 * over the host's (`{ ...extra, ...pre.config }` in its core), so this
 * stripper removes it before Pele sees it. The stripper is best effort (YAML
 * is a large language); the guarantee is the output check in
 * `diagram-render.ts`: if any fence series declaration survives, the render
 * fails and the block shows its code.
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
 * is; an unclosed one is too, as rewriting it could eat the diagram. Those
 * reach the output check, which refuses the render if the palette appears.
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
    // for the output check rather than change what the rest of the config says.
    if (body.includes("}%%")) continue;
    out = out.slice(0, match.index!) + `%%{init:${body}}%%` + out.slice(match.index! + match[0].length);
  }
  return out;
}

interface FrontMatter {
  bodyStart: number;
  bodyEnd: number;
  /** Just past the closing fence line; directives before this are ignored (Pele removes the frontmatter first). */
  end: number;
}

/** Spaces, tabs, BOM, NBSP and the other whitespace YAML treats as indentation. */
const INDENT = "[^\\S\\r\\n]*";

/**
 * Pele's frontmatter is a `---` fence at the start (after whitespace that is
 * not a line break, which includes a BOM and NBSP) and the last `---` line
 * after it (`matchFrontMatter` in its preprocess).
 */
function frontMatter(source: string): FrontMatter | null {
  const open = new RegExp(`^(${INDENT})---${INDENT}(?:\\n|$)`).exec(source);
  if (!open) return null;
  const bodyStart = open[0].length;
  const closing = new RegExp(`^${escapeRegExp(open[1]!)}---${INDENT}(?:\\n|$)`, "gm");
  closing.lastIndex = bodyStart;
  let last: RegExpExecArray | null = null;
  for (let match = closing.exec(source); match !== null; match = closing.exec(source)) last = match;
  if (last === null) return null;
  return { bodyStart, bodyEnd: last.index, end: last.index + last[0].length };
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function indentation(line: string): number {
  return /^[^\S\r\n]*/.exec(line)![0].length;
}

const BLOCK_KEY = /^([^\S\r\n]*)(?:"((?:[^"\\]|\\.)*)"|'((?:[^']|'')*)'|([^\s:][^:]*?))[^\S\r\n]*:/;
const CONFIG_LINE = /^([^\S\r\n]*)(?:"config"|'config'|config)[^\S\r\n]*:[^\S\r\n]*(?:#.*)?$/;

/** A YAML key at the start of a line, with its indentation. */
function blockKey(line: string): { indent: number; key: string } | null {
  const match = BLOCK_KEY.exec(line);
  if (!match) return null;
  const raw = match[2] ?? match[3] ?? match[4] ?? "";
  return {
    indent: match[1]!.length,
    key: match[2] !== undefined ? decodeYamlKey(raw, '"') : match[3] !== undefined ? decodeYamlKey(raw, "'") : raw.trim(),
  };
}

/** YAML double-quoted escapes, enough for a key Pele would read as `themeVariables`. */
function decodeYamlKey(raw: string, quote: '"' | "'"): string {
  if (quote === "'") return raw.replace(/''/g, "'");
  return raw.replace(/\\(x[0-9a-fA-F]{2}|u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|.)/g, (_whole, escape: string) => {
    const kind = escape[0]!;
    if (kind === "x" || kind === "u" || kind === "U") return String.fromCodePoint(Number.parseInt(escape.slice(1), 16));
    if (kind === "n") return "\n";
    if (kind === "t") return "\t";
    if (kind === "r") return "\r";
    if (kind === "0") return "\0";
    return escape;
  });
}

/**
 * Remove every `themeVariables` block inside a `config:` block, then any
 * `themeVariables` key in a flow map that is the config's value (on the same
 * line or the next). A `title: |` block outside `config` is not touched.
 */
function stripBlockConfig(lines: string[]): string[] {
  for (let i = 0; i < lines.length; i++) {
    const config = CONFIG_LINE.exec(lines[i]!);
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
    // A flow map may be the config's value on the next line:
    //   config:
    //     {themeVariables: {...}}
    const section = lines.slice(i + 1, end).join("\n");
    const cleaned = removeFlowKey(section, THEME);
    if (cleaned !== section) {
      const next = cleaned.split("\n");
      lines.splice(i + 1, end - (i + 1), ...next);
      end = i + 1 + next.length;
    }
  }
  return lines;
}

/** Remove `themeVariables` keys from a YAML flow mapping (`config: { … }`). */
function stripFlowConfig(text: string): string {
  const starts: number[] = [];
  const config = new RegExp(`(^|\\n)(${INDENT})(?:"config"|'config'|config)${INDENT}:${INDENT}\\{`, "gm");
  for (const match of text.matchAll(config)) starts.push(match.index! + match[0].length - 1);
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
  let previous = "{";
  for (let i = from; i < text.length; i++) {
    const char = text[i]!;
    if (char === "#" && (i === 0 || /[\s{,]/.test(text[i - 1]!))) {
      // A YAML comment, including one containing a colon; skip to its end.
      const newline = text.indexOf("\n", i);
      if (newline < 0) return null;
      i = newline;
      continue;
    }
    if (char === '"' || char === "'") {
      const end = quotedEnd(text, i);
      if (end < 0) return null;
      if ((previous === "{" || previous === ",") && decodeYamlKey(text.slice(i + 1, end), char) === key) {
        const colon = skipFlowSpace(text, end + 1);
        if (text[colon] === ":") return { start: i, end: flowValueEnd(text, colon + 1) };
      }
      // A quoted key or value: neither its contents nor its punctuation is
      // structure to this scan.
      i = end;
      previous = char;
      continue;
    }
    if (char === "{" || char === "}") {
      previous = char;
      continue;
    }
    if (char === ",") {
      previous = ",";
      continue;
    }
    if (/\s/.test(char)) continue;
    if ((previous === "{" || previous === ",") && text.startsWith(key, i) && !/[\w-]/.test(text[i + key.length] ?? "")) {
      const colon = skipFlowSpace(text, i + key.length);
      if (text[colon] === ":") return { start: i, end: flowValueEnd(text, colon + 1) };
    }
    previous = char;
  }
  return null;
}

/** The end quote of a YAML string starting at `start`, or -1. */
function quotedEnd(text: string, start: number): number {
  const quote = text[start]!;
  for (let i = start + 1; i < text.length; i++) {
    const char = text[i]!;
    if (quote === '"') {
      if (char === "\\") {
        i++;
      } else if (char === '"') {
        return i;
      }
    } else if (char === "'") {
      if (text[i + 1] === "'") i++;
      else return i;
    }
  }
  return -1;
}

function skipFlowSpace(text: string, at: number): number {
  while (at < text.length && /[^\S\r\n]/.test(text[at]!)) at++;
  return at;
}

/** The index just past a flow value starting at `from`. */
function flowValueEnd(text: string, from: number): number {
  let at = skipFlowSpace(text, from);
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
  if (front === null) return stripDirectives(out);
  const body = out.slice(front.bodyStart, front.bodyEnd);
  let cleaned = stripFlowConfig(stripBlockConfig(body.split("\n")).join("\n"));
  // The block remover takes blank lines that followed the removed key with
  // it; the newline before the closing fence is structural.
  if (body.endsWith("\n") && !cleaned.endsWith("\n")) cleaned += "\n";
  return (
    out.slice(0, front.bodyStart) +
    cleaned +
    out.slice(front.bodyEnd, front.end) +
    // Pele removes the frontmatter before it reads directives; a `%%{` in a
    // frontmatter title must not swallow the diagram's real init directive.
    stripDirectives(out.slice(front.end))
  );
}
