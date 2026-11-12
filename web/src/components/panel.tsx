import { CaretDoubleLeftIcon, CaretDoubleRightIcon } from "@phosphor-icons/react";
import { createContext, useContext, useState } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";
import { RegMark } from "@/components/reg-mark";

const PanelCollapseContext = createContext<{ toggle: () => void } | null>(null);

const WIDTH_MIN = 200;
const WIDTH_MAX = 600;
const WIDTH_DEFAULT = 304; // matches the old fixed w-76

function loadWidth(key: string | undefined): number {
  if (!key) return WIDTH_DEFAULT;
  const raw = Number(localStorage.getItem(`rh.panel.width.${key}`));
  return Number.isFinite(raw) && raw >= WIDTH_MIN && raw <= WIDTH_MAX ? raw : WIDTH_DEFAULT;
}

export function Panel({
  className,
  children,
  collapseKey,
}: {
  className?: string;
  children: React.ReactNode;
  collapseKey?: string;
}) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState(
    () => !!collapseKey && localStorage.getItem(`rh.panel.${collapseKey}`) === "1",
  );
  const [width, setWidth] = useState(() => loadWidth(collapseKey));
  const toggle = () => {
    setCollapsed((prev) => {
      const next = !prev;
      try {
        localStorage.setItem(`rh.panel.${collapseKey}`, next ? "1" : "0");
      } catch {
        // storage unavailable
      }
      return next;
    });
  };

  const startResize = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const target = e.currentTarget;
    const startX = e.clientX;
    const startWidth = width;
    let latest = startWidth;
    try {
      target.setPointerCapture(e.pointerId);
    } catch {
      // synthetic or stale pointer: dragging still works via listeners
    }
    const onMove = (ev: PointerEvent) => {
      latest = Math.min(WIDTH_MAX, Math.max(WIDTH_MIN, startWidth + (ev.clientX - startX)));
      setWidth(latest);
    };
    const onUp = () => {
      target.removeEventListener("pointermove", onMove);
      target.removeEventListener("pointerup", onUp);
      try {
        if (collapseKey) localStorage.setItem(`rh.panel.width.${collapseKey}`, String(latest));
      } catch {
        // storage unavailable: width still applies for this page
      }
    };
    target.addEventListener("pointermove", onMove);
    target.addEventListener("pointerup", onUp);
  };

  const isCollapsed = !!collapseKey && collapsed;
  const resizable = !!collapseKey;

  // One stable DOM tree: collapsing only flips css classes, so the (large)
  // panel content never unmounts and the toggle is instant. Phones ignore
  // the collapsed state entirely.
  return (
    <PanelCollapseContext.Provider value={collapseKey ? { toggle } : null}>
      {isCollapsed && (
        <aside className="hidden w-7 shrink-0 flex-col items-center border-r border-border bg-panel py-1.5 md:flex">
          <button
            type="button"
            title={t("expand panel")}
            onClick={toggle}
            className="flex size-5 items-center justify-center text-muted hover:bg-hover hover:text-fg"
          >
            <CaretDoubleRightIcon className="size-3.5" />
          </button>
          <span
            data-label="" className="label-caps mt-2 text-2xs text-faint"
            style={{ writingMode: "vertical-rl" }}
          >
            {collapseKey}
          </span>
        </aside>
      )}
      <aside
        className={cn(
          "relative flex shrink-0 flex-col border-border bg-panel max-md:w-full md:border-r",
          resizable ? "md:w-[var(--panel-w)]" : "md:w-76",
          isCollapsed && "md:hidden",
          className,
        )}
        style={resizable ? { ["--panel-w" as string]: `${width}px` } : undefined}
      >
        {children}
        {/* Where the shared 32px header line meets this panel's edge (desktop). */}
        <RegMark className="top-[27px] right-[-5px]" />
        {resizable && (
          <div
            onPointerDown={startResize}
            title={t("drag to resize")}
            className="absolute inset-y-0 -right-[2px] z-10 hidden w-[5px] cursor-col-resize hover:bg-fg/30 active:bg-fg/60 md:block"
          />
        )}
      </aside>
    </PanelCollapseContext.Provider>
  );
}

export function PanelHeader({ title, extra }: { title: string; extra?: React.ReactNode }) {
  const { t } = useTranslation();
  const collapse = useContext(PanelCollapseContext);
  return (
    <div className="flex h-8 shrink-0 items-center justify-between gap-2 border-b border-border px-2.5">
      <span className="flex min-w-0 items-center gap-1">
        {collapse && (
          <button
            type="button"
            title={t("collapse panel")}
            onClick={collapse.toggle}
            className="flex size-5 shrink-0 items-center justify-center text-faint hover:bg-hover hover:text-fg max-md:hidden"
          >
            <CaretDoubleLeftIcon className="size-3.5" />
          </button>
        )}
        <span className="truncate text-xs font-semibold text-fg">{title}</span>
      </span>
      {extra}
    </div>
  );
}

export function Content({ className, children }: { className?: string; children: React.ReactNode }) {
  return <main className={cn("flex min-w-0 flex-1 flex-col", className)}>{children}</main>;
}
