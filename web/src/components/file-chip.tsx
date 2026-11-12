import { DownloadSimpleIcon, EyeIcon, FileTextIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { DownloadLink } from "@/components/host-file";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { FilePreview } from "@/features/files/file-preview";

/** One host file, two actions: inspect here or save a copy. */
export function FileChip({ name, path }: { name: string; path: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <span className="inline-flex max-w-full items-stretch overflow-hidden rounded border border-border bg-raised text-2xs text-fg">
        <DialogTrigger
          title={path}
          aria-label={t("view {{name}}", { name })}
          aria-haspopup="dialog"
          className="flex min-w-0 items-center gap-1.5 px-2 py-1.5 text-left hover:bg-hover hover:text-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
        >
          <FileTextIcon className="size-3.5 shrink-0 text-muted" />
          <span className="truncate font-mono">{name}</span>
          <EyeIcon className="size-3.5 shrink-0 text-muted" />
        </DialogTrigger>
        <DownloadLink
          path={path}
          name={name}
          title={t("download {{name}}", { name })}
          className="flex w-8 shrink-0 items-center justify-center border-l border-border text-muted hover:bg-hover hover:text-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
        >
          <DownloadSimpleIcon className="size-3.5" />
          <span className="sr-only">{t("download {{name}}", { name })}</span>
        </DownloadLink>
      </span>
      {open && (
        <DialogContent
          title={t("file preview")}
          closeLabel={t("close {{name}}", { name: t("file preview") })}
          className="flex h-[min(42rem,calc(100dvh-2rem))] w-[min(64rem,calc(100vw-2rem))] flex-col overflow-hidden"
        >
          <FilePreview key={path} file={path} />
        </DialogContent>
      )}
    </Dialog>
  );
}
