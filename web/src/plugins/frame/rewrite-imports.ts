/**
 * Pure-JS ES module relative-import rewriter for sandboxed `blob:` frames
 * (`docs/plugins/cloud.md` §3).
 *
 * A `blob:` URL has an opaque base and cannot resolve relative specifiers like
 * `./chunk.js` or `../shared.js`. Before turning each plugin JS/MJS file into a
 * `blob:` URL, every static `import`/`export ... from` and dynamic `import(...)`
 * carrying a relative string specifier is rewritten to a bare specifier under
 * `@droppen-pkg/<normalized-path>`, which the frame's runtime import map maps
 * to that file's `blob:` URL (handling cyclic chunks cleanly).
 *
 * Zero WASM (`'wasm-unsafe-eval'` is forbidden in the frame's CSP) and zero
 * runtime dependencies.
 */

export const PKG_SPECIFIER_PREFIX = "@droppen-pkg/";

/** Normalize a `/`-separated relative path inside the package. */
export function resolvePackagePath(fromFile: string, specifier: string): string {
  const clean = specifier.split(/[?#]/, 1)[0] ?? specifier;
  const baseParts = fromFile.split("/").slice(0, -1);
  const relParts = clean.split("/");
  const out = [...baseParts];
  for (const part of relParts) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (out.length > 0) out.pop();
    } else {
      out.push(part);
    }
  }
  return out.join("/");
}

function isRelative(specifier: string): boolean {
  return specifier.startsWith("./") || specifier.startsWith("../");
}

function isIdentChar(ch: string): boolean {
  return (
    (ch >= "a" && ch <= "z") ||
    (ch >= "A" && ch <= "Z") ||
    (ch >= "0" && ch <= "9") ||
    ch === "_" ||
    ch === "$"
  );
}

function skipWsAndComments(src: string, pos: number): number {
  const len = src.length;
  while (pos < len) {
    const ch = src[pos]!;
    if (ch === " " || ch === "\t" || ch === "\r" || ch === "\n" || ch === "\uFEFF") {
      pos++;
      continue;
    }
    if (ch === "/" && src[pos + 1] === "/") {
      pos += 2;
      while (pos < len && src[pos] !== "\n") pos++;
      continue;
    }
    if (ch === "/" && src[pos + 1] === "*") {
      pos += 2;
      while (pos + 1 < len && !(src[pos] === "*" && src[pos + 1] === "/")) pos++;
      pos = Math.min(len, pos + 2);
      continue;
    }
    break;
  }
  return pos;
}

function readStringLiteral(src: string, start: number): { value: string; end: number; quote: string } | null {
  const quote = src[start];
  if (quote !== '"' && quote !== "'") return null;
  let i = start + 1;
  let value = "";
  while (i < src.length) {
    const ch = src[i]!;
    if (ch === "\\") {
      if (i + 1 >= src.length) return null;
      value += src[i + 1]!;
      i += 2;
      continue;
    }
    if (ch === quote) return { value, end: i + 1, quote };
    if (ch === "\n" || ch === "\r") return null;
    value += ch;
    i++;
  }
  return null;
}

function skipTemplateLiteral(src: string, start: number): number {
  let i = start + 1;
  const len = src.length;
  while (i < len) {
    const ch = src[i]!;
    if (ch === "\\") {
      i += 2;
      continue;
    }
    if (ch === "`") return i + 1;
    if (ch === "$" && src[i + 1] === "{") {
      i += 2;
      let depth = 1;
      while (i < len && depth > 0) {
        const c = src[i]!;
        if (c === "{") {
          depth++;
          i++;
        } else if (c === "}") {
          depth--;
          i++;
        } else if (c === "'" || c === '"') {
          const s = readStringLiteral(src, i);
          i = s ? s.end : i + 1;
        } else if (c === "`") {
          i = skipTemplateLiteral(src, i);
        } else if (c === "/" && (src[i + 1] === "/" || src[i + 1] === "*")) {
          i = skipWsAndComments(src, i);
        } else {
          i++;
        }
      }
      continue;
    }
    i++;
  }
  return len;
}

function skipRegexLiteral(src: string, start: number): number {
  let i = start + 1;
  let inClass = false;
  while (i < src.length) {
    const ch = src[i]!;
    if (ch === "\\") {
      i += 2;
      continue;
    }
    if (ch === "[") inClass = true;
    else if (ch === "]") inClass = false;
    else if (ch === "/" && !inClass) {
      i++;
      while (i < src.length && isIdentChar(src[i]!)) i++;
      return i;
    } else if (ch === "\n" || ch === "\r") {
      return start + 1;
    }
    i++;
  }
  return src.length;
}

const REGEX_PREV_KEYWORDS = new Set([
  "return",
  "case",
  "throw",
  "in",
  "of",
  "typeof",
  "void",
  "delete",
  "new",
  "else",
  "do",
  "yield",
  "await",
]);

function canStartRegex(prevToken: string): boolean {
  if (!prevToken) return true;
  if (REGEX_PREV_KEYWORDS.has(prevToken)) return true;
  const last = prevToken[prevToken.length - 1]!;
  return "([{:;,=!?&|+-*%^~<>".includes(last);
}

/**
 * Rewrite every relative ES module specifier in `source` (from `fromFile`) to
 * `@droppen-pkg/<resolved-path>` (or whatever `mapSpecifier` returns).
 */
export function rewriteRelativeImports(
  source: string,
  fromFile: string,
  mapSpecifier: (resolvedPath: string) => string = (resolved) => `${PKG_SPECIFIER_PREFIX}${resolved}`,
): string {
  const edits: { start: number; end: number; replacement: string }[] = [];
  const len = source.length;
  let i = 0;
  let prevToken = "";

  const maybeRewriteStringAt = (pos: number): number | null => {
    const lit = readStringLiteral(source, pos);
    if (!lit) return null;
    if (isRelative(lit.value)) {
      const resolved = resolvePackagePath(fromFile, lit.value);
      const mapped = mapSpecifier(resolved);
      edits.push({ start: pos, end: lit.end, replacement: JSON.stringify(mapped) });
    }
    return lit.end;
  };

  while (i < len) {
    const ch = source[i]!;
    if (ch === " " || ch === "\t" || ch === "\r" || ch === "\n" || ch === "\uFEFF") {
      i++;
      continue;
    }
    if (ch === "/" && (source[i + 1] === "/" || source[i + 1] === "*")) {
      i = skipWsAndComments(source, i);
      continue;
    }
    if (ch === "'" || ch === '"') {
      const lit = readStringLiteral(source, i);
      i = lit ? lit.end : i + 1;
      prevToken = '"';
      continue;
    }
    if (ch === "`") {
      i = skipTemplateLiteral(source, i);
      prevToken = "`";
      continue;
    }
    if (ch === "/" && canStartRegex(prevToken)) {
      i = skipRegexLiteral(source, i);
      prevToken = "/";
      continue;
    }

    if (isIdentChar(ch)) {
      const start = i;
      while (i < len && isIdentChar(source[i]!)) i++;
      const word = source.slice(start, i);

      if ((word === "import" || word === "export") && prevToken !== ".") {
        const afterWord = skipWsAndComments(source, i);
        const nextCh = source[afterWord];

        // Dynamic import: `import("./chunk.js")`
        if (word === "import" && nextCh === "(") {
          const argStart = skipWsAndComments(source, afterWord + 1);
          const end = maybeRewriteStringAt(argStart);
          if (end !== null) {
            i = end;
            prevToken = '"';
            continue;
          }
        }

        // Side-effect import: `import "./style.css"`
        if (word === "import" && (nextCh === '"' || nextCh === "'")) {
          const end = maybeRewriteStringAt(afterWord);
          if (end !== null) {
            i = end;
            prevToken = '"';
            continue;
          }
        }

        // Static `import ... from "./x"` or `export ... from "./x"`
        // Only scan ahead when it is not `import.meta` and not `export default`/`function`/`const`/etc.
        if (nextCh !== "." && nextCh !== "(") {
          let scan = afterWord;
          let braceDepth = 0;
          let foundFrom = false;
          while (scan < len) {
            scan = skipWsAndComments(source, scan);
            const c = source[scan];
            if (!c) break;
            if (c === "{") {
              braceDepth++;
              scan++;
              continue;
            }
            if (c === "}") {
              if (braceDepth > 0) braceDepth--;
              scan++;
              continue;
            }
            if (c === ";" || (braceDepth === 0 && c === "=")) break;
            if (c === "'" || c === '"') {
              const lit = readStringLiteral(source, scan);
              scan = lit ? lit.end : scan + 1;
              continue;
            }
            if (isIdentChar(c)) {
              const wStart = scan;
              while (scan < len && isIdentChar(source[scan]!)) scan++;
              const w = source.slice(wStart, scan);
              if (braceDepth === 0 && w === "from") {
                const specStart = skipWsAndComments(source, scan);
                const end = maybeRewriteStringAt(specStart);
                if (end !== null) {
                  i = end;
                  prevToken = '"';
                  foundFrom = true;
                }
                break;
              }
              if (
                braceDepth === 0 &&
                word === "export" &&
                (w === "const" || w === "let" || w === "var" || w === "function" || w === "class" || w === "async" || w === "default")
              ) {
                break;
              }
              continue;
            }
            scan++;
          }
          if (foundFrom) continue;
        }
      }

      prevToken = word;
      continue;
    }

    prevToken = ch;
    i++;
  }

  if (edits.length === 0) return source;
  let out = "";
  let cursor = 0;
  for (const edit of edits) {
    out += source.slice(cursor, edit.start) + edit.replacement;
    cursor = edit.end;
  }
  out += source.slice(cursor);
  return out;
}
