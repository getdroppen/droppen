import { Tooltip } from "@base-ui/react/tooltip";
import { useSyncExternalStore, type ReactElement, type ReactNode } from "react";

const COLUMN = "(min-width: 768px)";

function subscribe(onChange: () => void): () => void {
  const query = window.matchMedia(COLUMN);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

const isColumn = () => window.matchMedia(COLUMN).matches;

export function RailHint({ label, render, children }: { label: string; render: ReactElement<Record<string, unknown>>; children: ReactNode }) {
  const column = useSyncExternalStore(subscribe, isColumn, () => true);
  return (
    <Tooltip.Root>
      <Tooltip.Trigger delay={400} closeDelay={0} render={render}>
        {children}
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Positioner side={column ? "right" : "top"} align="center" sideOffset={6} className="z-50 outline-none">
          <Tooltip.Popup data-rail-hint="" className="border border-border-strong bg-bg px-2 py-1 font-mono text-2xs whitespace-nowrap text-fg">
            {label}
          </Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
