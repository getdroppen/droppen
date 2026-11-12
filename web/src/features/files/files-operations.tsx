import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CircleNotchIcon, UploadSimpleIcon } from "@phosphor-icons/react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { blobErrorText } from "@/components/blob-transfers";
import { authHeader, noteUnauthorized } from "@/lib/auth";
import { sendUpload } from "@/lib/transport/blob";
import { useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { transportFor } from "@/lib/machines/registry";

export function UploadButton({ dir }: { dir: string }) {
  const machine = useMachine();
  const { t } = useTranslation();
  const input = useRef<HTMLInputElement>(null);
  const queryClient = useQueryClient();
  const [status, setStatus] = useState("");
  const upload = useMutation({
    // Capture the destination once: navigation during an upload must not redirect later files.
    mutationFn: async ({
      files,
      destination,
    }: {
      files: File[];
      destination: string;
    }) => {
      const failures: string[] = [];
      for (const [index, file] of files.entries()) {
        setStatus(`${index + 1}/${files.length}`);
        try {
          const form = new FormData();
          form.append("file", file);
          // On the channel's file lane in cloud mode, and refused here past
          // what the machine takes in one request.
          const res = await sendUpload(
            new Request(`/api/fs/upload?dir=${encodeURIComponent(destination)}`, { method: "POST", headers: authHeader(), body: form }),
            file.size,
            transportFor(machine),
          );
          if (res.status === 401) noteUnauthorized("/api/fs/upload");
          if (!res.ok) {
            const body = (await res.json()) as { error?: string; hint?: string };
            throw new Error(body.hint ?? body.error ?? t("upload failed"));
          }
        } catch (error) {
          failures.push(
            `${file.name}: ${error instanceof Error ? blobErrorText(error, t) : t("upload failed")}`,
          );
        }
      }
      await queryClient.invalidateQueries({ queryKey: mq(machine, "fs-list") });
      if (failures.length) throw new Error(failures.join("; "));
    },
    onSuccess: () => setStatus(t("upload complete")),
    onError: (error: Error) => setStatus(error.message),
  });
  return (
    <div className="relative flex min-w-0 items-center gap-2">
      {status && (
        <span
          role={upload.isError ? "alert" : "status"}
          title={status}
          className={`max-w-32 truncate text-2xs ${upload.isError ? "text-danger" : "text-muted"}`}
        >
          {status}
        </span>
      )}
      <button
        type="button"
        title={t("upload into this directory")}
        disabled={upload.isPending}
        onClick={() => input.current?.click()}
        className="flex shrink-0 items-center gap-1 rounded px-2 py-1 text-xs text-accent hover:bg-hover disabled:opacity-50"
      >
        {upload.isPending ? (
          <CircleNotchIcon className="size-3.5 animate-spin" />
        ) : (
          <UploadSimpleIcon className="size-3.5" />
        )}
        {t("upload")}
      </button>
      <input
        ref={input}
        type="file"
        multiple
        className="hidden"
        onChange={(e) => {
          const files = [...(e.target.files ?? [])];
          if (files.length) upload.mutate({ files, destination: dir });
          e.target.value = "";
        }}
      />
    </div>
  );
}
