import type { CSSProperties } from "react";
import { cn } from "@/lib/cn";
import { AGENTS, type AgentIcon } from "./agent-marks";

/**
 * The library files the ring draws, read at build time. The set is spelled out
 * rather than globbed: a glob over the package would pull all ~1000 marks into
 * the landing chunk. Each entry names one file of `@lobehub/icons-static-svg`
 * and imports it raw, so Vite bundles exactly these.
 *
 * Vite replaces the call at build time; outside Vite (the render tests run the
 * sources directly) there is no such function and the field is empty. The map's
 * key is the file name, which `markFiles` matches `<slug>.svg` against.
 */
function bundledMarks(): Record<string, string> {
  try {
    return import.meta.glob<string>(
      [
        "../../../node_modules/@lobehub/icons-static-svg/icons/claudecode-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/codex.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/cursor.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/windsurf.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/opencode.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/goose.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/cline.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/roocode.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/kilocode.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/geminicli-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/antigravity-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/copilot-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/junie-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/kiro-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/qwen-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/kimi-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/minimax-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/zhipu-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/qoder-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/trae-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/openhands-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/codebuddy-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/amp-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/devin-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/poolside-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/zencoder-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/openclaw-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/mistral-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/codeflicker-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/codegeex-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/workbuddy-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/tiangong-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/zeroone-color.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/commandcode.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/pi.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/hermesagent.svg",
        "../../../node_modules/@lobehub/icons-static-svg/icons/youmind.svg",
      ],
      { query: "?raw", import: "default", eager: true },
    );
  } catch {
    return {};
  }
}

function markFiles(icons: readonly AgentIcon[], files: Record<string, string> = bundledMarks()): Map<string, string> {
  const byName = new Map(Object.entries(files).map(([path, svg]) => [path.slice(path.lastIndexOf("/") + 1), svg] as const));
  const marks = new Map<string, string>();
  for (const icon of icons) {
    for (const file of [`${icon.slug}-color.svg`, `${icon.slug}.svg`]) {
      const svg = byName.get(file);
      if (svg) {
        marks.set(`${icon.slug}:${icon.tone}`, svg);
        break;
      }
    }
  }
  return marks;
}

/** The `<svg>`'s own markup inside `svg`, with its `width`/`height` taken out so CSS sizes it. */
function innerMarkup(svg: string): { viewBox: string; body: string } {
  const open = /<svg\b([^>]*)>/.exec(svg);
  const viewBox = /viewBox="([^"]+)"/.exec(open?.[1] ?? "")?.[1] ?? "0 0 24 24";
  const body = svg
    .slice((open?.index ?? 0) + (open?.[0].length ?? 0))
    .replace(/<\/svg>\s*$/, "")
    // the file's own <title> is the tool's name, which the copy already says
    .replace(/<title\b[^>]*>[\s\S]*?<\/title>/g, "")
    .trim();
  return { viewBox, body };
}

function cellShown(grid: { columns: number; rows: number }, row: number, column: number): boolean {
  return column < grid.columns && row < grid.rows;
}

/** The grid a wide window draws, and the one a phone has (`index.css` sets the same pair for CSS). */
const GRID = { narrow: { columns: 4, rows: 5 }, wide: { columns: 12, rows: 4 } } as const;

/** The narrow wall is the wide wall's top-left corner; `index.css` closes its columns at `sm`. */
export function AgentBurst({ className }: { className?: string }) {
  const files = markFiles(AGENTS.map((agent) => agent.icon));
  const pieces = AGENTS.map((agent, index) => {
    const svg = files.get(`${agent.icon.slug}:${agent.icon.tone}`);
    return svg ? { ...agent, index, mainstream: index < 8, mark: innerMarkup(svg) } : null;
  }).filter((piece) => piece !== null);
  const grid = pieces.length === 0 ? [] : Array.from({ length: GRID.wide.rows }, (_, row) => Array.from({ length: GRID.wide.columns }, (_, column) => pieces[(column * GRID.wide.rows + row) % pieces.length]!));

  const body = (piece: (typeof pieces)[number]) => (
    <svg
      aria-hidden="true"
      focusable="false"
      viewBox={piece.mark.viewBox}
      // An ink mark takes the page's ink in both themes (`currentColor`). No filter: in the
      // dark theme the mark is already white, and inverting it would turn it black.
      className={cn("block size-full", piece.icon.tone === "ink" && "[&_path]:fill-current")}
      dangerouslySetInnerHTML={{ __html: piece.mark.body }}
    />
  );

  return (
    <div aria-hidden="true" data-allow-color="" className={cn("pointer-events-none absolute inset-0", className)}>
      <div
        data-burst=""
        className="relative flex h-full flex-col overflow-hidden"
        style={{ "--burst-columns": GRID.narrow.columns } as CSSProperties}
      >
        {grid.map((row, rowIndex) => (
          <div
            key={rowIndex}
            className="grid min-h-0 flex-1 border-b border-border last:border-b-0"
            style={{ gridTemplateRows: "1fr", gridTemplateColumns: `repeat(${GRID.wide.columns}, minmax(0, 1fr))` }}
          >
            {row.map((piece, column) => (
              <span
                key={`${piece.name}-${column}`}
                data-cell=""
                className={cn(
                  "flex items-center justify-center border-r border-border last:border-r-0",
                  !cellShown(GRID.narrow, rowIndex, column) && "max-sm:hidden",
                )}
              >
                <span
                  className={cn("block shrink-0", piece.mainstream ? "size-14 opacity-90 max-md:size-12" : "size-10 opacity-55 max-md:size-9 dark:opacity-65")}
                  style={{ transform: `rotate(${((piece.index * 7) % 13) - 6}deg)` }}
                >
                  {body(piece)}
                </span>
              </span>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
