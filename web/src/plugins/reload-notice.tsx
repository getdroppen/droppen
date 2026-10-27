/**
 * "Reload to finish": shown in Settings › Plugins and on a plugin's page when
 * the instance's list moved under the page — a plugin was installed,
 * upgraded or uninstalled elsewhere (`docs/plugins/runtime.md` §6). A module
 * cannot be unloaded, so only a reload can run the new list.
 */
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";

export function ReloadNotice({ className }: { className?: string }) {
  const { t } = useTranslation();
  return (
    <div role="alert" data-signal="failed" className={cn("flex max-w-xl items-start gap-3 border border-border px-3 py-2", className)}>
      <span className="min-w-0 flex-1 text-xs break-words text-fg">
        {t("a plugin was installed, upgraded or uninstalled elsewhere: reload this page to finish")}
      </span>
      <button
        type="button"
        onClick={() => window.location.reload()}
        className="flex h-7 shrink-0 items-center border border-border-strong bg-bg px-3 text-xs font-medium text-fg hover:bg-hover"
      >
        {t("reload")}
      </button>
    </div>
  );
}
