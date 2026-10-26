import { ArrowUpRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import { Dialog, DialogContent } from "@/components/ui/dialog";

/**
 * `POST /api/apps`'s refusal when the device has no `code-server` on its
 * server's `PATH` (`server-rs/src/apps/mod.rs`): a token beside the server's
 * own sentence, so the words here can be the reader's and say what to do.
 */
export const CODE_SERVER_MISSING = "code_server_missing";

/** code-server's own install guide: Droppen does not install it. */
export const CODE_SERVER_INSTALL_URL = "https://coder.com/docs/code-server/install";

/** Whether an app failed to start because code-server is not installed. */
export function isCodeServerMissing(error: unknown): boolean {
  return error instanceof ApiError && error.code === CODE_SERVER_MISSING;
}

/** The dialog's title, and what the rail's 「+」 says on hover after such a failure. */
export const CODE_SERVER_MISSING_TITLE = "VS Code needs code-server";

/** What the rail's 「+」 says on hover after an app failed to start. */
export function appLaunchErrorText(error: unknown, t: (key: string) => string): string {
  return isCodeServerMissing(error) ? t(CODE_SERVER_MISSING_TITLE) : errorText(error);
}

/**
 * What to do about it: install code-server where the server finds it, and
 * open VS Code again. `~/.local/bin` because it is on the `PATH` of the
 * server both the installer (`scripts/install.sh`, `service_path`) and the
 * desktop app (`desktop/core/src/env_path.rs`) run, and the server looks the
 * program up on every launch, so no restart.
 */
export function CodeServerMissingNotice() {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2 text-xs leading-[18px] text-fg">
      <p>{t("VS Code runs in code-server, which Droppen does not install. It is not installed on this device, or it is not on the PATH of this device's server.")}</p>
      <p className="text-muted">{t("Install it, for example in ~/.local/bin: a server set up by the installer or the Droppen app has that on its PATH. Then open VS Code again; the server does not need a restart.")}</p>
      <a
        href={CODE_SERVER_INSTALL_URL}
        target="_blank"
        rel="noopener noreferrer"
        data-testid="code-server-install-link"
        className="inline-flex items-center gap-0.5 self-start text-xs text-fg underline decoration-border-strong underline-offset-[3px] hover:decoration-fg"
      >
        {t("How to install code-server")}
        <ArrowUpRight className="size-3" aria-hidden="true" />
      </a>
    </div>
  );
}

/** {@link CodeServerMissingNotice}, as the dialog a failed 「+ → vscode」 opens. */
export function CodeServerMissingDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={t(CODE_SERVER_MISSING_TITLE)} closeLabel={t("Close")}>
        <CodeServerMissingNotice />
      </DialogContent>
    </Dialog>
  );
}
