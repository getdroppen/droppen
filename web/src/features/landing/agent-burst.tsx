import { cn } from "@/lib/cn";
import { AGENTS, type AgentIcon } from "./agent-marks";

/*
 * The agents Droppen follows, around the hero's action: the marks of the tools
 * whose sessions the app can run, drawn from Lobe Icons (`agent-marks.tsx`).
 *
 * The copy names Claude Code and Codex; the wall shows the wider set a
 * terminal can drive, as decoration only — the whole field is hidden from
 * assistive technology and takes no pointer.
 */

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

/** The files named by `icons`, keyed by `<slug>.svg`. */
export function markFiles(icons: readonly AgentIcon[], files: Record<string, string> = bundledMarks()): Map<string, string> {
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
export function innerMarkup(svg: string): { viewBox: string; body: string } {
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

/** True when an SVG has only neutral colours (or defaults to black). */
function neutralOnly(svg: string): boolean {
  const colors = svg.match(/#[\da-f]{8}|#[\da-f]{6}|#[\da-f]{4}|#[\da-f]{3}\b/gi) ?? [];
  return colors.every((color) => {
    let hex = color.slice(1);
    if (hex.length <= 4) hex = [...hex].map((digit) => digit + digit).join("");
    const channels = [0, 2, 4].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16));
    return Math.max(...channels) - Math.min(...channels) < 20;
  });
}

/**
 * Four regular rows of twelve cells make a wall rather than an orbit. Agents
 * repeat only to fill the final cells; equal-width columns fill the frame
 * exactly, without a partial copy or trailing rule at either end.
 */
export function AgentBurst({ className }: { className?: string }) {
  const files = markFiles(AGENTS.map((agent) => agent.icon));
  const pieces = AGENTS.map((agent, index) => {
    const svg = files.get(`${agent.icon.slug}:${agent.icon.tone}`);
    return svg ? { ...agent, index, mainstream: index < 8, monochrome: neutralOnly(svg), mark: innerMarkup(svg) } : null;
  }).filter((piece) => piece !== null);
  const rows = pieces.length === 0 ? [] : [0, 1, 2, 3].map((row) => Array.from({ length: 12 }, (_, column) => pieces[(column * 4 + row) % pieces.length]!));

  const body = (piece: (typeof pieces)[number]) => (
    <svg
      aria-hidden="true"
      focusable="false"
      viewBox={piece.mark.viewBox}
      className={cn("block size-full", (piece.icon.tone === "ink" || piece.monochrome) && "dark:invert")}
      dangerouslySetInnerHTML={{ __html: piece.mark.body }}
    />
  );

  return (
    <div aria-hidden="true" data-allow-color="" className={cn("pointer-events-none absolute inset-0", className)}>
      <div className="relative flex h-full flex-col overflow-hidden">
        {rows.map((row, rowIndex) => (
          <div
            key={rowIndex}
            className="grid min-h-0 flex-1 border-b border-border last:border-b-0"
            style={{ gridTemplateColumns: `repeat(${row.length}, minmax(0, 1fr))` }}
          >
            {row.map((piece, column) => (
              <span key={`${piece.name}-${column}`} className="flex items-center justify-center border-r border-border last:border-r-0">
                <span
                  className={cn(
                    "block shrink-0",
                    piece.mainstream ? "size-14 opacity-90 max-md:size-8" : "size-10 opacity-55 max-md:size-7 dark:opacity-65",
                  )}
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
