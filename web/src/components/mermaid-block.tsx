import { useEffect, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { useObjectUrl } from "@/components/source-image";
import {
  MERMAID_STABLE_MS,
  renderMermaidDiagram,
  type DiagramPicture,
  type DiagramTheme,
} from "@/lib/mermaid-render";

/**
 * Whether the page is on its dark sheet. The app's theme switch (`lib/theme.ts`,
 * `app/root-layout.tsx`) and a system change while the mode is `auto` both
 * land as the `dark` class on `<html>`, so watching that one attribute covers
 * every way a diagram's colours have to follow.
 */
function isDark(): boolean {
  return typeof document !== "undefined" && document.documentElement.classList.contains("dark");
}

function subscribeTheme(onChange: () => void): () => void {
  if (typeof document === "undefined" || typeof MutationObserver === "undefined") return () => undefined;
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  return () => observer.disconnect();
}

function useDiagramTheme(): DiagramTheme {
  return useSyncExternalStore(subscribeTheme, () => (isDark() ? "dark" : "light"), () => "light");
}

/** The last text that drew, and its picture. */
interface Drawn extends DiagramPicture {
  code: string;
}

/**
 * One ```mermaid fence, drawn.
 *
 * `fallback` is the plain code block the message has always shown. It stays on
 * screen while a new fence is still arriving, while the engine loads, and when
 * Mermaid refuses the text — no spinner, no error banner, just the source as
 * it was. Once a diagram is up, a small "source" control swaps that one block
 * back to its code (and "diagram" swaps it back again); the `diagrams` pref
 * (Settings → message rendering) is what turns them off for every block.
 *
 * The diagram is an `<img>` of the SVG, not the SVG itself (see
 * `lib/mermaid-render.ts`), at its natural size, scrolling sideways when it is
 * wider than the message.
 */
export function MermaidBlock({ code, fallback }: { code: string; fallback: React.ReactNode }) {
  const { t } = useTranslation();
  const theme = useDiagramTheme();
  const [showSource, setShowSource] = useState(false);
  const [stableCode, setStableCode] = useState(code);
  const [drawn, setDrawn] = useState<Drawn | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  // Released (revoked) once no block draws it: on a newer picture, or unmount.
  const url = useObjectUrl(drawn?.blob);

  // A streaming fence changes on every token; only a version that has stood
  // still for a moment is worth handing to the engine. A finished message's
  // fence starts out stable, so it is drawn straight away.
  useEffect(() => {
    if (code === stableCode) return;
    const timer = setTimeout(() => setStableCode(code), MERMAID_STABLE_MS);
    return () => clearTimeout(timer);
  }, [code, stableCode]);

  useEffect(() => {
    if (stableCode.trim() === "") return;
    const superseded = new AbortController();
    void renderMermaidDiagram(stableCode, theme, superseded.signal).then(
      (picture) => {
        if (superseded.signal.aborted) return;
        setDrawn({ code: stableCode, ...picture });
        setFailed(null);
      },
      () => {
        // Bad syntax, a half-written diagram, an engine that will not load:
        // all of them are the code block, and none of them is a message.
        if (!superseded.signal.aborted) setFailed(stableCode);
      },
    );
    return () => superseded.abort();
  }, [stableCode, theme]);

  // Once a block has a diagram it keeps it while the text moves on — a reply
  // still streaming in — and gives it up only for text that has stood still
  // and still does not draw.
  if (!drawn || !url || failed === code) return <>{fallback}</>;

  const action = showSource ? t("show diagram") : t("show source");
  return (
    <div data-mermaid-diagram="" className="my-1.5 min-w-0 border border-border bg-raised">
      <div className="flex h-6 items-center justify-end border-b border-border pr-1">
        <button
          type="button"
          onClick={() => setShowSource((shown) => !shown)}
          title={action}
          aria-label={action}
          className="flex h-5 items-center px-1.5 font-mono text-[10px] text-faint hover:bg-hover hover:text-fg"
        >
          {showSource ? t("diagram") : t("source")}
        </button>
      </div>
      {showSource ? (
        <pre className="overflow-x-auto px-2.5 py-2">
          <code className="block font-mono text-2xs leading-[18px]">{code}</code>
        </pre>
      ) : (
        <div data-mermaid-image="" className="overflow-x-auto p-2">
          <img
            src={url}
            width={drawn.width}
            height={drawn.height}
            alt={drawn.title ?? t("Mermaid diagram")}
            className="block max-w-none"
          />
        </div>
      )}
    </div>
  );
}
