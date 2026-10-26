import { createContext, useContext, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";
import { DownloadLink } from "@/components/host-file";
import { LightboxImage } from "@/components/image-lightbox";
import { MermaidBlock } from "@/components/mermaid-block";
import { ImageFallback } from "@/components/source-image";
import { cn } from "@/lib/cn";
import { localFilePath } from "@/lib/fs-url";
import { imageSource } from "@/lib/image-source";

/** A link the agent wrote to a file on the host, turned into a real download. */
function FileLink({ path, children }: { path: string; children: React.ReactNode }) {
  return (
    <DownloadLink
      path={path}
      name={path.split("/").pop() || undefined}
      title={path}
      className="text-fg underline decoration-border-strong underline-offset-[3px] hover:decoration-fg"
    >
      {children}
    </DownloadLink>
  );
}

/**
 * Whether an image is the content of a link (`[![badge](…)](…)`), where a
 * thumbnail button would fight the link for the click and a fallback card's
 * own anchor would be nested inside it.
 */
const InLink = createContext(false);

/**
 * A picture the agent put in its reply: a bounded thumbnail that opens full
 * size, from the host, through the GitHub proxy or straight from the web —
 * see `lib/image-source.ts` for which. Only `![]()` gets here; a plain link to
 * an image file stays a link.
 */
function MarkdownImage({ src, alt }: { src?: string; alt: string }) {
  const inLink = useContext(InLink);
  const source = imageSource(src);
  if (!source) return <ImageFallback alt={alt} reason={src || null} />;
  return (
    <LightboxImage
      source={source}
      alt={alt}
      caption={inLink ? undefined : alt || undefined}
      zoom={!inLink}
      thumbClassName="max-h-60 max-w-[min(24rem,100%)] rounded border border-border object-contain"
    />
  );
}

/**
 * react-markdown's own sanitiser, plus `file:`: it drops every protocol it
 * does not know to an empty string, and `file:///…` is a host path that
 * `localFilePath` turns into the same download route as `/…`.
 */
function urlTransform(url: string): string {
  return /^file:/i.test(url) ? url : defaultUrlTransform(url);
}

/**
 * Whether a ```mermaid fence in a message is drawn: the `diagrams` pref of the
 * device whose transcript this is, provided by the view that read it. Off
 * outside one.
 */
export const DiagramRendering = createContext(false);

/**
 * The value a view provides for `DiagramRendering`, from its device's prefs:
 * drawn unless they say `source` (a server from before the pref sends none),
 * but not before they have been read — a reader who chose the source must not
 * have Mermaid run on the agent's text while the prefs load, or when they
 * cannot be read at all.
 */
export function drawsDiagrams(prefs: { diagrams?: "rendered" | "source" } | undefined): boolean {
  return prefs !== undefined && prefs.diagrams !== "source";
}

/** The little of hast's shape this file reads: a fenced block is a `pre` with one `code`. */
interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: { className?: unknown };
  children?: HastNode[];
}

function hastText(node: HastNode): string {
  if (node.type === "text") return node.value ?? "";
  return (node.children ?? []).map(hastText).join("");
}

/**
 * The source of a ```mermaid fence, taken from react-markdown's syntax tree,
 * or null for every other block (and for inline code, which never reaches a
 * `pre`). Matching the fence's info string here, rather than in `code`, is what
 * lets the whole `pre` be replaced by the diagram.
 */
function mermaidSourceOf(node: unknown): string | null {
  if (node === null || typeof node !== "object") return null;
  const pre = node as HastNode;
  const code = (pre.children ?? []).find((child) => child.type === "element" && child.tagName === "code");
  if (!code) return null;
  const classes = code.properties?.className;
  const names = Array.isArray(classes) ? classes.map(String) : typeof classes === "string" ? classes.split(/\s+/) : [];
  if (!names.some((name) => /^language-mermaid$/i.test(name))) return null;
  return hastText(code);
}

/**
 * A fenced block. Every block renders exactly as it always has; a ```mermaid
 * fence is drawn instead when diagrams are on and Mermaid can read it, and is
 * the same code block in every other case.
 */
function MarkdownPre({ children, node }: React.ComponentProps<"pre"> & { node?: unknown }) {
  const renderDiagrams = useContext(DiagramRendering);
  const source = mermaidSourceOf(node);
  const code = <pre className="my-1.5 overflow-x-auto border border-border bg-raised px-2.5 py-2">{children}</pre>;
  if (!renderDiagrams || source === null) return code;
  return <MermaidBlock code={source} fallback={code} />;
}

const components: React.ComponentProps<typeof ReactMarkdown>["components"] = {
  h1: ({ children }) => <h1 className="mt-3 mb-1 text-sm font-semibold first:mt-0">{children}</h1>,
  h2: ({ children }) => <h2 className="mt-3 mb-1 text-[13px] font-semibold first:mt-0">{children}</h2>,
  h3: ({ children }) => <h3 className="mt-2 mb-1 text-xs font-semibold first:mt-0">{children}</h3>,
  h4: ({ children }) => <h4 className="mt-2 mb-0.5 text-xs font-semibold first:mt-0">{children}</h4>,
  p: ({ children }) => <p className="my-1 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => <ul className="my-1 list-[square] space-y-0.5 pl-4 marker:text-faint">{children}</ul>,
  ol: ({ children }) => <ol className="my-1 list-decimal space-y-0.5 pl-5 marker:font-mono marker:text-faint">{children}</ol>,
  li: ({ children }) => <li className="[&>p]:my-0">{children}</li>,
  a: ({ href, children }) => {
    const path = localFilePath(href);
    if (path) {
      return (
        <FileLink path={path}>
          <InLink.Provider value>{children}</InLink.Provider>
        </FileLink>
      );
    }
    return (
      <a href={href} target="_blank" rel="noreferrer" className="text-fg underline decoration-border-strong underline-offset-[3px] hover:decoration-fg">
        <InLink.Provider value>{children}</InLink.Provider>
      </a>
    );
  },
  img: ({ src, alt }) => <MarkdownImage src={typeof src === "string" ? src : undefined} alt={alt ?? ""} />,
  blockquote: ({ children }) => (
    <blockquote className="my-1 border-l border-border-strong pl-3 text-muted">{children}</blockquote>
  ),
  hr: () => <hr className="my-2 border-border" />,
  table: ({ children }) => (
    <div className="my-1.5 overflow-x-auto">
      <table className="w-max min-w-48 border-collapse text-2xs">{children}</table>
    </div>
  ),
  th: ({ children }) => (
    <th className="border border-border bg-raised px-1.5 py-0.5 text-left font-semibold">{children}</th>
  ),
  td: ({ children }) => <td className="border border-border px-1.5 py-0.5 align-top">{children}</td>,
  code: ({ className, children }) => {
    const block = /language-/.test(className ?? "");
    if (block) {
      return <code className={cn("block font-mono text-2xs leading-[18px]", className)}>{children}</code>;
    }
    return <code className="bg-raised px-1 py-px font-mono text-[0.9em] text-fg shadow-[inset_0_0_0_1px_var(--color-border)]">{children}</code>;
  },
  pre: MarkdownPre,
};

/**
 * Whether LaTeX in a message is typeset: the `math` pref of the device whose
 * transcript this is, provided by the view that read it. Off outside one.
 */
export const MathRendering = createContext(false);

type MathKit = typeof import("@/components/markdown-math");

/** Anything that could open a formula: a `$`, `\(` or `\[`. A message without one never loads KaTeX. */
const MAY_HAVE_MATH = /\$|\\[([]/;

let mathKit: MathKit | undefined;
let mathLoad: Promise<MathKit | undefined> | undefined;

/**
 * Set the math kit directly, without the fetch (and its stylesheet), which is
 * what `loadMath` does once it has both. Exported for the render tests, which
 * have no bundler to load a chunk or a stylesheet with.
 */
export function installMath(kit: MathKit) {
  mathKit = kit;
  mathLoad = Promise.resolve(kit);
}

/**
 * KaTeX and its stylesheet, fetched once for the page. A failed fetch (a
 * chunk gone after a deploy, the network down) leaves messages as written and
 * is tried again by the next message that asks.
 */
function loadMath(): Promise<MathKit | undefined> {
  mathLoad ??= Promise.all([import("@/components/markdown-math"), import("katex/dist/katex.min.css")]).then(
    ([kit]) => {
      installMath(kit);
      return kit;
    },
    () => {
      mathLoad = undefined;
      return undefined;
    },
  );
  return mathLoad;
}

/** The math kit when `text` should be typeset and it has loaded; until then, nothing. */
function useMathKit(text: string): MathKit | undefined {
  const wanted = useContext(MathRendering) && MAY_HAVE_MATH.test(text);
  const [kit, setKit] = useState(mathKit);
  useEffect(() => {
    if (!wanted || kit) return;
    let live = true;
    void loadMath().then((loaded) => {
      if (live && loaded) setKit(loaded);
    });
    return () => {
      live = false;
    };
  }, [wanted, kit]);
  return wanted ? (kit ?? mathKit) : undefined;
}

const PLAIN = [remarkGfm];

/**
 * Copy the TeX of the formula a click (or Enter or Space on a focused one)
 * landed on, then say so on the formula for a moment. One handler for the
 * whole message rather than one per formula: the formulas are KaTeX's markup,
 * drawn from hast, with nowhere to hang their own. Not a click that ends a
 * text selection: that one is someone selecting.
 */
function copyFormula(event: React.MouseEvent | React.KeyboardEvent, copied: string) {
  const formula = (event.target as Element).closest?.("[data-tex]");
  if (!(formula instanceof HTMLElement) || !event.currentTarget.contains(formula)) return;
  if (event.type === "keydown") {
    const { key } = event as React.KeyboardEvent;
    if (event.target !== formula || (key !== "Enter" && key !== " ")) return;
  } else if (window.getSelection()?.toString()) {
    return;
  }
  // A formula in a link's text copies rather than follows the link.
  event.preventDefault();
  navigator.clipboard?.writeText(formula.dataset.tex ?? "").then(
    () => {
      formula.dataset.copied = copied;
      setTimeout(() => delete formula.dataset.copied, 1500);
    },
    () => {
      // Clipboard refused (no permission, or not a secure context): nothing
      // was copied, so nothing says it was.
    },
  );
}

export function Markdown({ text, className }: { text: string; className?: string }) {
  const { t } = useTranslation();
  const math = useMathKit(text);
  const copied = t("copied");
  return (
    <div
      className={cn("min-w-0 text-[13px] leading-[21px] break-words max-md:text-sm max-md:leading-[22px]", className)}
      onClick={math ? (event) => copyFormula(event, copied) : undefined}
      onKeyDown={math ? (event) => copyFormula(event, copied) : undefined}
    >
      <ReactMarkdown
        remarkPlugins={math ? [remarkGfm, ...math.remarkPlugins] : PLAIN}
        rehypePlugins={math ? [[math.rehypeTypeset, { copyTitle: t("click to copy the TeX") }]] : undefined}
        components={components}
        urlTransform={urlTransform}
      >
        {math ? math.prepareMath(text) : text}
      </ReactMarkdown>
    </div>
  );
}
