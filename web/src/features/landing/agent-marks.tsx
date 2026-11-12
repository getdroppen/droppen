import { cn } from "@/lib/cn";

/*
 * Marks come from Lobe Icons (`@lobehub/icons-static-svg`, MIT, `icons.lobehub.com`)
 * and belong to their makers. `tone: "ink"` draws in the page's ink — dark on the
 * light sheet, light on the dark; `brand` keeps the colours of the library's
 * `-color.svg` file.
 */

type AgentIcon = {
  slug: string;
  tone: "ink" | "brand";
};

export type Agent = {
  name: string;
  icon: AgentIcon;
};

/**
 * The agents a terminal can run, and that Droppen's ACP settings can be
 * pointed at. Ordered so the two agents the headline names come first.
 */
export const AGENTS: readonly Agent[] = [
  { name: "Claude Code", icon: { slug: "claudecode", tone: "brand" } },
  { name: "Codex", icon: { slug: "codex", tone: "ink" } },
  { name: "Gemini CLI", icon: { slug: "geminicli", tone: "brand" } },
  { name: "Google Antigravity", icon: { slug: "antigravity", tone: "brand" } },
  { name: "Cursor", icon: { slug: "cursor", tone: "ink" } },
  { name: "GitHub Copilot", icon: { slug: "copilot", tone: "brand" } },
  { name: "Windsurf", icon: { slug: "windsurf", tone: "ink" } },
  { name: "Cline", icon: { slug: "cline", tone: "ink" } },
  { name: "Roo Code", icon: { slug: "roocode", tone: "ink" } },
  { name: "Kilo Code", icon: { slug: "kilocode", tone: "ink" } },
  { name: "OpenCode", icon: { slug: "opencode", tone: "ink" } },
  { name: "Goose", icon: { slug: "goose", tone: "ink" } },
  { name: "Junie", icon: { slug: "junie", tone: "brand" } },
  { name: "Kiro", icon: { slug: "kiro", tone: "brand" } },
  { name: "Qwen Code", icon: { slug: "qwen", tone: "brand" } },
  { name: "Kimi CLI", icon: { slug: "kimi", tone: "brand" } },
  { name: "MiniMax Code", icon: { slug: "minimax", tone: "brand" } },
  { name: "Z.ai GLM Agent", icon: { slug: "zhipu", tone: "brand" } },
  { name: "Qoder", icon: { slug: "qoder", tone: "brand" } },
  { name: "Trae", icon: { slug: "trae", tone: "brand" } },
  { name: "OpenHands", icon: { slug: "openhands", tone: "brand" } },
  { name: "CodeBuddy", icon: { slug: "codebuddy", tone: "brand" } },
  { name: "Amp", icon: { slug: "amp", tone: "brand" } },
  { name: "Devin", icon: { slug: "devin", tone: "brand" } },
  { name: "Poolside", icon: { slug: "poolside", tone: "brand" } },
  { name: "Zencoder", icon: { slug: "zencoder", tone: "brand" } },
  { name: "OpenClaw", icon: { slug: "openclaw", tone: "brand" } },
  { name: "Mistral Vibe", icon: { slug: "mistral", tone: "brand" } },
  { name: "Command Code", icon: { slug: "commandcode", tone: "ink" } },
  { name: "CodeFlicker", icon: { slug: "codeflicker", tone: "brand" } },
  { name: "CodeGeeX", icon: { slug: "codegeex", tone: "brand" } },
  { name: "Pi", icon: { slug: "pi", tone: "ink" } },
  { name: "Hermes Agent", icon: { slug: "hermesagent", tone: "ink" } },
  { name: "WorkBuddy", icon: { slug: "workbuddy", tone: "brand" } },
  { name: "Tiangong", icon: { slug: "tiangong", tone: "brand" } },
  { name: "YouMind", icon: { slug: "youmind", tone: "ink" } },
  { name: "Zero One", icon: { slug: "zeroone", tone: "brand" } },
];

/**
 * The library files the ring draws, read at build time. The set is spelled out
 * rather than globbed: a glob over the package would pull all ~1000 marks into
 * the landing chunk. Each entry names one file of `@lobehub/icons-static-svg`
 * and imports it raw, so Vite bundles exactly these. An agent added to `AGENTS`
 * needs its file here too, or the ring quietly leaves it out.
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

/**
 * The wall's shape at each width, and what `index.css` gives its grid: a phone
 * four columns over five, six from 30rem, ten from 40rem. The rows stay five:
 * a wider window has more room across, so it adds columns rather than stretching
 * cells; past ten columns the cells are already near square on the 1360px sheet,
 * so the wall stops widening there. The marks shrink as the columns multiply.
 */
const GRID = {
  phone: { columns: 4, rows: 5 },
  mid: { columns: 6, rows: 5 },
  wide: { columns: 10, rows: 5 },
} as const;

/** The cell's display at the breakpoints: 4 columns below 30rem, 6 to 40rem, 10 above. */
function cellClass(column: number): string {
  if (column >= GRID.wide.columns) return "hidden";
  if (column >= GRID.mid.columns) return "hidden sm:flex";
  if (column >= GRID.phone.columns) return "hidden min-[30rem]:flex";
  return "";
}

/** One drawn agent: its mark and whether it is one of the first eight the wall features. */
export type AgentPiece = {
  name: string;
  icon: AgentIcon;
  index: number;
  mainstream: boolean;
  mark: { viewBox: string; body: string };
};

/**
 * Every agent whose mark is bundled, in the wall's order. Exported for the
 * share card (`web/dev/og-card.tsx`), which draws the same marks at its own
 * fixed scale rather than the responsive wall below.
 */
export function agentPieces(): AgentPiece[] {
  const files = markFiles(AGENTS.map((agent) => agent.icon));
  return AGENTS.map((agent, index) => {
    const svg = files.get(`${agent.icon.slug}:${agent.icon.tone}`);
    return svg ? { ...agent, index, mainstream: index < 8, mark: innerMarkup(svg) } : null;
  }).filter((piece) => piece !== null);
}

/** The piece's own SVG, sized by its parent (`size-*`). */
export function AgentGlyph({ piece, className }: { piece: AgentPiece; className?: string }) {
  return (
    <svg
      aria-hidden="true"
      focusable="false"
      viewBox={piece.mark.viewBox}
      // An ink mark takes the page's ink in both themes (`currentColor`). No filter: in the
      // dark theme the mark is already white, and inverting it would turn it black.
      className={cn("block size-full", piece.icon?.tone === "ink" && "[&_path]:fill-current", className)}
      dangerouslySetInnerHTML={{ __html: piece.mark.body }}
    />
  );
}

/** The wall every width draws from, its columns closed by `index.css` at 30rem and 40rem. */
export function AgentBurst() {
  const pieces = agentPieces();
  // Column-major over five rows: the phone's first twenty marks read as a wall, and
  // the same order fills the ten-column rows above it.
  const grid = pieces.length === 0 ? [] : Array.from({ length: GRID.wide.rows }, (_, row) => Array.from({ length: GRID.wide.columns }, (_, column) => pieces[(column * GRID.wide.rows + row) % pieces.length]!));

  return (
    <div aria-hidden="true" data-allow-color="" className="pointer-events-none absolute inset-0">
      <div data-burst="" className="relative flex h-full flex-col overflow-hidden">
        {grid.map((row, rowIndex) => (
          <div
            key={rowIndex}
            className="grid min-h-0 flex-1 border-b border-border last:border-b-0"
            style={{ gridTemplateRows: "1fr" }}
          >
            {row.map((piece, column) => (
              <span
                key={`${piece.name}-${column}`}
                data-cell=""
                className={cn(
                  "flex items-center justify-center border-r border-border last:border-r-0",
                  cellClass(column),
                )}
              >
                <span
                  className={cn(
                    "block shrink-0",
                    piece.mainstream
                      ? "size-8 opacity-90 min-[30rem]:size-10 sm:size-8 md:size-10 lg:size-12 xl:size-16"
                      : "size-5 opacity-55 min-[30rem]:size-6 sm:size-5 md:size-[26px] lg:size-8 xl:size-11 dark:opacity-65",
                  )}
                  style={{ transform: `rotate(${((piece.index * 7) % 13) - 6}deg)` }}
                >
                  <AgentGlyph piece={piece} />
                </span>
              </span>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
