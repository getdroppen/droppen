import { useTranslation } from "react-i18next";
import { Menu } from "@base-ui/react/menu";
import { DotsThreeIcon } from "@phosphor-icons/react";
import { cn } from "@/lib/cn";

/** One action of a row's 「⋯」 menu. */
export interface RowMenuItem {
  /** Its test id: `promote-<id>`, `host-<id>`, `remove-<id>`… */
  id: string;
  label: string;
  /** Why it cannot be pressed now, said under it (a tooltip does not reach a finger); `null`: it can. */
  blocked: string | null;
  /** What it does, said under it when that is not plain from its label. */
  hint?: string;
  /** Something else is under way: not pressable, nothing to say. */
  busy?: boolean;
  danger?: boolean;
  onSelect: () => void;
}

/** A row's 「⋯」: its actions as the app's menus draw them (24px rows, 36px under a finger), a destructive one last and apart. */
export function RowMenu({ id, items }: { id: string; items: RowMenuItem[] }) {
  const { t } = useTranslation();
  if (!items.length) return null;
  return (
    <Menu.Root>
      <Menu.Trigger
        data-testid={`actions-${id}`}
        title={t("more actions")}
        aria-label={t("more actions")}
        className="flex size-7 shrink-0 items-center justify-center border border-transparent text-muted hover:border-border-strong hover:text-fg data-[popup-open]:border-border-strong data-[popup-open]:text-fg pointer-coarse:size-9"
      >
        <DotsThreeIcon className="size-4" />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={4} collisionPadding={8} className="z-50 outline-none">
          <Menu.Popup data-testid={`actions-menu-${id}`} className="max-h-[min(var(--available-height),20rem)] min-w-44 max-w-[min(var(--available-width),20rem)] overflow-y-auto border border-border-strong bg-bg py-1 outline-none">
            {items.map((item, i) => (
              <div key={item.id}>
                {item.danger && i > 0 && <Menu.Separator className="my-1 h-px bg-border" />}
                <Menu.Item
                  data-testid={item.id}
                  data-signal={item.danger ? "failed" : undefined}
                  disabled={item.busy || item.blocked !== null}
                  onClick={item.onSelect}
                  className={cn(
                    "flex min-h-6 w-full cursor-pointer flex-col items-start justify-center gap-0.5 px-2 py-1 text-left font-mono text-2xs outline-none select-none data-[highlighted]:bg-hover",
                    "pointer-coarse:min-h-9 pointer-coarse:text-sm data-[disabled]:cursor-default",
                    item.danger ? "text-danger" : "text-fg",
                  )}
                >
                  <span className={cn((item.busy || item.blocked !== null) && "opacity-40")}>{item.label}</span>
                  {(item.blocked ?? item.hint) && <span className="max-w-64 text-[10px] leading-3.5 whitespace-normal text-muted pointer-coarse:font-sans-text pointer-coarse:text-xs">{item.blocked ?? item.hint}</span>}
                </Menu.Item>
              </div>
            ))}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
