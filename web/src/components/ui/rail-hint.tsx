import { Tooltip } from "@base-ui/react/tooltip";
import { type ReactElement, type ReactNode } from "react";
import { useMediaQuery } from "usehooks-ts";

const COLUMN = "(min-width: 768px)";

export function RailHint({ label, render, children }: { label: string; render: ReactElement<Record<string, unknown>>; children: ReactNode }) {
  const column = useMediaQuery(COLUMN, { defaultValue: true });
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
