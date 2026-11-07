import { Menu } from "@base-ui/react/menu";
import { ArchiveIcon, DotsThreeIcon, CircleNotchIcon, PencilSimpleIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";

/** Low-frequency actions share one slot; search and pin remain directly accessible. */
export function SessionActionsMenu({
  onRename,
  onArchive,
  archiving,
}: {
  onRename: () => void;
  onArchive?: () => void;
  archiving: boolean;
}) {
  const { t } = useTranslation();
  return (
    <Menu.Root>
      <Menu.Trigger
        title={t("session actions")}
        aria-label={t("session actions")}
        className="flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
      >
        {archiving ? <CircleNotchIcon className="size-3.5 animate-spin" /> : <DotsThreeIcon className="size-3.5" />}
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={4} className="z-50 outline-none">
          <Menu.Popup className="min-w-44 max-w-[calc(100vw-2rem)] border border-border-strong bg-bg py-1 outline-none">
            <Menu.Item
              onClick={onRename}
              className="flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left font-mono text-2xs text-fg outline-none data-[highlighted]:bg-hover"
            >
              <PencilSimpleIcon className="size-3.5 shrink-0" />
              {t("rename session")}
            </Menu.Item>
            {onArchive && (
              <Menu.Item
                disabled={archiving}
                onClick={onArchive}
                className="flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left font-mono text-2xs text-danger outline-none data-[disabled]:pointer-events-none data-[disabled]:opacity-40 data-[highlighted]:bg-hover"
              >
                <ArchiveIcon className="size-3.5 shrink-0" />
                {t("archive session")}
              </Menu.Item>
            )}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
