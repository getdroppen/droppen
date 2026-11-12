import { Autocomplete } from "@base-ui/react/autocomplete";
import { useTranslation } from "react-i18next";

export function Combobox({
  value,
  onValueChange,
  items,
  placeholder,
}: {
  value: string;
  onValueChange: (value: string) => void;
  items: string[];
  placeholder?: string;
}) {
  const { t } = useTranslation();
  return (
    <Autocomplete.Root
      items={items}
      value={value}
      onValueChange={onValueChange}
      openOnInputClick
    >
      <Autocomplete.Input
        placeholder={placeholder}
        className="h-8 w-full border border-border bg-bg px-3 text-[13px] text-fg placeholder:text-faint hover:border-border-strong focus:border-fg max-md:h-10"
      />
      <Autocomplete.Portal>
        <Autocomplete.Positioner sideOffset={4} className="z-50 outline-none">
          <Autocomplete.Popup className="popup-motion max-h-[min(var(--available-height),20rem)] min-w-[max(var(--anchor-width),10rem)] max-w-[min(var(--available-width),20rem)] overflow-y-auto border border-border-strong bg-bg py-1.5 outline-none">
            <Autocomplete.Empty className="px-3 py-2 text-[13px] text-muted">
              {t("no known directory matches — the typed path is used as-is")}
            </Autocomplete.Empty>
            <Autocomplete.List>
              {(item: string) => (
                <Autocomplete.Item
                  key={item}
                  value={item}
                  className="flex min-h-8 cursor-pointer items-center truncate px-3 py-1 font-mono text-xs text-fg outline-none pointer-coarse:min-h-11 data-[highlighted]:bg-hover"
                >
                  {item}
                </Autocomplete.Item>
              )}
            </Autocomplete.List>
          </Autocomplete.Popup>
        </Autocomplete.Positioner>
      </Autocomplete.Portal>
    </Autocomplete.Root>
  );
}
