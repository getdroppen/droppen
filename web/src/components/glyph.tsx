import { cn } from "@/lib/cn";

/**
 * A monochrome mark: path data on a view box, drawn in `currentColor` so it
 * takes the ink of whatever it sits in, the way `SourceIcon` draws an agent.
 */
export interface Glyph {
  /** Stable key: what the glyph is, for lists that memoize on it. */
  id: string;
  viewBox: string;
  paths: readonly string[];
}

/**
 * The glyph in an SVG asset, or `null` for a file that is anything more than
 * a view box and paths: an asset is drawn as path data, never as markup.
 */
export function parseSvgGlyph(id: string, svg: string): Glyph | null {
  const body = svg.replace(/<!--[\s\S]*?-->/g, "").trim();
  const root = /^<svg\b([^>]*)>([\s\S]*)<\/svg>$/.exec(body);
  if (!root) return null;
  const viewBox = /\bviewBox="([\d.\s-]+)"/.exec(root[1]!)?.[1];
  if (!viewBox) return null;
  const paths: string[] = [];
  const rest = root[2]!.replace(/<path\s+d="([^"]+)"\s*\/>/g, (_, d: string) => {
    paths.push(d);
    return "";
  });
  if (rest.trim() !== "" || paths.length === 0) return null;
  if (!paths.every((d) => /^[\d\s.,eE+\-MmLlHhVvCcSsQqTtAaZz]+$/.test(d))) return null;
  return { id, viewBox, paths };
}

export function GlyphIcon({ glyph, label, className }: { glyph: Glyph; label?: string; className?: string }) {
  return (
    <svg
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      viewBox={glyph.viewBox}
      fill="currentColor"
      className={cn("size-3 shrink-0", className)}
      data-glyph={glyph.id}
    >
      {label && <title>{label}</title>}
      {glyph.paths.map((d, index) => (
        <path key={index} d={d} />
      ))}
    </svg>
  );
}
