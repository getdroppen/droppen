import { useQuery } from "@tanstack/react-query";
import { ArrowLeftIcon, DownloadSimpleIcon, FileTextIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { DownloadLink, HostImage } from "@/components/host-file";
import { Spinner } from "@/components/ui/spinner";
import { basename, formatBytes, formatTime } from "@/lib/format";
import { isPreviewImage } from "./files-model";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

export function FilePreview({
  file,
  onClose,
}: {
  file: string;
  onClose?: () => void;
}) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const image = isPreviewImage(file);
  const [imageError, setImageError] = useState(false);
  const { data, isPending, isError, refetch } = useQuery({
    queryKey: mq(machine, "fs-read", file),
    queryFn: async ({ signal }) => {
      const res = await api.GET("/api/fs/read", {
        params: { query: { path: file } },
        signal,
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    enabled: !image,
    retry: false,
  });
  return (
    <section
      className="files-preview flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
      aria-label={t("file preview")}
    >
      {/* Always reachable, including loading, permission failures and binary files. */}
      <header className="flex min-h-10 shrink-0 items-center gap-2 border-b border-border px-2">
        {onClose && (
          <button
            type="button"
            aria-label={t("back to file list")}
            title={t("back to file list")}
            onClick={onClose}
            className="flex size-8 shrink-0 items-center justify-center rounded text-muted hover:bg-hover"
          >
            <ArrowLeftIcon className="size-4" />
          </button>
        )}
        <span
          className="min-w-0 flex-1 truncate font-mono text-xs"
          title={file}
        >
          {basename(file)}
        </span>
        <DownloadLink
          path={file}
          title={t("download")}
          className="flex shrink-0 items-center gap-1 rounded px-2 py-2 text-xs text-accent hover:bg-hover"
        >
          <DownloadSimpleIcon className="size-3.5" />
          {t("download")}
        </DownloadLink>
      </header>
      {image ? (
        <div
          className="min-h-0 flex-1 overflow-auto p-4"
          onLoadCapture={() => setImageError(false)}
          onErrorCapture={() => setImageError(true)}
        >
          {imageError && (
            <p role="alert" data-signal="failed" className="mb-3 text-xs text-danger">
              {t("cannot preview image · use download")}
            </p>
          )}
          <HostImage
            path={file}
            alt={basename(file)}
            className="mx-auto max-h-full max-w-full object-contain"
            onFail={() => setImageError(true)}
          />
        </div>
      ) : isPending ? (
        <Spinner className="flex-1" />
      ) : isError || !data ? (
        <div role="alert" data-signal="failed" className="p-4 text-xs text-danger">
          {t("cannot read file")}
          <button
            type="button"
            onClick={() => void refetch()}
            className="ml-3 text-accent underline"
          >
            {t("retry")}
          </button>
        </div>
      ) : (
        <>
          <div className="flex shrink-0 flex-wrap gap-2 border-b border-border px-3 py-1 text-2xs text-faint">
            <span>{formatBytes(data.sizeBytes)}</span>
            <span>{formatTime(data.mtime)}</span>
            {data.truncated && (
              <span data-signal="warn" className="text-warn-fg">
                {t("preview truncated · download the complete file")}
              </span>
            )}
          </div>
          {data.binary ? (
            <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 overflow-auto p-4 text-center text-xs text-muted">
              <FileTextIcon className="size-8 shrink-0 text-faint" />
              <p className="max-w-full break-words font-mono text-fg">{basename(file)}</p>
              <p>{t("binary file · use download")}</p>
            </div>
          ) : (
            <pre className="min-h-0 flex-1 overflow-auto p-3 font-mono text-xs leading-5 whitespace-pre-wrap break-words">
              {data.content}
            </pre>
          )}
        </>
      )}
    </section>
  );
}
